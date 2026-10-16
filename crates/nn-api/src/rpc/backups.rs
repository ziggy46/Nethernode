//! BackupService.

use std::collections::HashMap;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_store::Backup;
use serde_json::json;

use crate::AppState;
use crate::backups::{self, Kind};
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};

pub(crate) struct BackupRpc {
    state: AppState,
}

impl BackupRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn load(&self, id: &str) -> Result<Backup, ConnectError> {
        self.state
            .auth
            .store()
            .find_backup(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("no such backup"))
    }

    async fn server(&self, id: &str) -> Result<nn_store::Server, ConnectError> {
        self.state
            .auth
            .store()
            .find_server(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("server not found"))
    }
}

pub(crate) fn proto_backup(b: Backup, users: &HashMap<String, String>) -> pb::Backup {
    pb::Backup {
        kind: match b.kind.as_str() {
            "scheduled" => pb::BackupKind::BACKUP_KIND_SCHEDULED,
            "pre-restore" => pb::BackupKind::BACKUP_KIND_PRE_RESTORE,
            _ => pb::BackupKind::BACKUP_KIND_MANUAL,
        }
        .into(),
        status: match b.status.as_str() {
            "creating" => pb::BackupStatus::BACKUP_STATUS_CREATING,
            "failed" => pb::BackupStatus::BACKUP_STATUS_FAILED,
            _ => pb::BackupStatus::BACKUP_STATUS_READY,
        }
        .into(),
        created_by: b
            .created_by
            .as_ref()
            .and_then(|u| users.get(u).cloned())
            .unwrap_or_default(),
        id: b.id,
        server_id: b.server_id,
        server_name: b.server_name,
        size_bytes: u64::try_from(b.size_bytes).unwrap_or(0),
        files: u64::try_from(b.files).unwrap_or(0),
        raw_bytes: u64::try_from(b.raw_bytes).unwrap_or(0),
        message: b.message.unwrap_or_default(),
        created_at_ms: b.created_at,
        finished_at_ms: b.finished_at,
        ..Default::default()
    }
}

#[allow(refining_impl_trait)]
impl pb::BackupService for BackupRpc {
    async fn list_backups(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListBackupsRequest>,
    ) -> ServiceResult<pb::ListBackupsResponse> {
        require(&ctx, Permission::View)?;
        let store = self.state.auth.store();
        let backups = store
            .list_backups(request.server_id)
            .await
            .map_err(store_error)?;
        let users: HashMap<String, String> = store
            .list_users()
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|u| (u.id, u.username))
            .collect();
        // Free space where this server's backups go (its machine's disk).
        let node = match request.server_id {
            Some(id) => self.state.nodes.for_server_id(id).await,
            None => self.state.nodes.local().clone(),
        };
        Response::ok(pb::ListBackupsResponse {
            backups: backups
                .into_iter()
                .map(|b| proto_backup(b, &users))
                .collect(),
            free_bytes: node.disk_space().map(|(_, free)| free),
            ..Default::default()
        })
    }

    async fn create_backup(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateBackupRequest>,
    ) -> ServiceResult<pb::CreateBackupResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let server = self.server(request.server_id).await?;
        let (backup_id, job_id) =
            backups::start_backup(&self.state, &server, Kind::Manual, Some(&user.id)).await?;
        audit(
            &self.state,
            &user,
            "backup.create",
            ("server", &server.id),
            json!({ "backup": backup_id }),
        )
        .await;
        Response::ok(pb::CreateBackupResponse {
            backup_id,
            job_id,
            ..Default::default()
        })
    }

    async fn restore_backup(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RestoreBackupRequest>,
    ) -> ServiceResult<pb::RestoreBackupResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let backup = self.load(request.backup_id).await?;
        if backup.status != "ready" {
            return Err(ConnectError::failed_precondition(
                "only finished backups can be restored",
            ));
        }
        let server_id = backup.server_id.clone().ok_or_else(|| {
            ConnectError::failed_precondition("its server was deleted; download the backup instead")
        })?;
        let server = self.server(&server_id).await?;
        let details = json!({ "backup": backup.id, "created_at": backup.created_at, "start_after": request.start_after });
        let job_id =
            backups::start_restore(&self.state, backup, server, &user.id, request.start_after)
                .await?;
        audit(
            &self.state,
            &user,
            "backup.restore",
            ("server", &server_id),
            details,
        )
        .await;
        Response::ok(pb::RestoreBackupResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn delete_backup(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteBackupRequest>,
    ) -> ServiceResult<pb::DeleteBackupResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let backup = self.load(request.backup_id).await?;
        backups::delete(&self.state, &backup).await?;
        audit(
            &self.state,
            &user,
            "backup.delete",
            ("server", backup.server_id.as_deref().unwrap_or("")),
            json!({ "backup": backup.id, "server_name": backup.server_name }),
        )
        .await;
        Response::ok(pb::DeleteBackupResponse::default())
    }
}
