//! CatalogService: versions, modpack and mod search, and the CurseForge key.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_catalog::{Platform, ProjectKind, SearchQuery};
use serde_json::json;

use crate::AppState;
use crate::content::{catalog_error, icons};
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require};

pub(crate) struct CatalogRpc {
    state: AppState,
}

impl CatalogRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

pub(crate) fn platform(p: Option<pb::Platform>) -> Result<Platform, ConnectError> {
    match p {
        Some(pb::Platform::PLATFORM_MODRINTH) => Ok(Platform::Modrinth),
        Some(pb::Platform::PLATFORM_CURSEFORGE) => Ok(Platform::CurseForge),
        _ => Err(ConnectError::invalid_argument(
            "choose Modrinth or CurseForge",
        )),
    }
}

pub(crate) fn proto_platform(p: Platform) -> pb::Platform {
    match p {
        Platform::Modrinth => pb::Platform::PLATFORM_MODRINTH,
        Platform::CurseForge => pb::Platform::PLATFORM_CURSEFORGE,
    }
}

pub(crate) fn proto_project(p: nn_catalog::Project) -> pb::Project {
    pb::Project {
        platform: proto_platform(p.platform).into(),
        icon_url: icons::proxied(p.icon_url.as_deref()),
        id: p.id,
        slug: p.slug,
        title: p.title,
        summary: p.summary,
        author: p.author,
        downloads: p.downloads,
        loaders: p.loaders,
        game_versions: p.game_versions,
        page_url: p.page_url,
        server_side: p.server_side,
        updated_ms: p.updated_ms,
        ..Default::default()
    }
}

pub(crate) fn proto_version(v: nn_catalog::ProjectVersion) -> pb::ProjectVersion {
    let file = v.primary_file().cloned();
    pb::ProjectVersion {
        filename: file
            .as_ref()
            .map(|f| f.filename.clone())
            .unwrap_or_default(),
        size: file.as_ref().map_or(0, |f| f.size),
        manual_download: file.as_ref().is_some_and(|f| f.url.is_none()),
        id: v.id,
        project_id: v.project_id,
        name: v.name,
        version_number: v.version_number,
        game_versions: v.game_versions,
        loaders: v.loaders,
        channel: v.channel,
        published_ms: v.published_ms,
        ..Default::default()
    }
}

fn opt(s: &str) -> Option<String> {
    Some(s.trim().to_owned()).filter(|s| !s.is_empty())
}

#[allow(refining_impl_trait)]
impl pb::CatalogService for CatalogRpc {
    async fn list_mc_versions(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListMcVersionsRequest>,
    ) -> ServiceResult<pb::ListMcVersionsResponse> {
        require(&ctx, Permission::ManageServers)?;
        let versions = self
            .state
            .content
            .catalog
            .mc_versions(request.loader, request.include_snapshots)
            .await
            .map_err(catalog_error)?;
        Response::ok(pb::ListMcVersionsResponse {
            versions: versions
                .into_iter()
                .map(|v| pb::McVersion {
                    id: v.id,
                    snapshot: v.snapshot,
                    released_ms: v.released_ms,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn list_loader_versions(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListLoaderVersionsRequest>,
    ) -> ServiceResult<pb::ListLoaderVersionsResponse> {
        require(&ctx, Permission::ManageServers)?;
        let versions = self
            .state
            .content
            .catalog
            .loader_versions(request.loader, request.mc_version.trim())
            .await
            .map_err(catalog_error)?;
        Response::ok(pb::ListLoaderVersionsResponse {
            versions: versions
                .into_iter()
                .map(|v| pb::LoaderVersion {
                    version: v.version,
                    stable: v.stable,
                    recommended: v.recommended,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    async fn search_projects(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SearchProjectsRequest>,
    ) -> ServiceResult<pb::SearchProjectsResponse> {
        require(&ctx, Permission::ManageServers)?;
        let kind = match request.kind.as_known() {
            Some(pb::ProjectKind::PROJECT_KIND_MOD) => ProjectKind::Mod,
            _ => ProjectKind::Modpack,
        };
        let query = SearchQuery {
            text: request.query.chars().take(200).collect(),
            mc_version: opt(request.mc_version),
            loader: opt(request.loader),
            offset: request.offset.min(10_000),
            limit: if request.limit == 0 {
                20
            } else {
                request.limit.min(50)
            },
        };
        let content = &self.state.content;
        let page = match platform(request.platform.as_known())? {
            Platform::Modrinth => content.catalog.modrinth_search(kind, &query).await,
            Platform::CurseForge => {
                let key = content.curseforge_key().ok_or_else(|| {
                    ConnectError::failed_precondition(
                        "Add a CurseForge API key in Settings to search CurseForge.",
                    )
                })?;
                content.catalog.curseforge_search(&key, kind, &query).await
            }
        }
        .map_err(catalog_error)?;
        Response::ok(pb::SearchProjectsResponse {
            total: page.total,
            projects: page.projects.into_iter().map(proto_project).collect(),
            ..Default::default()
        })
    }

    async fn list_project_versions(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListProjectVersionsRequest>,
    ) -> ServiceResult<pb::ListProjectVersionsResponse> {
        require(&ctx, Permission::ManageServers)?;
        let (mc, loader) = (opt(request.mc_version), opt(request.loader));
        let content = &self.state.content;
        let versions = match platform(request.platform.as_known())? {
            Platform::Modrinth => {
                content
                    .catalog
                    .modrinth_versions(request.project_id, mc.as_deref(), loader.as_deref())
                    .await
            }
            Platform::CurseForge => {
                let key = content.curseforge_key().ok_or_else(|| {
                    ConnectError::failed_precondition(
                        "Add a CurseForge API key in Settings to list CurseForge files.",
                    )
                })?;
                content
                    .catalog
                    .curseforge_versions(&key, request.project_id, mc.as_deref(), loader.as_deref())
                    .await
            }
        }
        .map_err(catalog_error)?;
        Response::ok(pb::ListProjectVersionsResponse {
            versions: versions.into_iter().map(proto_version).collect(),
            ..Default::default()
        })
    }

    async fn get_curse_forge_status(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetCurseForgeStatusRequest>,
    ) -> ServiceResult<pb::GetCurseForgeStatusResponse> {
        require(&ctx, Permission::ManageServers)?;
        Response::ok(pb::GetCurseForgeStatusResponse {
            configured: self.state.content.curseforge_key().is_some(),
            ..Default::default()
        })
    }

    async fn set_curse_forge_key(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetCurseForgeKeyRequest>,
    ) -> ServiceResult<pb::SetCurseForgeKeyResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let key = opt(request.key);
        if let Some(key) = &key {
            if key.len() > 200 || key.chars().any(char::is_control) {
                return Err(ConnectError::invalid_argument(
                    "that isn't a CurseForge API key",
                ));
            }
            self.state
                .content
                .catalog
                .curseforge_check_key(key)
                .await
                .map_err(catalog_error)?;
        }
        self.state
            .content
            .save_curseforge_key(self.state.auth.store(), key.as_deref())
            .await?;
        self.state.nodes.set_curseforge_key(key.clone());
        // Never the key itself.
        audit(
            &self.state,
            &user,
            "settings.curseforge_key",
            ("settings", "curseforge"),
            json!({ "set": key.is_some() }),
        )
        .await;
        Response::ok(pb::SetCurseForgeKeyResponse::default())
    }
}
