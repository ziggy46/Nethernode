//! Item icons from Minecraft itself. The textures are Mojang's, so
//! NetherNode doesn't ship them: when someone looks at an inventory, the
//! panel downloads that version's client from Mojang (as launchers do),
//! checks it, keeps it in its cache folder, and reads textures from it.
//!
//! Flat items (tools, food, ingots, …) use their item texture. Blocks are
//! shown as a cube from their top and side textures, like the game's
//! inventory does; odd shapes (stairs, chests) get the closest cube.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Catalog, Error};

/// Largest client jar accepted.
const MAX_JAR: u64 = 200 * 1024 * 1024;
/// Hosts Mojang serves version files and clients from.
const MOJANG_HOSTS: &[&str] = &["piston-meta.mojang.com", "piston-data.mojang.com"];

/// How to draw an item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Icon {
    /// One texture, drawn flat.
    Flat {
        /// Texture path, like `item/diamond_sword`.
        texture: String,
        /// A colour to multiply it by (grass and leaves are grey until the
        /// game tints them), like `#79c05a`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tint: Option<String>,
    },
    /// A block, drawn as a cube.
    Cube {
        /// Top face texture.
        top: String,
        /// Side faces texture.
        side: String,
        /// A colour for the top face, as for [`Icon::Flat`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tint: Option<String>,
    },
}

/// The colours the game gives grey foliage textures in inventories.
fn tint_for(texture: &str) -> Option<String> {
    let name = texture.rsplit('/').next().unwrap_or(texture);
    let colour = match name {
        "spruce_leaves" => "#619961",
        "birch_leaves" => "#80a755",
        "grass_block_top" | "short_grass" | "grass" | "tall_grass_top" | "tall_grass_bottom"
        | "fern" | "large_fern_top" | "large_fern_bottom" | "vine" | "lily_pad" | "oak_leaves"
        | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "mangrove_leaves" => "#79c05a",
        _ => return None,
    };
    Some(colour.to_owned())
}

type Jar = Arc<Mutex<zip::ZipArchive<std::fs::File>>>;

/// Minecraft clients downloaded for their textures, by version.
pub struct ItemTextures {
    dir: PathBuf,
    jars: Mutex<HashMap<String, Jar>>,
    fetching: tokio::sync::Mutex<()>,
}

impl ItemTextures {
    /// Keeps clients under `dir` (one folder per version).
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            jars: Mutex::default(),
            fetching: tokio::sync::Mutex::new(()),
        }
    }

    /// How to draw each of `ids` (`minecraft:diamond_sword`) in `version`,
    /// and the concrete version the textures come from (`LATEST` resolves).
    /// Items it can't work out (other namespaces, unusual models) are left
    /// out.
    pub async fn icons(
        &self,
        catalog: &Catalog,
        version: &str,
        ids: Vec<String>,
    ) -> Result<(String, HashMap<String, Icon>), Error> {
        let version = catalog.release_id(version).await?;
        let jar = self.jar(catalog, &version).await?;
        let icons = tokio::task::spawn_blocking(move || {
            let mut zip = jar.lock().unwrap_or_else(|e| e.into_inner());
            ids.into_iter()
                .filter_map(|id| resolve(&mut zip, &id).map(|icon| (id, icon)))
                .collect()
        })
        .await
        .map_err(|e| Error::Upstream(e.to_string()))?;
        Ok((version, icons))
    }

    /// A texture's PNG, like `item/diamond_sword`.
    pub async fn texture(
        &self,
        catalog: &Catalog,
        version: &str,
        path: &str,
    ) -> Result<Vec<u8>, Error> {
        if !valid_path(path) {
            return Err(Error::NotFound("no such texture".into()));
        }
        let jar = self.jar(catalog, version).await?;
        let entry = format!("assets/minecraft/textures/{path}.png");
        tokio::task::spawn_blocking(move || {
            let mut zip = jar.lock().unwrap_or_else(|e| e.into_inner());
            read(&mut zip, &entry, 1024 * 1024)
                .ok_or_else(|| Error::NotFound("no such texture".into()))
        })
        .await
        .map_err(|e| Error::Upstream(e.to_string()))?
    }

    async fn jar(&self, catalog: &Catalog, version: &str) -> Result<Jar, Error> {
        let version = catalog.release_id(version).await?;
        if let Some(jar) = self.cached(&version) {
            return Ok(jar);
        }
        let _one = self.fetching.lock().await;
        if let Some(jar) = self.cached(&version) {
            return Ok(jar);
        }
        let file = self.dir.join(&version).join("client.jar");
        if !file.exists() {
            catalog.download_client(&version, &file).await?;
        }
        let opened = std::fs::File::open(&file)
            .map_err(|e| Error::Upstream(format!("couldn't open the Minecraft client: {e}")))?;
        let zip = zip::ZipArchive::new(opened)
            .map_err(|e| Error::Upstream(format!("the Minecraft client is damaged: {e}")))?;
        let jar: Jar = Arc::new(Mutex::new(zip));
        self.jars
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(version, jar.clone());
        Ok(jar)
    }

    fn cached(&self, version: &str) -> Option<Jar> {
        self.jars
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(version)
            .cloned()
    }
}

