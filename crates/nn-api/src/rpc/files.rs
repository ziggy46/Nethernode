//! `FileService`: browse and edit a server's files. Admins only; every
//! change is audit-logged with its path (never file contents).

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::{AuthUser, Permission};
use nn_files::{EDITOR_LIMIT, Entry, EntryKind, Expect, RelPath, TRASH_RETENTION, TrashItem};
use serde_json::json;

use crate::AppState;
use crate::files::{file_error, open};
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require};

pub(crate) struct FileRpc {
    state: AppState,
}

impl FileRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn record(
        &self,
        user: &AuthUser,
        action: &str,
        server_id: &str,
        details: serde_json::Value,
    ) {
        audit(&self.state, user, action, ("server", server_id), details).await;
    }
}

fn path(raw: &str) -> Result<RelPath, ConnectError> {
    RelPath::parse(raw).map_err(crate::files::file_error)
}

fn kind(kind: EntryKind) -> pb::FileKind {
    match kind {
        EntryKind::File => pb::FileKind::FILE_KIND_FILE,
        EntryKind::Directory => pb::FileKind::FILE_KIND_DIRECTORY,
        EntryKind::Symlink => pb::FileKind::FILE_KIND_SYMLINK,
        EntryKind::Other => pb::FileKind::FILE_KIND_OTHER,
    }
}

fn entry(e: Entry) -> pb::FileEntry {
    pb::FileEntry {
        name: e.name,
        kind: kind(e.kind).into(),
        size: e.size,
        modified_ms: e.modified_ms,
        ..Default::default()
    }
}

fn trash_item(t: TrashItem) -> pb::TrashItem {
    pb::TrashItem {
        id: t.id,
        original_path: t.original,
        kind: kind(t.kind).into(),
        size: t.size,
        deleted_ms: t.deleted_ms,
        ..Default::default()
    }
}

#[allow(refining_impl_trait)]
impl pb::FileService for FileRpc {
    async fn list_files(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListFilesRequest>,
    ) -> ServiceResult<pb::ListFilesResponse> {
        require(&ctx, Permission::ManageServers)?;
        let dir = path(request.path)?;
        let (_, files) = open(&self.state, request.server_id).await?;
        let entries = files.list(&dir).await.map_err(file_error)?;
        Response::ok(pb::ListFilesResponse {
            path: dir.as_str().to_owned(),
            entries: entries.into_iter().map(entry).collect(),
            ..Default::default()
        })
    }

    async fn read_file(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ReadFileRequest>,
    ) -> ServiceResult<pb::ReadFileResponse> {
        require(&ctx, Permission::ManageServers)?;
        let file = path(request.path)?;
        let (_, files) = open(&self.state, request.server_id).await?;
        let content = files.read(&file, EDITOR_LIMIT).await.map_err(file_error)?;
        Response::ok(pb::ReadFileResponse {
            content: content.data,
            is_text: content.is_text,
            version: content.version,
            modified_ms: content.modified_ms,
            ..Default::default()
        })
    }

