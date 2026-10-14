//! ModService: the mods folder of modded servers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::{AuthUser, Permission};
use nn_catalog::jar::{read_mod_info, sha1_hex};
use nn_catalog::{Platform, ProjectVersion};
use nn_files::{EntryKind, Expect, FileError, RelPath, ServerFiles};
use nn_store::Server;
use serde_json::json;

use crate::AppState;
use crate::content::{catalog_error, manual};
use crate::files::{blocking, open, spawn_job};
use crate::proto::nethernode::v1 as pb;
use crate::rpc::catalog::{platform, proto_project, proto_version};
use crate::rpc::{audit, require};

const MODS_DIR: &str = "mods";
const DISABLED: &str = ".disabled";
/// Largest mod file installed.
const MAX_MOD: u64 = 256 * 1024 * 1024;
/// Most mods installed for one request (the mod plus what it requires).
const MAX_INSTALL: usize = 25;

pub(crate) struct ModRpc {
    state: AppState,
}

impl ModRpc {
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

/// The loader that runs a server's mods, if it has any.
pub(crate) fn mod_loader(server: &Server) -> Option<String> {
    match server.loader.as_str() {
        "fabric" | "quilt" | "forge" | "neoforge" => Some(server.loader.clone()),
        "modrinth" | "curseforge" => server
            .modpack_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<nn_orchestrator::spec::Modpack>(j).ok())
            .and_then(|p| p.loader)
            .or_else(|| Some(String::new())),
        _ => None,
    }
}

fn mods_dir() -> RelPath {
    RelPath::parse(MODS_DIR).expect("valid path")
}

/// A mod file name: one component ending in `.jar` or `.jar.disabled`.
pub(crate) fn mod_path(filename: &str) -> Result<RelPath, ConnectError> {
    let name = filename.trim();
    if !(name.ends_with(".jar") || name.ends_with(".jar.disabled")) {
        return Err(ConnectError::invalid_argument("that isn't a mod file"));
    }
    nn_files::validate_name(name).map_err(crate::files::file_error)?;
    mods_dir().join(name).map_err(crate::files::file_error)
}

fn is_mod_file(name: &str) -> bool {
    name.ends_with(".jar") || name.ends_with(".jar.disabled")
}

/// A jar's SHA-1 (how Modrinth identifies files).
fn sha1_of(files: &ServerFiles, entry: &nn_files::Entry) -> Option<String> {
    let path = mods_dir().join(&entry.name).ok()?;
    let (file, _) = files.open_read(&path).ok()?;
    sha1_hex(std::io::BufReader::new(file)).ok()
}

async fn mod_server(
    state: &AppState,
    server_id: &str,
) -> Result<(Server, Arc<ServerFiles>, String), ConnectError> {
    let (server, files) = open(state, server_id).await?;
    let loader = mod_loader(&server).ok_or_else(|| {
        ConnectError::failed_precondition(format!("{} doesn't run mods", server.name))
    })?;
    Ok((server, files, loader))
}

/// Creates mods/ if it isn't there yet.
fn ensure_mods_dir(files: &ServerFiles) -> Result<(), FileError> {
    match files.mkdir(&mods_dir()) {
        Err(FileError::AlreadyExists(_)) | Ok(()) => Ok(()),
        Err(e) => Err(e),
    }
}