impl Catalog {
    /// A concrete version: `LATEST` and `SNAPSHOT` become the newest of each.
    async fn release_id(&self, version: &str) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Latest {
            release: String,
            snapshot: String,
        }
        #[derive(Deserialize)]
        struct Manifest {
            latest: Latest,
        }
        match version.to_ascii_uppercase().as_str() {
            "LATEST" | "SNAPSHOT" => {
                let raw = self.cached_text("mojang", &self.endpoints.mojang).await?;
                let manifest: Manifest = crate::versions::parse_json(&raw)?;
                Ok(if version.eq_ignore_ascii_case("snapshot") {
                    manifest.latest.snapshot
                } else {
                    manifest.latest.release
                })
            }
            _ if valid_version(version) => Ok(version.to_owned()),
            _ => Err(Error::NotFound(format!(
                "unknown Minecraft version {version}"
            ))),
        }
    }

    /// Downloads the client of `version` from Mojang to `file`, checking
    /// its SHA-1.
    async fn download_client(&self, version: &str, file: &Path) -> Result<(), Error> {
        use sha1::Digest as _;
        #[derive(Deserialize)]
        struct Manifest {
            versions: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            id: String,
            url: String,
        }
        #[derive(Deserialize)]
        struct VersionFile {
            downloads: Downloads,
        }
        #[derive(Deserialize)]
        struct Downloads {
            client: Download,
        }
        #[derive(Deserialize)]
        struct Download {
            sha1: String,
            size: u64,
            url: String,
        }
        let raw = self.cached_text("mojang", &self.endpoints.mojang).await?;
        let manifest: Manifest = crate::versions::parse_json(&raw)?;
        let entry = manifest
            .versions
            .into_iter()
            .find(|v| v.id == version)
            .ok_or_else(|| Error::NotFound(format!("Mojang doesn't list Minecraft {version}")))?;
        mojang_url(&entry.url)?;
        let raw = self
            .cached_text(&format!("mojang-{version}"), &entry.url)
            .await?;
        let client = crate::versions::parse_json::<VersionFile>(&raw)?
            .downloads
            .client;
        mojang_url(&client.url)?;
        if client.size > MAX_JAR {
            return Err(Error::Upstream(
                "the Minecraft client is unexpectedly large".into(),
            ));
        }
        tracing::info!(%version, "downloading the Minecraft client for item icons");
        let mut response = self
            .http
            .get(&client.url)
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| {
                Error::Upstream(format!(
                    "couldn't download the Minecraft client: {}",
                    e.without_url()
                ))
            })?;
        let mut bytes = Vec::with_capacity(usize::try_from(client.size).unwrap_or(0));
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            Error::Upstream(format!(
                "couldn't download the Minecraft client: {}",
                e.without_url()
            ))
        })? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() as u64 > MAX_JAR {
                return Err(Error::Upstream(
                    "the Minecraft client is unexpectedly large".into(),
                ));
            }
        }
        if hex::encode(sha1::Sha1::digest(&bytes)) != client.sha1.to_ascii_lowercase() {
            return Err(Error::Upstream(
                "the Minecraft client didn't match its checksum".into(),
            ));
        }
        let (file, bytes) = (file.to_owned(), bytes);
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let part = file.with_extension("jar.part");
            std::fs::write(&part, &bytes)?;
            std::fs::rename(&part, &file)
        })
        .await
        .map_err(|e| Error::Upstream(e.to_string()))?
        .map_err(|e| Error::Upstream(format!("couldn't save the Minecraft client: {e}")))
    }
}

