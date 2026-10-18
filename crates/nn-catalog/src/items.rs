//! Item icons from Minecraft itself. The textures are Mojang's, so
//! NetherNode doesn't ship them: when someone looks at an inventory, the
//! panel downloads that version's client from Mojang (as launchers do),
//! checks it, keeps it in its cache folder, and reads textures from it.
//!
//! Flat items (tools, food, ingots, …) use their item texture. Blocks are
//! shown as a cube from their top and side textures, like the game's
//! inventory does; odd shapes (stairs) get the closest cube. Items the
//! game draws with its own models (chests, beds, banners, heads, shields,
//! …) are cut from those models' textures: the item definitions say which
//! model and texture, and `layout` knows each model's shape.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Catalog, Error};

/// Largest client jar accepted.
const MAX_JAR: u64 = 200 * 1024 * 1024;
/// Hosts Mojang serves version files and clients from.
const MOJANG_HOSTS: &[&str] = &["piston-meta.mojang.com", "piston-data.mojang.com"];

/// How to draw an item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Cut from texture sheets, for items the game draws with its own
    /// models rather than item textures (chests, beds, heads, …).
    Sheet {
        /// The sheets parts are cut from.
        sheets: Vec<SheetTexture>,
        /// The whole icon when flat; the left face of a cube.
        front: Vec<Part>,
        /// For a cube, its top face; empty when flat.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        top: Vec<Part>,
        /// For a cube, its right face; empty when flat.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        side: Vec<Part>,
    },
}

/// A texture sheet an [`Icon::Sheet`] is cut from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetTexture {
    /// Texture path, like `entity/chest/normal`.
    pub texture: String,
    /// Its size in pixels, `[width, height]`.
    pub size: [u32; 2],
}

/// A rectangle of a texture sheet, placed on a 16×16 face.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    /// Which of the icon's sheets.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sheet: usize,
    /// `[x, y, width, height]` on the sheet, in pixels.
    pub from: [u32; 4],
    /// `[x, y, width, height]` on the face, 0 to 16.
    pub to: [f32; 4],
    /// Drawn upside down (the game keeps chest textures that way).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flip: bool,
    /// A colour to multiply it by (a banner's dye), like `#b02e26`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
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

/// An open game client or mod file.
pub type Jar = Arc<Mutex<zip::ZipArchive<std::fs::File>>>;

/// Opens a jar (a mod file, say) to draw items from.
pub fn open_jar(path: &Path) -> Result<Jar, Error> {
    let file = std::fs::File::open(path)
        .map_err(|e| Error::Upstream(format!("couldn't open {}: {e}", path.display())))?;
    let zip = zip::ZipArchive::new(file)
        .map_err(|e| Error::Upstream(format!("{} isn't a jar: {e}", path.display())))?;
    Ok(Arc::new(Mutex::new(zip)))
}

/// A texture's PNG from a mod file: `path` like `item/wrench` in
/// `namespace`.
pub async fn mod_texture(jar: Jar, namespace: &str, path: &str) -> Result<Vec<u8>, Error> {
    if !valid_namespace(namespace) || !valid_path(path) {
        return Err(Error::NotFound("no such texture".into()));
    }
    let entry = format!("assets/{namespace}/textures/{path}.png");
    tokio::task::spawn_blocking(move || {
        let mut zip = jar.lock().unwrap_or_else(|e| e.into_inner());
        read(&mut zip, &entry, 1024 * 1024).ok_or_else(|| Error::NotFound("no such texture".into()))
    })
    .await
    .map_err(|e| Error::Upstream(e.to_string()))?
}

/// Minecraft clients downloaded for their textures, by version.
pub struct ItemTextures {
    dir: PathBuf,
    /// Open clients, and when each was last used.
    jars: Mutex<HashMap<String, (Instant, Jar)>>,
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