impl ModRpc {
    /// Resolves the version to install and what it requires (Modrinth),
    /// for this server's Minecraft version and loader.
    async fn plan(
        &self,
        platform: Platform,
        project_id: &str,
        version_id: Option<&str>,
        mc: &str,
        loader: &str,
    ) -> Result<Vec<ProjectVersion>, ConnectError> {
        let catalog = &self.state.content.catalog;
        let key = self.state.content.curseforge_key();
        let loader = Some(loader).filter(|l| !l.is_empty());
        let pick = |versions: Vec<ProjectVersion>, what: &str| {
            versions
                .iter()
                .position(|v| v.channel == "release")
                .or(if versions.is_empty() { None } else { Some(0) })
                .map(|i| versions[i].clone())
                .ok_or_else(|| {
                    ConnectError::not_found(format!(
                        "{what} has no version for Minecraft {mc}{}",
                        loader.map(|l| format!(" with {l}")).unwrap_or_default()
                    ))
                })
        };
        let first = match (platform, version_id) {
            (Platform::Modrinth, Some(id)) => {
                catalog.modrinth_version(id).await.map_err(catalog_error)?
            }
            (Platform::Modrinth, None) => pick(
                catalog
                    .modrinth_versions(project_id, Some(mc), loader)
                    .await
                    .map_err(catalog_error)?,
                "this mod",
            )?,
            (Platform::CurseForge, _) => {
                let key = key.ok_or_else(|| {
                    ConnectError::failed_precondition(
                        "Add a CurseForge API key in Settings to install from CurseForge.",
                    )
                })?;
                match version_id {
                    Some(id) => catalog
                        .curseforge_version(&key, project_id, id)
                        .await
                        .map_err(catalog_error)?,
                    None => pick(
                        catalog
                            .curseforge_versions(&key, project_id, Some(mc), loader)
                            .await
                            .map_err(catalog_error)?,
                        "this mod",
                    )?,
                }
            }
        };
        let mut plan = vec![first];
        if platform != Platform::Modrinth {
            return Ok(plan);
        }
        // Required dependencies, breadth first.
        let mut seen: HashSet<String> = plan.iter().map(|v| v.project_id.clone()).collect();
        let mut i = 0;
        while i < plan.len() && plan.len() < MAX_INSTALL {
            let deps: Vec<_> = plan[i]
                .dependencies
                .iter()
                .filter(|d| d.required)
                .cloned()
                .collect();
            for dep in deps {
                let version = match (&dep.version_id, &dep.project_id) {
                    (Some(id), _) => catalog.modrinth_version(id).await.ok(),
                    (None, Some(project)) if !seen.contains(project) => catalog
                        .modrinth_versions(project, Some(mc), loader)
                        .await
                        .ok()
                        .and_then(|v| pick(v, "a required mod").ok()),
                    _ => None,
                };
                if let Some(v) = version
                    && seen.insert(v.project_id.clone())
                {
                    plan.push(v);
                }
            }
            i += 1;
        }
        Ok(plan)
    }

    /// Downloads `plan` into mods/ as a background job, skipping files
    /// already there. `replace` is trashed once its replacement is in.
    fn start_install(
        &self,
        server: &Server,
        files: Arc<ServerFiles>,
        plan: Vec<ProjectVersion>,
        replace: Option<RelPath>,
    ) -> String {
        let catalog = self.state.content.catalog.clone();
        let handle = tokio::runtime::Handle::current();
        spawn_job(&self.state, &server.id, "install", move |progress| {
            ensure_mods_dir(&files)?;
            let existing: HashSet<String> = files
                .list(&mods_dir())?
                .into_iter()
                .map(|e| e.name.trim_end_matches(DISABLED).to_owned())
                .collect();
            let total = plan.len() as u64;
            let mut installed = Vec::new();
            for (n, version) in plan.iter().enumerate() {
                progress(n as u64, total);
                let Some(file) = version.primary_file() else {
                    continue;
                };
                if existing.contains(&file.filename) && replace.is_none() {
                    continue;
                }
                nn_files::validate_name(&file.filename)?;
                let data = handle
                    .block_on(catalog.download(file, MAX_MOD))
                    .map_err(|e| FileError::Invalid(e.to_string()))?;
                let path = mods_dir().join(&file.filename)?;
                files.write(&path, &data, Expect::Any)?;
                installed.push(file.filename.clone());
            }
            if let Some(old) = &replace
                && !installed
                    .iter()
                    .any(|name| old.file_name() == Some(name.as_str()))
            {
                files.delete(old)?;
            }
            progress(total, total);
            Ok(match installed.as_slice() {
                [] => "Already installed.".to_owned(),
                [one] => format!("Installed {one}. Restart the server to load it."),
                many => format!(
                    "Installed {} files ({}). Restart the server to load them.",
                    many.len(),
                    many.join(", ")
                ),
            })
        })
    }
}