fn mojang_url(url: &str) -> Result<(), Error> {
    let parsed = reqwest::Url::parse(url).map_err(|_| Error::Upstream("bad Mojang link".into()))?;
    if parsed.scheme() == "https" && parsed.host_str().is_some_and(|h| MOJANG_HOSTS.contains(&h)) {
        Ok(())
    } else {
        Err(Error::Upstream(
            "Mojang's version list points somewhere unexpected".into(),
        ))
    }
}

fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b' '))
}

/// A texture or model path: `item/diamond_sword`, `block/oak_log_top`.
fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 128
        && !p.contains("..")
        && p.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'/' | b'-' | b'.')
        })
}

fn read(zip: &mut zip::ZipArchive<std::fs::File>, name: &str, max: u64) -> Option<Vec<u8>> {
    let entry = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    entry.take(max).read_to_end(&mut out).ok()?;
    Some(out)
}

fn read_json(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<Value> {
    serde_json::from_slice(&read(zip, name, 256 * 1024)?).ok()
}

fn exists(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> bool {
    zip.by_name(name).is_ok()
}

/// `minecraft:block/stone` → `block/stone`.
fn bare(reference: &str) -> &str {
    reference.strip_prefix("minecraft:").unwrap_or(reference)
}

fn resolve(zip: &mut zip::ZipArchive<std::fs::File>, id: &str) -> Option<Icon> {
    let (namespace, name) = id.split_once(':').unwrap_or(("minecraft", id));
    if namespace != "minecraft" || !valid_path(name) || name.contains('/') {
        return None;
    }
    let flat = format!("item/{name}");
    if exists(zip, &format!("assets/minecraft/textures/{flat}.png")) {
        return Some(Icon::Flat {
            tint: tint_for(&flat),
            texture: flat,
        });
    }
    // 1.21.4 and later describe items in items/; before, models/item/.
    let model = read_json(zip, &format!("assets/minecraft/items/{name}.json"))
        .and_then(|item| model_reference(&item))
        .or_else(|| {
            let path = format!("item/{name}");
            exists(zip, &format!("assets/minecraft/models/{path}.json")).then_some(path)
        })?;
    model_icon(zip, bare(&model))
}

/// The first plain model an item definition points to.
fn model_reference(definition: &Value) -> Option<String> {
    match definition {
        Value::Object(map) => {
            if let (Some(Value::String(kind)), Some(Value::String(model))) =
                (map.get("type"), map.get("model"))
                && bare(kind) == "model"
            {
                return Some(model.clone());
            }
            map.values().find_map(model_reference)
        }
        Value::Array(items) => items.iter().find_map(model_reference),
        _ => None,
    }
}

fn model_icon(zip: &mut zip::ZipArchive<std::fs::File>, model: &str) -> Option<Icon> {
    let mut textures: HashMap<String, String> = HashMap::new();
    let mut current = Some(model.to_owned());
    let mut generated = false;
    for _ in 0..12 {
        let Some(path) = current.take() else { break };
        if path.ends_with("generated") || path.starts_with("builtin/") {
            generated = path.ends_with("generated");
            break;
        }
        if !valid_path(&path) {
            break;
        }
        let Some(json) = read_json(zip, &format!("assets/minecraft/models/{path}.json")) else {
            break;
        };
        if let Some(Value::Object(own)) = json.get("textures") {
            for (key, value) in own {
                if let Value::String(value) = value {
                    textures.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        current = json
            .get("parent")
            .and_then(Value::as_str)
            .map(|p| bare(p).to_owned());
    }
    let lookup = |key: &str| -> Option<String> {
        let mut value = textures.get(key)?.clone();
        for _ in 0..8 {
            match value.strip_prefix('#') {
                Some(next) => value = textures.get(next)?.clone(),
                None => return Some(bare(&value).to_owned()),
            }
        }
        None
    };
    let pick = |keys: &[&str]| keys.iter().find_map(|k| lookup(k));
    let texture_ok = |zip: &mut zip::ZipArchive<std::fs::File>, t: &str| {
        valid_path(t) && exists(zip, &format!("assets/minecraft/textures/{t}.png"))
    };
    if generated || textures.contains_key("layer0") {
        let texture = lookup("layer0")?;
        return texture_ok(zip, &texture).then(|| Icon::Flat {
            tint: tint_for(&texture),
            texture,
        });
    }
    let top = pick(&["top", "end", "up", "all", "texture", "particle"]);
    let side = pick(&["side", "front", "north", "all", "texture", "particle"]);
    match (top, side) {
        (Some(top), Some(side)) if texture_ok(zip, &top) && texture_ok(zip, &side) => {
            Some(Icon::Cube {
                tint: tint_for(&top),
                top,
                side,
            })
        }
        _ => {
            let texture = lookup("particle")?;
            texture_ok(zip, &texture).then(|| Icon::Flat {
                tint: tint_for(&texture),
                texture,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// A tiny "client jar" with a few textures, models, and item definitions.
    fn jar(dir: &Path) -> zip::ZipArchive<std::fs::File> {
        let path = dir.join("client.jar");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        let mut add = |name: &str, body: &[u8]| {
            zip.start_file(name, options).unwrap();
            zip.write_all(body).unwrap();
        };
        let png = b"\x89PNG fake";
        add("assets/minecraft/textures/item/diamond_sword.png", png);
        add("assets/minecraft/textures/block/stone.png", png);
        add("assets/minecraft/textures/block/oak_log.png", png);
        add("assets/minecraft/textures/block/oak_log_top.png", png);
        add("assets/minecraft/textures/block/chest.png", png);
        add(
            "assets/minecraft/items/stone.json",
            br#"{"model":{"type":"minecraft:model","model":"minecraft:block/stone"}}"#,
        );
        add(
            "assets/minecraft/models/block/stone.json",
            br#"{"parent":"minecraft:block/cube_all","textures":{"all":"minecraft:block/stone"}}"#,
        );
        add(
            "assets/minecraft/models/block/cube_all.json",
            br##"{"parent":"block/cube","textures":{"particle":"#all"}}"##,
        );
        add(
            "assets/minecraft/models/block/cube.json",
            br#"{"elements":[]}"#,
        );
        add(
            "assets/minecraft/models/item/oak_log.json",
            br#"{"parent":"minecraft:block/oak_log"}"#,
        );
        add("assets/minecraft/models/block/oak_log.json", br#"{"parent":"block/cube_column","textures":{"end":"block/oak_log_top","side":"block/oak_log"}}"#);
        add("assets/minecraft/items/chest.json", br#"{"model":{"type":"minecraft:special","base":"minecraft:item/chest","model":{"type":"minecraft:chest"}}}"#);
        add(
            "assets/minecraft/models/item/chest.json",
            br#"{"parent":"builtin/entity","textures":{"particle":"block/chest"}}"#,
        );
        zip.finish().unwrap();
        zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap()
    }

    #[test]
    fn items_resolve_to_flat_textures_or_cubes() {
        let dir = tempfile::tempdir().unwrap();
        let mut zip = jar(dir.path());
        assert_eq!(
            resolve(&mut zip, "minecraft:diamond_sword"),
            Some(Icon::Flat {
                texture: "item/diamond_sword".into(),
                tint: None
            })
        );
        assert_eq!(
            resolve(&mut zip, "minecraft:stone"),
            Some(Icon::Cube {
                top: "block/stone".into(),
                side: "block/stone".into(),
                tint: None
            })
        );
        assert_eq!(
            resolve(&mut zip, "oak_log"),
            Some(Icon::Cube {
                top: "block/oak_log_top".into(),
                side: "block/oak_log".into(),
                tint: None
            })
        );
        assert_eq!(resolve(&mut zip, "minecraft:nothing_here"), None);
        assert_eq!(resolve(&mut zip, "create:wrench"), None);
        assert_eq!(resolve(&mut zip, "minecraft:../secrets"), None);
    }

    #[test]
    fn paths_and_versions_are_checked() {
        assert!(valid_path("item/diamond_sword"));
        assert!(!valid_path("../../etc/passwd"));
        assert!(!valid_path("item/Thing"));
        assert!(valid_version("1.21.4"));
        assert!(valid_version("26.4-snapshot-4"));
        assert!(!valid_version("1.21/../x"));
        assert!(mojang_url("https://piston-data.mojang.com/v1/objects/abc/client.jar").is_ok());
        assert!(mojang_url("https://evil.example/client.jar").is_err());
    }
}
