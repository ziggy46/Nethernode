//! Turning "this modpack, this version" into what a server needs: the
//! pack's details, its Minecraft version, and its mod loader.

use connectrpc::ConnectError;
use nn_catalog::content::MOD_LOADERS;
use nn_catalog::{ProjectKind, ProjectVersion, compare_versions};
use nn_orchestrator::spec::{Modpack, ServerPack};

use super::{Content, catalog_error};

/// A resolved pack: what to store, and the Minecraft version it runs.
pub(crate) struct Resolved {
    pub modpack: Modpack,
    pub mc_version: String,
}

pub(crate) fn newest_game_version(version: &ProjectVersion) -> Option<String> {
    version
        .game_versions
        .iter()
        .max_by(|a, b| compare_versions(a, b))
        .cloned()
}

fn mod_loader(version: &ProjectVersion) -> Option<String> {
    version
        .loaders
        .iter()
        .find(|l| MOD_LOADERS.contains(&l.as_str()))
        .cloned()
}

/// The version to install: the one asked for, or the newest release.
fn pick(versions: Vec<ProjectVersion>) -> Option<ProjectVersion> {
    let release = versions.iter().position(|v| v.channel == "release");
    let index = release.unwrap_or(0);
    versions.into_iter().nth(index)
}

impl Content {
    /// Looks up a Modrinth pack (by ID or slug) and version (or the newest
    /// release).
    pub(crate) async fn resolve_modrinth(
        &self,
        project: &str,
        version_id: Option<&str>,
    ) -> Result<Resolved, ConnectError> {
        let catalog = &self.catalog;
        let project = catalog
            .modrinth_project(project)
            .await
            .map_err(catalog_error)?;
        let version = match version_id {
            Some(id) => {
                let v = catalog.modrinth_version(id).await.map_err(catalog_error)?;
                if v.project_id != project.id {
                    return Err(ConnectError::invalid_argument(
                        "that version belongs to another project",
                    ));
                }
                v
            }
            None => pick(
                catalog
                    .modrinth_versions(&project.id, None, None)
                    .await
                    .map_err(catalog_error)?,
            )
            .ok_or_else(|| {
                ConnectError::not_found(format!("{} has no versions to install", project.title))
            })?,
        };
        let mc_version = newest_game_version(&version).ok_or_else(|| {
            ConnectError::failed_precondition(
                "this pack version doesn't say which Minecraft version it's for",
            )
        })?;
        Ok(Resolved {
            mc_version,
            modpack: Modpack {
                platform: "modrinth".into(),
                project_id: project.id,
                slug: project.slug,
                title: project.title,
                loader: mod_loader(&version),
                version_name: Some(version.version_number.clone()),
                version_id: Some(version.id),
                icon_url: project.icon_url,
                page_url: Some(project.page_url),
                ..Default::default()
            },
        })
    }

