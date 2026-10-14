//! What can be installed: Minecraft versions, server software builds,
//! modpacks and mods from Modrinth and CurseForge, and what's inside a mod
//! jar.
//!
//! Upstream answers are cached on disk for six hours. When an upstream is
//! down, older cached data is used rather than failing.

mod cache;
pub mod content;
mod curseforge;
pub mod jar;
mod modrinth;
pub mod versions;

use std::path::PathBuf;
use std::time::Duration;

pub use content::{
    Platform, Project, ProjectKind, ProjectVersion, SearchPage, SearchQuery, VersionFile,
};
pub use versions::{LoaderVersion, McVersion, compare_versions};

/// Identifies NetherNode to upstream APIs (Modrinth asks for this). No
/// personal details.
pub const USER_AGENT: &str = concat!("NetherNode/", env!("CARGO_PKG_VERSION"));

/// How long cached answers stay fresh.
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);

/// How long one upstream request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Largest upstream answer read.
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// Why something couldn't be listed or fetched. Messages are shown to admins.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The upstream failed or answered with something unexpected.
    #[error("{0}")]
    Upstream(String),
    /// Not something NetherNode knows how to do.
    #[error("{0}")]
    Unsupported(String),
    /// Needs configuration (a CurseForge API key).
    #[error("{0}")]
    NotConfigured(String),
    /// Nothing by that name.
    #[error("{0}")]
    NotFound(String),
}

/// Where each upstream lives (overridable for tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Mojang's version manifest.
    pub mojang: String,
    /// Fabric meta API base.
    pub fabric: String,
    /// Quilt meta API base.
    pub quilt: String,
    /// Forge's Maven metadata.
    pub forge_maven: String,
    /// Forge's recommended/latest builds.
    pub forge_promotions: String,
    /// NeoForge's Maven metadata.
    pub neoforge_maven: String,
    /// Paper's (Fill) API base.
    pub paper: String,
    /// Purpur's API base.
    pub purpur: String,
    /// Modrinth API base.
    pub modrinth: String,
    /// CurseForge API base.
    pub curseforge: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            mojang: "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json".into(),
            fabric: "https://meta.fabricmc.net".into(),
            quilt: "https://meta.quiltmc.org".into(),
            forge_maven:
                "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml"
                    .into(),
            forge_promotions:
                "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json"
                    .into(),
            neoforge_maven:
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
                    .into(),
            paper: "https://fill.papermc.io".into(),
            purpur: "https://api.purpurmc.org".into(),
            modrinth: "https://api.modrinth.com".into(),
            curseforge: "https://api.curseforge.com".into(),
        }
    }
}

/// Hosts mod and modpack files are downloaded from.
const DOWNLOAD_HOSTS: &[&str] = &[
    "cdn.modrinth.com",
    "edge.forgecdn.net",
    "mediafilez.forgecdn.net",
    "media.forgecdn.net",
];

/// Whether `url` is an https URL on one of the download hosts.
pub fn download_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.host_str().is_some_and(|h| DOWNLOAD_HOSTS.contains(&h))
}

/// The catalog client.
pub struct Catalog {
    http: reqwest::Client,
    /// For files: only follows redirects to other download hosts.
    downloads: reqwest::Client,
    cache: cache::Cache,
    endpoints: Endpoints,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new(None, Endpoints::default())
    }
}

impl Catalog {
    /// A catalog caching under `cache_dir` (`None`: no disk cache).
    pub fn new(cache_dir: Option<PathBuf>, endpoints: Endpoints) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        let downloads = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 5 && download_allowed(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .unwrap_or_default();
        Self {
            http,
            downloads,
            cache: cache::Cache::new(cache_dir, CACHE_TTL),
            endpoints,
        }
    }

    /// Downloads a mod or modpack file of at most `max` bytes from the
    /// platforms' file hosts, checking it against the hashes given.
    pub async fn download(&self, file: &VersionFile, max: u64) -> Result<Vec<u8>, Error> {
        use sha1::Digest as _;
        let url = file.url.as_deref().ok_or_else(|| {
            Error::NotConfigured(format!(
                "{}'s author doesn't allow other apps to download it; download it from the website and upload it",
                file.filename
            ))
        })?;
        let parsed =
            reqwest::Url::parse(url).map_err(|_| Error::Upstream("bad download link".into()))?;
        if !download_allowed(&parsed) {
            return Err(Error::Upstream(format!(
                "won't download from {}",
                parsed.host_str().unwrap_or("there")
            )));
        }
        if file.size > max {
            return Err(Error::Upstream(format!("{} is too large", file.filename)));
        }
        let mut response = self.downloads.get(parsed).send().await.map_err(|e| {
            Error::Upstream(format!(
                "downloading {}: {}",
                file.filename,
                e.without_url()
            ))
        })?;
        if !response.status().is_success() {
            return Err(Error::Upstream(format!(
                "downloading {}: {}",
                file.filename,
                response.status()
            )));
        }
        let mut data = Vec::with_capacity(usize::try_from(file.size.min(max)).unwrap_or(0));
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            Error::Upstream(format!(
                "downloading {}: {}",
                file.filename,
                e.without_url()
            ))
        })? {
            if (data.len() + chunk.len()) as u64 > max {
                return Err(Error::Upstream(format!("{} is too large", file.filename)));
            }
            data.extend_from_slice(&chunk);
        }
        let matches = match (&file.sha512, &file.sha1) {
            (Some(want), _) => hex::encode(sha2::Sha512::digest(&data)).eq_ignore_ascii_case(want),
            (None, Some(want)) => hex::encode(sha1::Sha1::digest(&data)).eq_ignore_ascii_case(want),
            (None, None) => true,
        };
        if !matches {
            return Err(Error::Upstream(format!(
                "{} didn't download intact (checksum mismatch)",
                file.filename
            )));
        }
        Ok(data)
    }

    /// The HTTP client (with NetherNode's user agent and timeouts).
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// GETs `url` as text, failing on error statuses.
    async fn fetch_text(&self, request: reqwest::RequestBuilder) -> Result<String, Error> {
        let response = request.send().await.map_err(|e| {
            Error::Upstream(format!(
                "couldn't reach {}: {}",
                host_of(&e),
                e.without_url()
            ))
        })?;
        let status = response.status();
        let url = response.url().host_str().unwrap_or("upstream").to_owned();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound(format!("{url} has no such item")));
        }
        if !status.is_success() {
            return Err(Error::Upstream(format!("{url} answered {status}")));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Upstream(format!("{url}: {}", e.without_url())))?;
        if bytes.len() > MAX_RESPONSE {
            return Err(Error::Upstream(format!("{url} sent too much data")));
        }
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::Upstream(format!("{url} sent invalid text")))
    }

    /// `url`'s body, through the disk cache under `key`.
    pub(crate) async fn cached_text(&self, key: &str, url: &str) -> Result<String, Error> {
        self.cache
            .get(key, || self.fetch_text(self.http.get(url)))
            .await
    }
}

fn host_of(e: &reqwest::Error) -> String {
    e.url()
        .and_then(|u| u.host_str())
        .unwrap_or("upstream")
        .to_owned()
}