#[allow(refining_impl_trait)]
impl pb::ModService for ModRpc {
    async fn list_mods(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListModsRequest>,
    ) -> ServiceResult<pb::ListModsResponse> {
        require(&ctx, Permission::ManageServers)?;
        let (server, files) = open(&self.state, request.server_id).await?;
        let Some(loader) = mod_loader(&server) else {
            return Response::ok(pb::ListModsResponse {
                supported: false,
                mc_version: server.mc_version,
                ..Default::default()
            });
        };
        let curseforge = server.loader == "curseforge";
        let (mods, manual_downloads) = blocking(move || {
            let entries = match files.list(&mods_dir()) {
                Ok(entries) => entries,
                Err(FileError::NotFound(_)) => Vec::new(),
                Err(e) => return Err(e),
            };
            let mut mods = Vec::new();
            for entry in entries
                .into_iter()
                .filter(|e| e.kind == EntryKind::File && is_mod_file(&e.name))
            {
                let path = mods_dir().join(&entry.name)?;
                let info = files
                    .open_read(&path)
                    .ok()
                    .and_then(|(file, _)| read_mod_info(std::io::BufReader::new(file)))
                    .unwrap_or_default();
                mods.push(pb::InstalledMod {
                    enabled: !entry.name.ends_with(DISABLED),
                    size: entry.size,
                    modified_ms: entry.modified_ms,
                    name: info.name.unwrap_or_default(),
                    version: info.version.unwrap_or_default(),
                    description: info.description.unwrap_or_default(),
                    authors: info.authors,
                    loader: info.loader.unwrap_or_default(),
                    has_icon: info.icon.is_some(),
                    environment: info.environment.unwrap_or_default(),
                    filename: entry.name,
                    ..Default::default()
                });
            }
            let mut manual_downloads = Vec::new();
            if curseforge {
                let needs = RelPath::parse(manual::NEEDS_DOWNLOAD_FILE)?;
                if let Ok(content) = files.read(&needs, 256 * 1024) {
                    for f in manual::parse(&String::from_utf8_lossy(&content.data)) {
                        let present = ["", "mods/"].iter().any(|sub| {
                            RelPath::parse(&format!(
                                "{}/{sub}{}",
                                nn_orchestrator::spec::CURSEFORGE_DOWNLOADS_DIR,
                                f.filename
                            ))
                            .ok()
                            .and_then(|p| files.stat(&p).ok())
                            .is_some_and(|e| e.kind == EntryKind::File)
                        });
                        manual_downloads.push(pb::ManualDownload {
                            r#mod: f.mod_name,
                            version_name: f.version_name,
                            filename: f.filename,
                            download_page: f.download_page,
                            present,
                            ..Default::default()
                        });
                    }
                }
            }
            Ok((mods, manual_downloads))
        })
        .await?;
        Response::ok(pb::ListModsResponse {
            supported: true,
            loader,
            mc_version: server.mc_version,
            mods,
            downloads_folder: if curseforge {
                format!("{}/mods", nn_orchestrator::spec::CURSEFORGE_DOWNLOADS_DIR)
            } else {
                String::new()
            },
            manual_downloads,
            ..Default::default()
        })
    }

