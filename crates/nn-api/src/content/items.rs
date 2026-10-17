//! Minecraft item icons for inventories (see `nn_catalog::items`).
//!
//! - `GET /api/servers/{id}/items?ids=minecraft:stone,…`: how to draw each
//!   item in that server's Minecraft version.
//! - `GET /api/minecraft/{version}/textures/{path}.png`: a texture.
//! - `GET /api/servers/{id}/mods/textures/{namespace}/{path}.png`: a texture
//!   from one of the server's mods.
//!
//! Items from mods are drawn from the mod's own file: the panel finds which
//! file in the server's `mods` folder has the item's namespace (from mod
//! IDs) and keeps a copy of it in its cache.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::files_http::authorize;
use crate::middleware::CurrentUser;

/// Items asked about at once, at most.
const MAX_IDS: usize = 200;
/// How long the list of a server's mods is trusted.
const INDEX_FRESH: Duration = Duration::from_secs(10 * 60);
/// Mod files looked at, at most.
const MAX_MODS: usize = 1000;
/// Largest mod file copied.
const MAX_MOD_FILE: u64 = 256 * 1024 * 1024;

/// A mod file in a server's `mods` folder.
#[derive(Debug, Clone)]
struct ModFile {
    path: String,
    size: u64,
    modified_ms: i64,
}

type Index = Arc<HashMap<String, ModFile>>;

/// Which mod file has each namespace, per server, and copies of the files.
#[derive(Default)]
pub(crate) struct ModFiles {
    index: Mutex<HashMap<String, (Instant, Index)>>,
    /// Open copies, and when each was last used.
    jars: Mutex<HashMap<String, (Instant, nn_catalog::items::Jar)>>,
    building: tokio::sync::Mutex<()>,
}

