//! Modpacks and mods, the same shape whichever platform they come from.

use serde::{Deserialize, Serialize};

/// Where content comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// modrinth.com (public API).
    Modrinth,
    /// curseforge.com (needs an API key).
    CurseForge,
}

/// What kind of project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    /// A whole server's worth of mods and config.
    Modpack,
    /// One mod.
    Mod,
}

/// A modpack or mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Where it's from.
    pub platform: Platform,
    /// Platform ID.
    pub id: String,
    /// URL name.
    pub slug: String,
    /// Display name.
    pub title: String,
    /// One-line description.
    pub summary: String,
    /// Who made it.
    pub author: String,
    /// Icon image URL.
    pub icon_url: Option<String>,
    /// Total downloads.
    pub downloads: u64,
    /// Mod loaders it supports (`fabric`, `forge`, `neoforge`, `quilt`).
    pub loaders: Vec<String>,
    /// Minecraft versions it supports, newest last.
    pub game_versions: Vec<String>,
    /// Its page on the platform's website.
    pub page_url: String,
    /// Whether it runs on servers: `required`, `optional`, `unsupported`,
    /// or `unknown`.
    pub server_side: String,
    /// Last update, unix ms.
    pub updated_ms: i64,
}

/// One file of a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionFile {
    /// Where to download it; `None` when the author doesn't allow other
    /// apps to download it (CurseForge).
    pub url: Option<String>,
    /// File name.
    pub filename: String,
    /// Bytes.
    pub size: u64,
    /// Hex SHA-1.
    pub sha1: Option<String>,
    /// Hex SHA-512 (Modrinth).
    pub sha512: Option<String>,
    /// The main file of the version.
    pub primary: bool,
}

/// A release of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectVersion {
    /// Platform ID (Modrinth version ID, CurseForge file ID).
    pub id: String,
    /// The project it belongs to.
    pub project_id: String,
    /// Display name.
    pub name: String,
    /// Version number as the author wrote it.
    pub version_number: String,
    /// Minecraft versions it's for.
    pub game_versions: Vec<String>,
    /// Mod loaders it's for.
    pub loaders: Vec<String>,
    /// `release`, `beta`, or `alpha`.
    pub channel: String,
    /// Publication time, unix ms.
    pub published_ms: i64,
    /// Its files.
    pub files: Vec<VersionFile>,
    /// Other projects it needs or works with.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// The file ID of the server pack the author published alongside this
    /// version (CurseForge modpacks).
    #[serde(default)]
    pub server_pack_id: Option<String>,
}

/// Another project a version depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// The project, when given.
    pub project_id: Option<String>,
    /// A specific version, when given.
    pub version_id: Option<String>,
    /// Needed for the mod to work (not just optional or incompatible).
    pub required: bool,
}

impl ProjectVersion {
    /// The file to install.
    pub fn primary_file(&self) -> Option<&VersionFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.first())
    }
}

/// A search.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Free text; empty lists the most popular.
    pub text: String,
    /// Only content for this Minecraft version.
    pub mc_version: Option<String>,
    /// Only content for this loader.
    pub loader: Option<String>,
    /// Results to skip.
    pub offset: u32,
    /// Results per page (at most 50).
    pub limit: u32,
}

/// A page of search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    /// The matches.
    pub projects: Vec<Project>,
    /// How many match in total.
    pub total: u64,
}

/// Mod loaders that run mods, as platforms name them.
pub const MOD_LOADERS: &[&str] = &["fabric", "forge", "neoforge", "quilt"];

pub(crate) fn millis(rfc3339: &str) -> i64 {
    rfc3339
        .parse::<jiff::Timestamp>()
        .map(|t| t.as_millisecond())
        .unwrap_or(0)
}