    async fn check_mod_updates(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CheckModUpdatesRequest>,
    ) -> ServiceResult<pb::CheckModUpdatesResponse> {
        require(&ctx, Permission::ManageServers)?;
        let (server, files, loader) = mod_server(&self.state, request.server_id).await?;
        let hashes: Vec<(String, String)> = blocking(move || {
            let entries = match files.list(&mods_dir()) {
                Ok(entries) => entries,
                Err(FileError::NotFound(_)) => Vec::new(),
                Err(e) => return Err(e),
            };
            Ok(entries
                .iter()
                .filter(|e| e.kind == EntryKind::File && is_mod_file(&e.name))
                .filter_map(|e| Some((sha1_of(&files, e)?, e.name.clone())))
                .collect())
        })
        .await?;
        let catalog = &self.state.content.catalog;
        let sha1s: Vec<String> = hashes.iter().map(|(h, _)| h.clone()).collect();
        let known = catalog
            .modrinth_identify(&sha1s)
            .await
            .map_err(catalog_error)?;
        let latest = if loader.is_empty() {
            HashMap::new()
        } else {
            catalog
                .modrinth_updates(&sha1s, &server.mc_version, &loader)
                .await
                .map_err(catalog_error)?
        };
        let project_ids: Vec<String> = {
            let mut ids: Vec<String> = known.values().map(|v| v.project_id.clone()).collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let projects: HashMap<String, nn_catalog::Project> = catalog
            .modrinth_projects(&project_ids)
            .await
            .map_err(catalog_error)?
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let mut mods = Vec::new();
        for (hash, filename) in hashes {
            let Some(current) = known.get(&hash) else {
                continue;
            };
            let Some(project) = projects.get(&current.project_id) else {
                continue;
            };
            let newer = latest
                .get(&hash)
                .filter(|v| v.id != current.id && v.published_ms > current.published_ms)
                .cloned();
            mods.push(pb::ModUpdate {
                filename,
                project: proto_project(project.clone()).into(),
                latest: newer.map(proto_version).into(),
                ..Default::default()
            });
        }
        Response::ok(pb::CheckModUpdatesResponse {
            mods,
            ..Default::default()
        })
    }

    async fn install_mod(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::InstallModRequest>,
    ) -> ServiceResult<pb::InstallModResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (server, files, loader) = mod_server(&self.state, request.server_id).await?;
        let platform = platform(request.platform.as_known())?;
        let version = Some(request.version_id.trim()).filter(|v| !v.is_empty());
        let plan = self
            .plan(
                platform,
                request.project_id.trim(),
                version,
                &server.mc_version,
                &loader,
            )
            .await?;
        if let Some(file) = plan[0].primary_file()
            && file.url.is_none()
        {
            return Err(ConnectError::failed_precondition(format!(
                "{}'s author doesn't allow other apps to download it. Download it from CurseForge and upload it into mods/.",
                file.filename
            )));
        }
        let names: Vec<String> = plan
            .iter()
            .filter_map(|v| v.primary_file().map(|f| f.filename.clone()))
            .collect();
        let job_id = self.start_install(&server, files, plan, None);
        self.record(
            &user,
            "mod.install",
            &server.id,
            json!({ "project": request.project_id, "files": names }),
        )
        .await;
        Response::ok(pb::InstallModResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn update_mod(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateModRequest>,
    ) -> ServiceResult<pb::UpdateModResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (server, files, _) = mod_server(&self.state, request.server_id).await?;
        let old = mod_path(request.filename)?;
        let version = self
            .state
            .content
            .catalog
            .modrinth_version(request.version_id.trim())
            .await
            .map_err(catalog_error)?;
        let job_id = self.start_install(&server, files, vec![version.clone()], Some(old));
        self.record(
            &user,
            "mod.update",
            &server.id,
            json!({ "file": request.filename, "version": version.version_number }),
        )
        .await;
        Response::ok(pb::UpdateModResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn set_mod_enabled(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetModEnabledRequest>,
    ) -> ServiceResult<pb::SetModEnabledResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (server, files, _) = mod_server(&self.state, request.server_id).await?;
        let from = mod_path(request.filename)?;
        let base = request
            .filename
            .trim()
            .trim_end_matches(DISABLED)
            .to_owned();
        let target = if request.enabled {
            base
        } else {
            format!("{base}{DISABLED}")
        };
        if target == request.filename.trim() {
            return Response::ok(pb::SetModEnabledResponse {
                filename: target,
                ..Default::default()
            });
        }
        let to = mod_path(&target)?;
        blocking(move || files.rename(&from, &to)).await?;
        self.record(
            &user,
            if request.enabled {
                "mod.enable"
            } else {
                "mod.disable"
            },
            &server.id,
            json!({ "file": target }),
        )
        .await;
        Response::ok(pb::SetModEnabledResponse {
            filename: target,
            ..Default::default()
        })
    }

    async fn delete_mod(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteModRequest>,
    ) -> ServiceResult<pb::DeleteModResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let (server, files, _) = mod_server(&self.state, request.server_id).await?;
        let path = mod_path(request.filename)?;
        blocking(move || files.delete(&path)).await?;
        self.record(
            &user,
            "mod.delete",
            &server.id,
            json!({ "file": request.filename }),
        )
        .await;
        Response::ok(pb::DeleteModResponse::default())
    }
}