impl ModFiles {
    /// Namespaces (mod IDs) to the files that have them.
    async fn index(&self, state: &AppState, server: &nn_store::Server) -> Index {
        let fresh = |me: &Self| {
            me.index
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&server.id)
                .filter(|(at, _)| at.elapsed() < INDEX_FRESH)
                .map(|(_, index)| index.clone())
        };
        if let Some(index) = fresh(self) {
            return index;
        }
        let _one = self.building.lock().await;
        if let Some(index) = fresh(self) {
            return index;
        }
        let mut map = HashMap::new();
        if let Ok(folder) = state.nodes.for_server(server).folder(server).await
            && let Ok(mods) = nn_files::RelPath::parse("mods")
        {
            let entries = folder.list(&mods).await.unwrap_or_default();
            for entry in entries
                .into_iter()
                .filter(|e| e.kind == nn_files::EntryKind::File && e.name.ends_with(".jar"))
                .take(MAX_MODS)
            {
                let path = format!("mods/{}", entry.name);
                let Ok(rel) = nn_files::RelPath::parse(&path) else {
                    continue;
                };
                if let Ok(Some(info)) = folder.mod_info(&rel).await
                    && let Some(id) = info.id
                {
                    map.entry(id.to_ascii_lowercase()).or_insert(ModFile {
                        path,
                        size: entry.size,
                        modified_ms: entry.modified_ms,
                    });
                }
            }
        }
        let index = Arc::new(map);
        self.index
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(server.id.clone(), (Instant::now(), index.clone()));
        index
    }

    /// The mod file with `namespace`, copied into the panel's cache.
    async fn jar(
        &self,
        state: &AppState,
        server: &nn_store::Server,
        namespace: &str,
    ) -> Option<nn_catalog::items::Jar> {
        use sha2::Digest as _;
        use tokio::io::AsyncReadExt as _;
        let file = self.index(state, server).await.get(namespace)?.clone();
        let key = hex::encode(
            &sha2::Sha256::digest(format!(
                "{}\0{}\0{}\0{}",
                server.id, file.path, file.size, file.modified_ms
            ))[..16],
        );
        if let Some((used, jar)) = self
            .jars
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&key)
        {
            *used = Instant::now();
            return Some(jar.clone());
        }
        if file.size > MAX_MOD_FILE {
            return None;
        }
        let cached = state
            .content
            .cache_dir()
            .join("mods")
            .join(format!("{key}.jar"));
        if !cached.exists() {
            let folder = state.nodes.for_server(server).folder(server).await.ok()?;
            let rel = nn_files::RelPath::parse(&file.path).ok()?;
            let mut reader = folder
                .open_read(&rel)
                .await
                .ok()?
                .from_offset(0)
                .await
                .ok()?;
            let mut bytes = Vec::new();
            (&mut reader)
                .take(MAX_MOD_FILE)
                .read_to_end(&mut bytes)
                .await
                .ok()?;
            let target = cached.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                if let Some(dir) = target.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let part = target.with_extension("jar.part");
                std::fs::write(&part, &bytes)?;
                std::fs::rename(&part, &target)
            })
            .await
            .ok()?
            .ok()?;
        }
        let jar = nn_catalog::items::open_jar(&cached).ok()?;
        self.jars
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, (Instant::now(), jar.clone()));
        Some(jar)
    }

    /// Copies in `dir` not used for `unused` (going by when they were
    /// copied, if not opened since NetherNode started). They're forgotten
    /// here; the caller deletes them.
    pub(crate) fn prunable(&self, dir: &std::path::Path, unused: Duration) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut jars = self.jars.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(key) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            let recent = match jars.get(&key) {
                Some((used, _)) => used.elapsed() < unused,
                None => nn_catalog::items::modified_within(&path, unused),
            };
            if !recent {
                jars.remove(&key);
                out.push(path);
            }
        }
        out
    }

    /// Forgets every copy and mod list.
    pub(crate) fn forget_all(&self) {
        self.jars.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.index.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[derive(Deserialize)]
pub(crate) struct IconQuery {
    ids: String,
}

#[derive(Serialize)]
struct Icons {
    /// The Minecraft version the textures come from.
    version: String,
    icons: HashMap<String, nn_catalog::items::Icon>,
}

fn failed(e: &nn_catalog::Error) -> Response {
    let status = match e {
        nn_catalog::Error::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_GATEWAY,
    };
    (status, e.to_string()).into_response()
}

/// How to draw the items in a server's inventories.
pub(crate) async fn icons(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Query(query): Query<IconQuery>,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let server = match state.auth.store().find_server(&id).await {
        Ok(Some(server)) => server,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let ids: Vec<String> = query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .take(MAX_IDS)
        .map(str::to_owned)
        .collect();
    let content = &state.content;
    let mut namespaces: Vec<&str> = ids
        .iter()
        .filter_map(|id| id.split_once(':').map(|(ns, _)| ns))
        .filter(|ns| *ns != "minecraft")
        .collect();
    namespaces.sort_unstable();
    namespaces.dedup();
    let mut mods = Vec::new();
    for namespace in namespaces {
        if let Some(jar) = content.mod_files.jar(&state, &server, namespace).await {
            mods.push(jar);
        }
    }
    let version = server.mc_version.clone();
    match content
        .item_textures
        .icons(&content.catalog, &version, ids, mods)
        .await
    {
        Ok((version, icons)) => Json(Icons { version, icons }).into_response(),
        Err(e) => failed(&e),
    }
}

/// One texture.
pub(crate) async fn texture(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path((version, path)): Path<(String, String)>,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let Some(path) = path.strip_suffix(".png") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content = &state.content;
    match content
        .item_textures
        .texture(&content.catalog, &version, path)
        .await
    {
        Ok(png) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                // A version's textures never change.
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable",
                ),
            ],
            png,
        )
            .into_response(),
        Err(e) => failed(&e),
    }
}

/// One texture from one of a server's mods.
pub(crate) async fn mod_texture(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path((id, namespace, path)): Path<(String, String, String)>,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let Some(path) = path.strip_suffix(".png") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let server = match state.auth.store().find_server(&id).await {
        Ok(Some(server)) => server,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let Some(jar) = state
        .content
        .mod_files
        .jar(&state, &server, &namespace)
        .await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match nn_catalog::items::mod_texture(jar, &namespace, path).await {
        Ok(png) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            png,
        )
            .into_response(),
        Err(e) => failed(&e),
    }
}