    /// How to draw each of `ids` (`minecraft:diamond_sword`,
    /// `create:wrench`) in `version`, with `mods` for items from mods, and
    /// the concrete version the textures come from (`LATEST` resolves).
    /// Items it can't work out are left out.
    pub async fn icons(
        &self,
        catalog: &Catalog,
        version: &str,
        ids: Vec<String>,
        mods: Vec<Jar>,
    ) -> Result<(String, HashMap<String, Icon>), Error> {
        let version = catalog.release_id(version).await?;
        let jar = self.jar(catalog, &version).await?;
        let icons = tokio::task::spawn_blocking(move || {
            let mut vanilla = jar.lock().unwrap_or_else(|e| e.into_inner());
            let mut assets = Assets {
                vanilla: &mut vanilla,
                mods: mods
                    .iter()
                    .map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
                    .collect(),
            };
            ids.into_iter()
                .filter_map(|id| resolve(&mut assets, &id).map(|icon| (id, icon)))
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
            .insert(version, (Instant::now(), jar.clone()));
        Ok(jar)
    }

    fn cached(&self, version: &str) -> Option<Jar> {
        let mut jars = self.jars.lock().unwrap_or_else(|e| e.into_inner());
        let (used, jar) = jars.get_mut(version)?;
        *used = Instant::now();
        Some(jar.clone())
    }

    /// The folder clients are kept in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Folders of clients that can go: not a version in `keep`, and not
    /// used for `unused` (going by when it was downloaded, if it hasn't
    /// been opened since NetherNode started). They're forgotten here; the
    /// caller deletes them.
    pub fn prunable(&self, keep: &HashSet<String>, unused: Duration) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut jars = self.jars.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let Ok(version) = entry.file_name().into_string() else {
                continue;
            };
            if keep.contains(&version) {
                continue;
            }
            let recent = match jars.get(&version) {
                Some((used, _)) => used.elapsed() < unused,
                None => modified_within(&entry.path().join("client.jar"), unused),
            };
            if !recent {
                jars.remove(&version);
                out.push(entry.path());
            }
        }
        out
    }

