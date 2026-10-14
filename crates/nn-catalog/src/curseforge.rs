//! CurseForge (needs an API key): <https://docs.curseforge.com/rest-api/>.

use serde::Deserialize;

use crate::content::{
    Platform, Project, ProjectKind, ProjectVersion, SearchPage, SearchQuery, VersionFile, millis,
};
use crate::versions::{enc, parse_json};
use crate::{Catalog, Error};

/// Minecraft's game ID on CurseForge.
const MINECRAFT: u32 = 432;
const CLASS_MODPACK: u32 = 4471;
const CLASS_MOD: u32 = 6;

/// CurseForge's numbers for mod loaders.
fn loader_type(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

fn loader_name(n: u32) -> Option<&'static str> {
    match n {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mod {
    id: u64,
    name: String,
    slug: String,
    links: Links,
    summary: String,
    download_count: f64,
    #[serde(default)]
    authors: Vec<Author>,
    logo: Option<Logo>,
    #[serde(default)]
    latest_files_indexes: Vec<FileIndex>,
    date_modified: String,
    class_id: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Links {
    website_url: String,
}

#[derive(Deserialize)]
struct Author {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logo {
    thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileIndex {
    game_version: String,
    mod_loader: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    id: u64,
    mod_id: u64,
    display_name: String,
    file_name: String,
    release_type: u32,
    file_date: String,
    file_length: u64,
    download_url: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    hashes: Vec<Hash>,
}

#[derive(Deserialize)]
struct Hash {
    value: String,
    algo: u32,
}

#[derive(Deserialize)]
struct Data<T> {
    data: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    total_count: u64,
}

#[derive(Deserialize)]
struct Paged<T> {
    data: Vec<T>,
    pagination: Pagination,
}

fn project(m: Mod) -> Project {
    let mut loaders: Vec<String> = m
        .latest_files_indexes
        .iter()
        .filter_map(|i| i.mod_loader.and_then(loader_name))
        .map(str::to_owned)
        .collect();
    loaders.sort();
    loaders.dedup();
    let mut game_versions: Vec<String> = m
        .latest_files_indexes
        .iter()
        .map(|i| i.game_version.clone())
        .collect();
    game_versions.sort_by(|a, b| crate::compare_versions(a, b));
    game_versions.dedup();
    Project {
        platform: Platform::CurseForge,
        id: m.id.to_string(),
        slug: m.slug,
        title: m.name,
        summary: m.summary,
        author: m
            .authors
            .into_iter()
            .next()
            .map(|a| a.name)
            .unwrap_or_default(),
        icon_url: m
            .logo
            .and_then(|l| l.thumbnail_url)
            .filter(|u| !u.is_empty()),
        downloads: m.download_count as u64,
        loaders,
        game_versions,
        page_url: m.links.website_url,
        // CurseForge doesn't say; mods for servers are usually fine.
        server_side: if m.class_id == Some(CLASS_MODPACK) {
            "required"
        } else {
            "unknown"
        }
        .into(),
        updated_ms: millis(&m.date_modified),
    }
}

fn version(f: File) -> ProjectVersion {
    // gameVersions mixes Minecraft versions with loader and side names.
    let (loaders, game_versions): (Vec<String>, Vec<String>) = f
        .game_versions
        .into_iter()
        .partition(|v| !v.starts_with(|c: char| c.is_ascii_digit()));
    let loaders = loaders
        .into_iter()
        .map(|l| l.to_ascii_lowercase())
        .filter(|l| ["forge", "fabric", "quilt", "neoforge"].contains(&l.as_str()))
        .collect();
    ProjectVersion {
        id: f.id.to_string(),
        project_id: f.mod_id.to_string(),
        name: f.display_name.clone(),
        version_number: f.display_name,
        game_versions,
        loaders,
        channel: match f.release_type {
            1 => "release",
            2 => "beta",
            _ => "alpha",
        }
        .into(),
        published_ms: millis(&f.file_date),
        files: vec![VersionFile {
            url: f.download_url.filter(|u| !u.is_empty()),
            filename: f.file_name,
            size: f.file_length,
            sha1: f
                .hashes
                .iter()
                .find(|h| h.algo == 1)
                .map(|h| h.value.clone()),
            sha512: None,
            primary: true,
        }],
    }
}

pub(crate) fn parse_search(raw: &str) -> Result<SearchPage, Error> {
    let page: Paged<Mod> = parse_json(raw)?;
    Ok(SearchPage {
        total: page.pagination.total_count,
        projects: page.data.into_iter().map(project).collect(),
    })
}

pub(crate) fn parse_files(raw: &str) -> Result<Vec<ProjectVersion>, Error> {
    let page: Paged<File> = parse_json(raw)?;
    let mut versions: Vec<_> = page.data.into_iter().map(version).collect();
    versions.sort_by_key(|v| std::cmp::Reverse(v.published_ms));
    Ok(versions)
}

impl Catalog {
    fn curseforge(&self, key: &str, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}/v1{path}", self.endpoints.curseforge))
            .header("x-api-key", key)
    }

    /// Checks that `key` works.
    pub async fn curseforge_check_key(&self, key: &str) -> Result<(), Error> {
        match self
            .fetch_text(self.curseforge(key, &format!("/games/{MINECRAFT}")))
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::Upstream(e)) if e.contains("403") || e.contains("401") => Err(
                Error::NotConfigured("CurseForge didn't accept that API key".into()),
            ),
            Err(e) => Err(e),
        }
    }

    /// Searches CurseForge.
    pub async fn curseforge_search(
        &self,
        key: &str,
        kind: ProjectKind,
        query: &SearchQuery,
    ) -> Result<SearchPage, Error> {
        let class = match kind {
            ProjectKind::Modpack => CLASS_MODPACK,
            ProjectKind::Mod => CLASS_MOD,
        };
        let mut params = vec![
            ("gameId", MINECRAFT.to_string()),
            ("classId", class.to_string()),
            // 2: popularity.
            ("sortField", "2".into()),
            ("sortOrder", "desc".into()),
            ("pageSize", query.limit.clamp(1, 50).to_string()),
            ("index", query.offset.to_string()),
        ];
        if !query.text.trim().is_empty() {
            params.push(("searchFilter", query.text.trim().to_owned()));
        }
        if let Some(v) = query.mc_version.as_deref().filter(|v| !v.is_empty()) {
            params.push(("gameVersion", v.to_owned()));
        }
        if let Some(l) = query.loader.as_deref().and_then(loader_type) {
            params.push(("modLoaderType", l.to_string()));
        }
        let raw = self
            .fetch_text(self.curseforge(key, "/mods/search").query(&params))
            .await?;
        parse_search(&raw)
    }

    /// One CurseForge project, by numeric ID.
    pub async fn curseforge_project(&self, key: &str, id: &str) -> Result<Project, Error> {
        let raw = self
            .fetch_text(self.curseforge(key, &format!("/mods/{}", enc(id))))
            .await?;
        Ok(project(parse_json::<Data<Mod>>(&raw)?.data))
    }

    /// A project's files, newest first, optionally only for one Minecraft
    /// version and loader.
    pub async fn curseforge_versions(
        &self,
        key: &str,
        project: &str,
        mc_version: Option<&str>,
        loader: Option<&str>,
    ) -> Result<Vec<ProjectVersion>, Error> {
        let mut params = vec![("pageSize", "50".to_owned())];
        if let Some(v) = mc_version.filter(|v| !v.is_empty()) {
            params.push(("gameVersion", v.to_owned()));
        }
        if let Some(l) = loader.and_then(loader_type) {
            params.push(("modLoaderType", l.to_string()));
        }
        let raw = self
            .fetch_text(
                self.curseforge(key, &format!("/mods/{}/files", enc(project)))
                    .query(&params),
            )
            .await?;
        parse_files(&raw)
    }

    /// One file of a project.
    pub async fn curseforge_version(
        &self,
        key: &str,
        project: &str,
        file: &str,
    ) -> Result<ProjectVersion, Error> {
        let raw = self
            .fetch_text(
                self.curseforge(key, &format!("/mods/{}/files/{}", enc(project), enc(file))),
            )
            .await?;
        Ok(version(parse_json::<Data<File>>(&raw)?.data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn search_results() {
        let page = parse_search(&fixture("curseforge-search.json")).unwrap();
        assert_eq!(page.total, 4321);
        let p = &page.projects[0];
        assert_eq!(
            (p.platform, p.id.as_str(), p.slug.as_str()),
            (Platform::CurseForge, "285109", "all-the-mods-9")
        );
        assert_eq!(p.loaders, ["forge"]);
        assert_eq!(p.game_versions, ["1.20.1"]);
        assert_eq!(p.author, "ATMTeam");
        assert!(
            p.icon_url
                .as_deref()
                .unwrap()
                .starts_with("https://media.forgecdn.net/")
        );
    }

    #[test]
    fn files_split_versions_from_loaders_and_flag_manual_downloads() {
        let files = parse_files(&fixture("curseforge-files.json")).unwrap();
        assert_eq!(files[0].id, "5000001");
        assert_eq!(files[0].game_versions, ["1.20.1"]);
        assert_eq!(files[0].loaders, ["forge"]);
        assert_eq!(
            files[0].files[0].sha1.as_deref(),
            Some("da39a3ee5e6b4b0d3255bfef95601890afd80709")
        );
        assert_eq!(files[1].loaders, ["neoforge"]);
        assert_eq!(files[1].channel, "beta");
        assert!(
            files[1].files[0].url.is_none(),
            "not allowed for third-party download"
        );
    }
}