    /// Looks up a CurseForge pack (by numeric ID or slug). Without an API
    /// key nothing can be looked up: the slug is used as-is and the
    /// Minecraft version must be given.
    pub(crate) async fn resolve_curseforge(
        &self,
        project: &str,
        version_id: Option<&str>,
        mc_version: Option<&str>,
    ) -> Result<Resolved, ConnectError> {
        let Some(key) = self.curseforge_key() else {
            if project.chars().all(|c| c.is_ascii_digit()) {
                return Err(ConnectError::invalid_argument(
                    "without a CurseForge API key, give the pack's slug (from its page URL), not its number",
                ));
            }
            if !project
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || project.is_empty()
            {
                return Err(ConnectError::invalid_argument(
                    "that isn't a CurseForge modpack slug",
                ));
            }
            let mc_version = mc_version
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    ConnectError::invalid_argument(
                        "choose the pack's Minecraft version (or add a CurseForge API key so NetherNode can look it up)",
                    )
                })?
                .to_owned();
            return Ok(Resolved {
                mc_version,
                modpack: Modpack {
                    platform: "curseforge".into(),
                    project_id: String::new(),
                    slug: project.to_owned(),
                    title: project.replace(['-', '_'], " "),
                    version_id: version_id.map(str::to_owned),
                    version_name: None,
                    loader: None,
                    icon_url: None,
                    page_url: Some(format!(
                        "https://www.curseforge.com/minecraft/modpacks/{project}"
                    )),
                    no_server_pack: Some(
                        "NetherNode needs a CurseForge API key to find the server pack.".into(),
                    ),
                    ..Default::default()
                },
            });
        };
        let catalog = &self.catalog;
        let found = if project.chars().all(|c| c.is_ascii_digit()) {
            catalog.curseforge_project(&key, project).await
        } else {
            catalog
                .curseforge_by_slug(&key, ProjectKind::Modpack, project)
                .await
        }
        .map_err(catalog_error)?;
        let version = match version_id {
            Some(file) => catalog
                .curseforge_version(&key, &found.id, file)
                .await
                .map_err(catalog_error)?,
            None => pick(
                catalog
                    .curseforge_versions(&key, &found.id, None, None)
                    .await
                    .map_err(catalog_error)?,
            )
            .ok_or_else(|| {
                ConnectError::not_found(format!("{} has no files to install", found.title))
            })?,
        };
        let mut mc_version = newest_game_version(&version).ok_or_else(|| {
            ConnectError::failed_precondition(
                "this pack file doesn't say which Minecraft version it's for",
            )
        })?;
        let mut modpack = Modpack {
            platform: "curseforge".into(),
            project_id: found.id,
            slug: found.slug,
            title: found.title,
            loader: mod_loader(&version),
            version_name: Some(version.name.clone()),
            version_id: Some(version.id.clone()),
            icon_url: found.icon_url,
            page_url: Some(found.page_url),
            ..Default::default()
        };
        // Servers are installed from the author's server pack when there is
        // one; otherwise from the modpack, leaving out what the image knows
        // only runs in the game.
        match self.server_pack(&key, &modpack.project_id, &version).await {
            Ok((pack, manifest)) => {
                mc_version = manifest.mc_version;
                modpack.loader = Some(manifest.loader);
                modpack.loader_version = Some(manifest.loader_version);
                modpack.server_pack = Some(pack);
            }
            Err(why) => {
                tracing::info!(pack = %modpack.slug, file = %version.id, %why, "installing from the modpack, not a server pack");
                modpack.no_server_pack = Some(why);
            }
        }
        Ok(Resolved {
            mc_version,
            modpack,
        })
    }

    /// The server pack published with a CurseForge pack file, and what the
    /// pack's manifest says it runs on (the server pack's own files don't
    /// say reliably). Errors say why there's none to use.
    async fn server_pack(
        &self,
        key: &str,
        project: &str,
        version: &ProjectVersion,
    ) -> Result<(ServerPack, nn_catalog::PackManifest), String> {
        let catalog = &self.catalog;
        let id = version
            .server_pack_id
            .as_deref()
            .ok_or("The author doesn't publish a server pack for this version.")?;
        let file = catalog
            .curseforge_version(key, project, id)
            .await
            .map_err(|e| format!("The server pack couldn't be looked up: {e}"))?;
        let file = file
            .files
            .into_iter()
            .next()
            .ok_or("The server pack has no file.")?;
        let url = file
            .url
            .ok_or("The author doesn't allow other apps to download the server pack.")?;
        let modpack = version
            .files
            .first()
            .ok_or("The modpack has no file to read its manifest from.")?;
        let zip = catalog
            .download(modpack, MAX_MODPACK_ZIP)
            .await
            .map_err(|e| format!("The modpack couldn't be read: {e}"))?;
        let manifest = nn_catalog::pack_manifest(&zip)
            .map_err(|e| format!("{}.", sentence_case(&e.to_string())))?;
        Ok((
            ServerPack {
                file_id: id.to_owned(),
                file_name: file.filename,
                url,
            },
            manifest,
        ))
    }
}

/// The largest modpack zip read for its manifest (they're mostly a list of
/// mods plus configs).
const MAX_MODPACK_ZIP: u64 = 256 * 1024 * 1024;

fn sentence_case(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