    /// Forgets every client, so they're downloaded again when needed.
    pub fn forget_all(&self) {
        self.jars.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// Whether `path` was modified within `window` (false if it's missing).
pub fn modified_within(path: &Path, window: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < window)
}

impl Catalog {
    /// A concrete version: `LATEST` and `SNAPSHOT` become the newest of each.
    pub async fn release_id(&self, version: &str) -> Result<String, Error> {
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

type Zip = zip::ZipArchive<std::fs::File>;

fn read(zip: &mut Zip, name: &str, max: u64) -> Option<Vec<u8>> {
    let entry = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    entry.take(max).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Where an item's files come from: the game, and mods (which also point
/// at the game's models, like `minecraft:block/cube_all`).
struct Assets<'a> {
    vanilla: &'a mut Zip,
    mods: Vec<std::sync::MutexGuard<'a, Zip>>,
}

impl Assets<'_> {
    /// `assets/<namespace>/<rest>` from whichever file has it.
    fn read(&mut self, namespace: &str, rest: &str, max: u64) -> Option<Vec<u8>> {
        let name = format!("assets/{namespace}/{rest}");
        if namespace == "minecraft"
            && let Some(found) = read(self.vanilla, &name, max)
        {
            return Some(found);
        }
        self.mods
            .iter_mut()
            .find_map(|zip| read(zip, &name, max))
            .or_else(|| read(self.vanilla, &name, max))
    }

    fn json(&mut self, namespace: &str, rest: &str) -> Option<Value> {
        serde_json::from_slice(&self.read(namespace, rest, 256 * 1024)?).ok()
    }

    fn exists(&mut self, namespace: &str, rest: &str) -> bool {
        let name = format!("assets/{namespace}/{rest}");
        self.vanilla.by_name(&name).is_ok()
            || self.mods.iter_mut().any(|zip| zip.by_name(&name).is_ok())
    }
}

/// A resource location: `create:item/wrench` → (`create`, `item/wrench`);
/// no namespace means `minecraft`.
fn location(reference: &str) -> (String, String) {
    match reference.split_once(':') {
        Some((namespace, path)) => (namespace.to_owned(), path.to_owned()),
        None => ("minecraft".to_owned(), reference.to_owned()),
    }
}

fn valid_namespace(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

/// How an [`Icon`] names a texture: the game's as `block/stone`, a mod's as
/// `create:item/wrench`.
fn texture_name(namespace: &str, path: &str) -> String {
    if namespace == "minecraft" {
        path.to_owned()
    } else {
        format!("{namespace}:{path}")
    }
}

fn resolve(assets: &mut Assets<'_>, id: &str) -> Option<Icon> {
    let (namespace, name) = location(id);
    if !valid_namespace(&namespace) || !valid_path(&name) || name.contains('/') {
        return None;
    }
    let flat = format!("item/{name}");
    if assets.exists(&namespace, &format!("textures/{flat}.png")) {
        return Some(Icon::Flat {
            tint: (namespace == "minecraft")
                .then(|| tint_for(&flat))
                .flatten(),
            texture: texture_name(&namespace, &flat),
        });
    }
    // 1.21.4 and later describe items in items/; before, models/item/.
    let definition = assets.json(&namespace, &format!("items/{name}.json"));
    let special = match &definition {
        Some(definition) => special_model(definition),
        // Before 1.21.4 these were "builtin/entity", drawn by the game.
        None if namespace == "minecraft" => legacy_special(&name),
        None => None,
    };
    let mut base = None;
    if let Some(special) = special {
        match layout(&special.kind, &special.fields) {
            Some(layout) => {
                if let Some(icon) = sheet_icon(assets, layout) {
                    return Some(icon);
                }
            }
            None => unknown_model(&special.kind, id),
        }
        base = special.base;
    }
    // Otherwise the item's plain model: for a model-drawn item, the one the
    // game uses for its particles.
    let model = base
        .or_else(|| definition.as_ref().and_then(model_reference))
        .or_else(|| {
            let path = format!("item/{name}");
            assets
                .exists(&namespace, &format!("models/{path}.json"))
                .then(|| format!("{namespace}:{path}"))
        })?;
    model_icon(assets, &model)
}

/// Says once per kind that an item is drawn by a game model with no
/// layout here yet (a newer version's, or a mod's), so it can be added.
/// Until then the item shows its plain texture.
fn unknown_model(kind: &str, item: &str) {
    static SEEN: std::sync::LazyLock<Mutex<HashSet<String>>> =
        std::sync::LazyLock::new(Mutex::default);
    if SEEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(kind.to_owned())
    {
        tracing::info!(
            model = kind,
            item,
            "an item is drawn by a game model NetherNode can't draw yet; showing its plain texture"
        );
    }
}

type Fields = serde_json::Map<String, Value>;

/// A "special" model an item definition draws.
#[derive(Debug, PartialEq)]
struct Special {
    /// `chest`, `bed`, `head`, …
    kind: String,
    /// Its settings (texture, colour, …).
    fields: Fields,
    /// The plain model beside it, for particles.
    base: Option<String>,
}

/// The "special" model an item definition draws (`chest`, `bed`, `head`,
/// …): its kind and its settings (texture, colour). A chest's usual look
/// (the `fallback` of its date switch) wins over its Christmas one, and a
/// shield at rest over one in use.
fn special_model(definition: &Value) -> Option<Special> {
    let map = definition.as_object()?;
    if map
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| location(t).1 == "special")
    {
        let model = map.get("model")?.as_object()?;
        return Some(Special {
            kind: location(model.get("type")?.as_str()?).1,
            fields: model.clone(),
            base: map.get("base").and_then(Value::as_str).map(str::to_owned),
        });
    }
    const FIRST: [&str; 3] = ["model", "fallback", "on_false"];
    let nested = |value: &Value| match value {
        Value::Array(items) => items.iter().find_map(special_model),
        value => special_model(value),
    };
    FIRST
        .iter()
        .filter_map(|key| map.get(*key))
        .find_map(nested)
        .or_else(|| {
            map.iter()
                .filter(|(key, _)| !FIRST.contains(&key.as_str()))
                .find_map(|(_, value)| nested(value))
        })
}

/// What a 1.21.4 item definition would say for the game's model-drawn
/// items in older versions, which have none.
fn legacy_special(name: &str) -> Option<Special> {
    let with = |kind: &str, key: &str, value: &str| {
        let mut fields = Fields::new();
        fields.insert(key.to_owned(), Value::String(value.to_owned()));
        Some(Special {
            kind: kind.to_owned(),
            fields,
            base: None,
        })
    };
    let colour = |suffix: &str| {
        name.strip_suffix(suffix)
            .filter(|c| dye_colour(c).is_some())
    };
    match name {
        "chest" => with("chest", "texture", "normal"),
        "trapped_chest" => with("chest", "texture", "trapped"),
        "ender_chest" => with("chest", "texture", "ender"),
        "shulker_box" => with("shulker_box", "texture", "shulker"),
        "shield" | "conduit" | "decorated_pot" => Some(Special {
            kind: name.to_owned(),
            fields: Fields::new(),
            base: None,
        }),
        "skeleton_skull" => with("head", "kind", "skeleton"),
        "wither_skeleton_skull" => with("head", "kind", "wither_skeleton"),
        "zombie_head" => with("head", "kind", "zombie"),
        "creeper_head" => with("head", "kind", "creeper"),
        "player_head" => with("head", "kind", "player"),
        "piglin_head" => with("head", "kind", "piglin"),
        "dragon_head" => with("head", "kind", "dragon"),
        _ => {
            if let Some(c) = colour("_bed") {
                with("bed", "texture", c)
            } else if let Some(c) = colour("_shulker_box") {
                with("shulker_box", "texture", &format!("shulker_{c}"))
            } else if let Some(c) = colour("_banner") {
                with("banner", "color", c)
            } else {
                None
            }
        }
    }
}

/// The colour the game dyes things (banners) each colour.
fn dye_colour(name: &str) -> Option<&'static str> {
    Some(match name {
        "white" => "#f9fffe",
        "orange" => "#f9801d",
        "magenta" => "#c74ebd",
        "light_blue" => "#3ab3da",
        "yellow" => "#fed83d",
        "lime" => "#80c71f",
        "pink" => "#f38baa",
        "gray" => "#474f52",
        "light_gray" => "#9d9d97",
        "cyan" => "#169c9c",
        "purple" => "#8932b8",
        "blue" => "#3c44aa",
        "brown" => "#835432",
        "green" => "#5e7c16",
        "red" => "#b02e26",
        "black" => "#1d1d21",
        _ => return None,
    })
}

/// A sheet's size from its PNG header.
fn png_size(assets: &mut Assets<'_>, namespace: &str, path: &str) -> Option<[u32; 2]> {
    let head = assets.read(namespace, &format!("textures/{path}.png"), 24)?;
    if head.len() < 24 || &head[1..4] != b"PNG" {
        return None;
    }
    let word = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    let size = [word(16), word(20)];
    (size[0] > 0 && size[1] > 0 && size[0] <= 4096 && size[1] <= 4096).then_some(size)
}

/// How a model-drawn item is cut from its sheets. Positions on a sheet are
/// in the units of the game's own sheet (`width` wide); bigger sheets
/// (high-resolution packs) are scaled to match.
struct Layout {
    /// (namespace, texture path, width the positions assume).
    sheets: Vec<(String, String, u32)>,
    front: Vec<Cut>,
    top: Vec<Cut>,
    side: Vec<Cut>,
}

struct Cut {
    sheet: usize,
    from: [u32; 4],
    to: [f32; 4],
    flip: bool,
    tint: Option<String>,
}

const FACE: [f32; 4] = [0.0, 0.0, 16.0, 16.0];

/// Part `from` of the first sheet, placed at `to`.
fn cut(from: [u32; 4], to: [f32; 4]) -> Cut {
    Cut {
        sheet: 0,
        from,
        to,
        flip: false,
        tint: None,
    }
}

impl Cut {
    fn flipped(self) -> Self {
        Self { flip: true, ..self }
    }