    async fn write_file(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::WriteFileRequest>,
    ) -> ServiceResult<pb::WriteFileResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let file = path(request.path)?;
        if request.content.len() as u64 > EDITOR_LIMIT {
            return Err(ConnectError::invalid_argument(
                "files over 5 MB can't be saved from the editor; upload them instead",
            ));
        }
        let expect = match (request.create_only, request.expected_version) {
            (true, _) => Expect::New,
            (false, Some(version)) => Expect::Version(version.to_owned()),
            (false, None) => Expect::Any,
        };
        let (server, files) = open(&self.state, request.server_id).await?;
        let data = request.content.to_vec();
        let size = data.len();
        let version = files.write(&file, data, expect).await.map_err(file_error)?;
        self.record(
            &user,
            "file.write",
            &server.id,
            json!({ "path": file.as_str(), "size": size }),
        )
        .await;
        Response::ok(pb::WriteFileResponse {
            version,
            ..Default::default()
        })
    }

    async fn make_directory(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::MakeDirectoryRequest>,
    ) -> ServiceResult<pb::MakeDirectoryResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let dir = path(request.path)?;
        let (server, files) = open(&self.state, request.server_id).await?;
        files.mkdir(&dir).await.map_err(file_error)?;
        self.record(
            &user,
            "file.mkdir",
            &server.id,
            json!({ "path": dir.as_str() }),
        )
        .await;
        Response::ok(pb::MakeDirectoryResponse::default())
    }

    async fn rename_file(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RenameFileRequest>,
    ) -> ServiceResult<pb::RenameFileResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (from, to) = (path(request.from)?, path(request.to)?);
        let (server, files) = open(&self.state, request.server_id).await?;
        files.rename(&from, &to).await.map_err(file_error)?;
        self.record(
            &user,
            "file.rename",
            &server.id,
            json!({ "from": from.as_str(), "to": to.as_str() }),
        )
        .await;
        Response::ok(pb::RenameFileResponse::default())
    }

    async fn copy_file(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CopyFileRequest>,
    ) -> ServiceResult<pb::CopyFileResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (from, to) = (path(request.from)?, path(request.to)?);
        let (server, files) = open(&self.state, request.server_id).await?;
        let report = files.copy(&from, &to).await.map_err(file_error)?;
        self.record(
            &user,
            "file.copy",
            &server.id,
            json!({ "from": from.as_str(), "to": to.as_str(), "files": report.files }),
        )
        .await;
        Response::ok(pb::CopyFileResponse {
            files: report.files,
            skipped_links: report.skipped_links,
            ..Default::default()
        })
    }

    async fn delete_files(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteFilesRequest>,
    ) -> ServiceResult<pb::DeleteFilesResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let paths = request
            .paths
            .iter()
            .map(|p| path(p))
            .collect::<Result<Vec<_>, _>>()?;
        if paths.is_empty() {
            return Err(ConnectError::invalid_argument("choose something to delete"));
        }
        let (server, files) = open(&self.state, request.server_id).await?;
        let shown: Vec<String> = paths.iter().map(|p| p.as_str().to_owned()).collect();
        let items = files
            .delete(paths, TRASH_RETENTION)
            .await
            .map_err(file_error)?;
        self.record(&user, "file.delete", &server.id, json!({ "paths": shown }))
            .await;
        Response::ok(pb::DeleteFilesResponse {
            items: items.into_iter().map(trash_item).collect(),
            ..Default::default()
        })
    }

    async fn list_trash(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListTrashRequest>,
    ) -> ServiceResult<pb::ListTrashResponse> {
        require(&ctx, Permission::ManageServers)?;
        let (_, files) = open(&self.state, request.server_id).await?;
        let items = files.trash(TRASH_RETENTION).await.map_err(file_error)?;
        Response::ok(pb::ListTrashResponse {
            items: items.into_iter().map(trash_item).collect(),
            retention_days: u32::try_from(TRASH_RETENTION.as_secs() / 86_400).unwrap_or(30),
            ..Default::default()
        })
    }

    async fn restore_file(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RestoreFileRequest>,
    ) -> ServiceResult<pb::RestoreFileResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (server, files) = open(&self.state, request.server_id).await?;
        let restored = files.restore(request.trash_id).await.map_err(file_error)?;
        self.record(
            &user,
            "file.restore",
            &server.id,
            json!({ "path": restored.as_str() }),
        )
        .await;
        Response::ok(pb::RestoreFileResponse {
            path: restored.as_str().to_owned(),
            ..Default::default()
        })
    }

    async fn purge_trash(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::PurgeTrashRequest>,
    ) -> ServiceResult<pb::PurgeTrashResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (server, files) = open(&self.state, request.server_id).await?;
        let id = request.trash_id;
        let details = json!({ "trash_id": id });
        files.purge(id).await.map_err(file_error)?;
        self.record(&user, "file.purge", &server.id, details).await;
        Response::ok(pb::PurgeTrashResponse::default())
    }

    async fn extract_archive(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExtractArchiveRequest>,
    ) -> ServiceResult<pb::ExtractArchiveResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (archive, dest) = (path(request.archive)?, path(request.destination)?);
        let overwrite = request.overwrite;
        let (server, files) = open(&self.state, request.server_id).await?;
        let details = json!({ "archive": archive.as_str(), "destination": dest.as_str() });
        let job_id = files.extract(archive, dest, overwrite);
        self.record(&user, "file.extract", &server.id, details)
            .await;
        Response::ok(pb::ExtractArchiveResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn compress_files(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CompressFilesRequest>,
    ) -> ServiceResult<pb::CompressFilesResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let sources = request
            .paths
            .iter()
            .map(|p| path(p))
            .collect::<Result<Vec<_>, _>>()?;
        let dest = path(request.destination)?;
        let (server, files) = open(&self.state, request.server_id).await?;
        let details = json!({
            "paths": sources.iter().map(RelPath::as_str).collect::<Vec<_>>(),
            "destination": dest.as_str(),
        });
        let job_id = files.compress(sources, dest);
        self.record(&user, "file.compress", &server.id, details)
            .await;
        Response::ok(pb::CompressFilesResponse {
            job_id,
            ..Default::default()
        })
    }
}