    fn on(self, sheet: usize) -> Self {
        Self { sheet, ..self }
    }

    fn tinted(self, tint: &str) -> Self {
        Self {
            tint: Some(tint.to_owned()),
            ..self
        }
    }
}

/// The layout for a model kind with `fields` (texture, colour, …), or
/// `None` for kinds this doesn't know.
fn layout(kind: &str, fields: &Fields) -> Option<Layout> {
    let field = |key: &str| fields.get(key).and_then(Value::as_str);
    // A texture named in the definition, in its folder (`minecraft:red` →
    // `entity/bed/red`).
    let named = |default: &str, folder: &str| {
        let (namespace, name) = location(field("texture").unwrap_or(default));
        (namespace, format!("{folder}{name}"), 64)
    };
    let game = |path: &str, width: u32| ("minecraft".to_owned(), path.to_owned(), width);
    let flat = |sheet, front| Layout {
        sheets: vec![sheet],
        front,
        top: Vec::new(),
        side: Vec::new(),
    };
    let cube = |sheets, top, front, side| Layout {
        sheets,
        front,
        top,
        side,
    };
    Some(match kind {
        // A 14×14 box: the lid (5 high) on the base (10 high), stored
        // upside down. The front, with the latch, faces left.
        "chest" => {
            const LID: f32 = 16.0 * 5.0 / 15.0;
            cube(
                vec![named("normal", "entity/chest/")],
                vec![cut([28, 0, 14, 14], FACE)],
                vec![
                    cut([42, 14, 14, 5], [0.0, 0.0, 16.0, LID]).flipped(),
                    cut([42, 33, 14, 10], [0.0, LID, 16.0, 16.0 - LID]).flipped(),
                    // The latch, over the seam.
                    cut([4, 1, 2, 4], [6.86, 3.43, 2.29, 4.57]).flipped(),
                ],
                vec![
                    cut([0, 14, 14, 5], [0.0, 0.0, 16.0, LID]).flipped(),
                    cut([0, 33, 14, 10], [0.0, LID, 16.0, 16.0 - LID]).flipped(),
                ],
            )
        }
        // The shell's lid (12 high) over the bottom of its base.
        "shulker_box" => {
            let sides = |x: u32| {
                vec![
                    cut([x, 16, 16, 12], [0.0, 0.0, 16.0, 12.0]),
                    cut([x, 48, 16, 4], [0.0, 12.0, 16.0, 4.0]),
                ]
            };
            cube(
                vec![named("shulker", "entity/shulker/")],
                vec![cut([16, 0, 16, 16], FACE)],
                sides(0),
                sides(16),
            )
        }
        // Seen from above: the pillow end, then the foot end.
        "bed" => flat(
            named("red", "entity/bed/"),
            vec![
                cut([6, 6, 16, 16], [4.0, 0.0, 8.0, 8.0]),
                cut([6, 28, 16, 16], [4.0, 8.0, 8.0, 8.0]),
            ],
        ),
        // The cloth (dyed) under its crossbar.
        "banner" => {
            let colour = dye_colour(field("color").unwrap_or("white"))?;
            flat(
                game("entity/banner_base", 64),
                vec![
                    cut([2, 44, 20, 2], [4.25, 0.0, 7.5, 0.75]),
                    cut([1, 1, 20, 40], [4.25, 0.75, 7.5, 15.0]).tinted(colour),
                ],
            )
        }
        // The plate's front, 12×22.
        "shield" => flat(
            game("entity/shield_base_nopattern", 64),
            vec![cut([1, 1, 12, 22], [3.64, 0.0, 8.73, 16.0])],
        ),
        // A mob's head: the 8×8×8 box at the sheet's corner, with the hat
        // layer over it for players and zombies.
        "head" => {
            let (path, wide, hat) = match field("kind")? {
                "skeleton" => ("entity/skeleton/skeleton", 8, false),
                "wither_skeleton" => ("entity/skeleton/wither_skeleton", 8, false),
                "zombie" => ("entity/zombie/zombie", 8, true),
                "creeper" => ("entity/creeper/creeper", 8, false),
                "piglin" => ("entity/piglin/piglin", 10, false),
                "player" => ("entity/player/wide/steve", 8, true),
                // The dragon's 16×16×16 head box, at (112, 30).
                "dragon" => {
                    return Some(cube(
                        vec![game("entity/enderdragon/dragon", 256)],
                        vec![cut([128, 30, 16, 16], FACE)],
                        vec![cut([128, 46, 16, 16], FACE)],
                        vec![cut([144, 46, 16, 16], FACE)],
                    ));
                }
                _ => return None,
            };
            let mut top = vec![cut([8, 0, wide, 8], FACE)];
            let mut front = vec![cut([8, 8, wide, 8], FACE)];
            let mut side = vec![cut([8 + wide, 8, 8, 8], FACE)];
            if hat {
                top.push(cut([40, 0, 8, 8], FACE));
                front.push(cut([40, 8, 8, 8], FACE));
                side.push(cut([48, 8, 8, 8], FACE));
            }
            cube(vec![game(path, 64)], top, front, side)
        }
        // The 6×6×6 shell.
        "conduit" => cube(
            vec![game("entity/conduit/base", 32)],
            vec![cut([6, 0, 6, 6], FACE)],
            vec![cut([6, 6, 6, 6], FACE)],
            vec![cut([12, 6, 6, 6], FACE)],
        ),
        // The body: its top from the pot's sheet, its sides plain.
        "decorated_pot" => cube(
            vec![
                game("entity/decorated_pot/decorated_pot_base", 32),
                game("entity/decorated_pot/decorated_pot_side", 16),
            ],
            vec![cut([0, 13, 14, 14], FACE)],
            vec![cut([1, 0, 14, 16], FACE).on(1)],
            vec![cut([1, 0, 14, 16], FACE).on(1)],
        ),
        _ => return None,
    })
}

/// Cuts a [`Layout`] from its sheets, if they're all there and the parts
/// fit them.
fn sheet_icon(assets: &mut Assets<'_>, layout: Layout) -> Option<Icon> {
    let mut sheets = Vec::new();
    let mut scales = Vec::new();
    for (namespace, path, width) in &layout.sheets {
        if !valid_namespace(namespace) || !valid_path(path) {
            return None;
        }
        let size = png_size(assets, namespace, path)?;
        if size[0] % width != 0 {
            return None;
        }
        scales.push(size[0] / width);
        sheets.push(SheetTexture {
            texture: texture_name(namespace, path),
            size,
        });
    }
    let place = |cuts: Vec<Cut>| -> Option<Vec<Part>> {
        cuts.into_iter()
            .map(|c| {
                let (scale, size) = (*scales.get(c.sheet)?, sheets.get(c.sheet)?.size);
                let from = c.from.map(|v| v * scale);
                (from[0] + from[2] <= size[0] && from[1] + from[3] <= size[1]).then_some(Part {
                    sheet: c.sheet,
                    from,
                    to: c.to,
                    flip: c.flip,
                    tint: c.tint,
                })
            })
            .collect()
    };
    let (front, top, side) = (
        place(layout.front)?,
        place(layout.top)?,
        place(layout.side)?,
    );
    Some(Icon::Sheet {
        sheets,
        front,
        top,
        side,
    })
}

/// The first plain model an item definition points to.
fn model_reference(definition: &Value) -> Option<String> {
    match definition {
        Value::Object(map) => {
            if let (Some(Value::String(kind)), Some(Value::String(model))) =
                (map.get("type"), map.get("model"))
                && location(kind).1 == "model"
            {
                return Some(model.clone());
            }
            map.values().find_map(model_reference)
        }
        Value::Array(items) => items.iter().find_map(model_reference),
        _ => None,
    }
}

fn model_icon(assets: &mut Assets<'_>, model: &str) -> Option<Icon> {
    // Texture variables, each with the namespace of the model that set it.
    let mut textures: HashMap<String, String> = HashMap::new();
    let mut current = Some(location(model));
    let mut generated = false;
    for _ in 0..12 {
        let Some((namespace, path)) = current.take() else {
            break;
        };
        if path.ends_with("generated") || path.starts_with("builtin/") {
            generated = path.ends_with("generated");
            break;
        }
        if !valid_namespace(&namespace) || !valid_path(&path) {
            break;
        }
        let Some(json) = assets.json(&namespace, &format!("models/{path}.json")) else {
            break;
        };
        if let Some(Value::Object(own)) = json.get("textures") {
            for (key, value) in own {
                if let Value::String(value) = value {
                    textures.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        current = json.get("parent").and_then(Value::as_str).map(location);
    }
    let lookup = |key: &str| -> Option<(String, String)> {
        let mut value = textures.get(key)?.clone();
        for _ in 0..8 {
            match value.strip_prefix('#') {
                Some(next) => value = textures.get(next)?.clone(),
                None => return Some(location(&value)),
            }
        }
        None
    };
    let pick = |keys: &[&str]| keys.iter().find_map(|k| lookup(k));
    let usable = |assets: &mut Assets<'_>, (namespace, path): &(String, String)| {
        valid_namespace(namespace)
            && valid_path(path)
            && assets.exists(namespace, &format!("textures/{path}.png"))
    };
    let tint = |(namespace, path): &(String, String)| {
        (namespace == "minecraft").then(|| tint_for(path)).flatten()
    };
    if generated || textures.contains_key("layer0") {
        let texture = lookup("layer0")?;
        return usable(assets, &texture).then(|| Icon::Flat {
            tint: tint(&texture),
            texture: texture_name(&texture.0, &texture.1),
        });
    }
    let top = pick(&["top", "end", "up", "all", "texture", "particle"]);
    let side = pick(&["side", "front", "north", "all", "texture", "particle"]);
    match (top, side) {
        (Some(top), Some(side)) if usable(assets, &top) && usable(assets, &side) => {
            Some(Icon::Cube {
                tint: tint(&top),
                top: texture_name(&top.0, &top.1),
                side: texture_name(&side.0, &side.1),
            })
        }
        _ => {
            let texture = lookup("particle")?;
            usable(assets, &texture).then(|| Icon::Flat {
                tint: tint(&texture),
                texture: texture_name(&texture.0, &texture.1),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn unused_clients_of_versions_no_server_runs_can_go() {
        let dir = tempfile::tempdir().unwrap();
        let textures = ItemTextures::new(dir.path().to_owned());
        let old = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
        for version in ["1.20.1", "1.21.1", "1.21.4"] {
            std::fs::create_dir(dir.path().join(version)).unwrap();
            let file = std::fs::File::create(dir.path().join(version).join("client.jar")).unwrap();
            if version != "1.21.4" {
                file.set_modified(old).unwrap();
            }
        }
        let keep = HashSet::from(["1.21.1".to_owned()]);
        let week = Duration::from_secs(7 * 24 * 3600);
        assert_eq!(
            textures.prunable(&keep, week),
            [dir.path().join("1.20.1")],
            "1.21.1 is in use and 1.21.4 is new"
        );
    }

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
        let mut vanilla = jar(dir.path());
        let mut zip = Assets {
            vanilla: &mut vanilla,
            mods: Vec::new(),
        };
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
        assert_eq!(
            resolve(&mut zip, "create:wrench"),
            None,
            "no mod files given"
        );
        assert_eq!(resolve(&mut zip, "minecraft:../secrets"), None);
    }

    #[test]
    fn mod_items_use_the_mods_textures_and_the_games_models() {
        let dir = tempfile::tempdir().unwrap();
        let mut vanilla = jar(dir.path());
        let path = dir.path().join("create.jar");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("assets/create/textures/item/wrench.png", &b"\x89PNG"[..]),
            ("assets/create/textures/block/andesite_casing.png", b"\x89PNG"),
            ("assets/create/models/item/andesite_casing.json", br#"{"parent":"create:block/andesite_casing"}"#),
            (
                "assets/create/models/block/andesite_casing.json",
                br#"{"parent":"minecraft:block/cube_all","textures":{"all":"create:block/andesite_casing"}}"#,
            ),
        ] {
            writer.start_file(name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();
        let jar = open_jar(&path).unwrap();
        let guard = jar.lock().unwrap();
        let mut assets = Assets {
            vanilla: &mut vanilla,
            mods: vec![guard],
        };
        assert_eq!(
            resolve(&mut assets, "create:wrench"),
            Some(Icon::Flat {
                texture: "create:item/wrench".into(),
                tint: None
            })
        );
        assert_eq!(
            resolve(&mut assets, "create:andesite_casing"),
            Some(Icon::Cube {
                top: "create:block/andesite_casing".into(),
                side: "create:block/andesite_casing".into(),
                tint: None
            })
        );
        assert_eq!(resolve(&mut assets, "create:missing"), None);
        assert_eq!(resolve(&mut assets, "Create:wrench"), None);
    }

    /// A PNG's first bytes, up to its size.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        out.extend(width.to_be_bytes());
        out.extend(height.to_be_bytes());
        out.extend([8, 6, 0, 0, 0]);
        out
    }

    #[test]
    fn model_drawn_items_are_cut_from_their_sheets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.jar");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        let sheet = png_header(64, 64);
        let big = png_header(128, 128);
        let half = png_header(64, 32);
        let special = |model: &str| {
            format!(
                r#"{{"model":{{"type":"minecraft:special","base":"minecraft:item/x","model":{model}}}}}"#
            )
        };
        let files: Vec<(&str, Vec<u8>)> = vec![
            ("assets/minecraft/textures/entity/chest/normal.png", sheet.clone()),
            ("assets/minecraft/textures/entity/chest/christmas.png", sheet.clone()),
            ("assets/minecraft/textures/entity/chest/ender.png", big.clone()),
            ("assets/minecraft/textures/entity/chest/trapped.png", sheet.clone()),
            ("assets/minecraft/textures/entity/shield_base_nopattern.png", sheet.clone()),
            ("assets/minecraft/textures/entity/bed/red.png", sheet.clone()),
            ("assets/minecraft/textures/entity/banner_base.png", sheet.clone()),
            ("assets/minecraft/textures/entity/skeleton/skeleton.png", half.clone()),
            ("assets/minecraft/textures/entity/player/wide/steve.png", sheet.clone()),
            (
                "assets/minecraft/items/chest.json",
                br#"{"model":{"type":"minecraft:select","property":"minecraft:local_time","cases":[{"when":["12-24"],"model":{"type":"minecraft:special","base":"minecraft:item/chest","model":{"type":"minecraft:chest","texture":"minecraft:christmas"}}}],"fallback":{"type":"minecraft:special","base":"minecraft:item/chest","model":{"type":"minecraft:chest","texture":"minecraft:normal"}}}}"#.to_vec(),
            ),
            (
                "assets/minecraft/items/ender_chest.json",
                special(r#"{"type":"minecraft:chest","texture":"minecraft:ender"}"#).into_bytes(),
            ),
            (
                "assets/minecraft/items/shield.json",
                br#"{"model":{"type":"minecraft:condition","property":"minecraft:using_item","on_false":{"type":"minecraft:special","base":"minecraft:item/shield","model":{"type":"minecraft:shield"}},"on_true":{"type":"minecraft:special","base":"minecraft:item/shield_blocking","model":{"type":"minecraft:shield"}}}}"#.to_vec(),
            ),
            (
                "assets/minecraft/items/red_banner.json",
                special(r#"{"type":"minecraft:banner","color":"red"}"#).into_bytes(),
            ),
            (
                "assets/minecraft/items/skeleton_skull.json",
                special(r#"{"type":"minecraft:head","kind":"skeleton"}"#).into_bytes(),
            ),
            (
                "assets/minecraft/items/player_head.json",
                special(r#"{"type":"minecraft:head","kind":"player"}"#).into_bytes(),
            ),
            (
                "assets/minecraft/items/copper_golem_statue.json",
                special(r#"{"type":"minecraft:copper_golem_statue"}"#).into_bytes(),
            ),
            (
                "assets/minecraft/models/item/x.json",
                br#"{"parent":"builtin/entity","textures":{"particle":"block/copper_block"}}"#.to_vec(),
            ),
            ("assets/minecraft/textures/block/copper_block.png", sheet.clone()),
        ];
        for (name, body) in files {
            writer.start_file(name, options).unwrap();
            writer.write_all(&body).unwrap();
        }
        writer.finish().unwrap();
        let mut vanilla = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut assets = Assets {
            vanilla: &mut vanilla,
            mods: Vec::new(),
        };
        let sheet_of = |icon: Option<Icon>| match icon {
            Some(Icon::Sheet {
                sheets,
                front,
                top,
                side,
            }) => (sheets, front, top, side),
            other => panic!("not cut from a sheet: {other:?}"),
        };

        let (sheets, front, top, side) = sheet_of(resolve(&mut assets, "minecraft:chest"));
        assert_eq!(
            sheets[0].texture, "entity/chest/normal",
            "not the Christmas one"
        );
        assert_eq!(sheets[0].size, [64, 64]);
        assert_eq!(top[0].from, [28, 0, 14, 14]);
        assert_eq!(front.len(), 3, "lid, base, and latch");
        assert!(front.iter().all(|p| p.flip));
        assert_eq!(side.len(), 2);

        let (sheets, front, ..) = sheet_of(resolve(&mut assets, "ender_chest"));
        assert_eq!(sheets[0].size, [128, 128]);
        assert_eq!(front[0].from, [84, 28, 28, 10], "scaled for a bigger sheet");

        let (sheets, front, top, _) = sheet_of(resolve(&mut assets, "minecraft:shield"));
        assert_eq!(sheets[0].texture, "entity/shield_base_nopattern");
        assert_eq!(front[0].from, [1, 1, 12, 22]);
        assert!(top.is_empty(), "drawn flat");

        let (_, front, ..) = sheet_of(resolve(&mut assets, "red_banner"));
        assert_eq!(
            front[1].tint.as_deref(),
            Some("#b02e26"),
            "the cloth is dyed"
        );

        let (sheets, front, ..) = sheet_of(resolve(&mut assets, "skeleton_skull"));
        assert_eq!(sheets[0].size, [64, 32]);
        assert_eq!(front.len(), 1, "no hat layer");
        let (_, front, ..) = sheet_of(resolve(&mut assets, "player_head"));
        assert_eq!(front.len(), 2, "with the hat layer");

        // Before 1.21.4 there are no item definitions; the names say it.
        let (sheets, ..) = sheet_of(resolve(&mut assets, "minecraft:trapped_chest"));
        assert_eq!(sheets[0].texture, "entity/chest/trapped");
        let (sheets, front, ..) = sheet_of(resolve(&mut assets, "red_bed"));
        assert_eq!(sheets[0].texture, "entity/bed/red");
        assert_eq!(front.len(), 2, "pillow end and foot end");
        assert_eq!(
            legacy_special("purple_shulker_box").unwrap().fields["texture"],
            "shulker_purple"
        );
        assert_eq!(legacy_special("not_a_bed"), None);

        // A kind with no layout yet shows its plain texture.
        assert_eq!(
            resolve(&mut assets, "copper_golem_statue"),
            Some(Icon::Cube {
                top: "block/copper_block".into(),
                side: "block/copper_block".into(),
                tint: None
            }),
            "from its base model"
        );
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
