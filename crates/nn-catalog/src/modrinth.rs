//! Modrinth (public API, no key): <https://docs.modrinth.com/api/>.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::json;

use crate::content::{
    Dependency, MOD_LOADERS, Platform, Project, ProjectKind, ProjectVersion, SearchPage,
    SearchQuery, VersionFile, millis,
};
use crate::versions::parse_json;
use crate::{Catalog, Error};

#[derive(Deserialize)]
struct Hit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    author: String,
    icon_url: Option<String>,
    downloads: u64,
    categories: Vec<String>,
    versions: Vec<String>,
    project_type: String,
    server_side: Option<String>,
    date_modified: String,
}

#[derive(Deserialize)]
struct FullProject {
    id: String,
    slug: String,
    title: String,
    description: String,
    icon_url: Option<String>,
    downloads: u64,
    loaders: Vec<String>,
    game_versions: Vec<String>,
    project_type: String,
    server_side: Option<String>,
    updated: String,
}

#[derive(Deserialize)]
struct Version {
    id: String,
    project_id: String,
    name: String,
    version_number: String,
    game_versions: Vec<String>,
    loaders: Vec<String>,
    version_type: String,
    date_published: String,
    files: Vec<File>,
    #[serde(default)]
    dependencies: Vec<Dep>,
}

#[derive(Deserialize)]
struct Dep {
    project_id: Option<String>,
    version_id: Option<String>,
    dependency_type: String,
}

#[derive(Deserialize)]
struct File {
    url: String,
    filename: String,
    size: u64,
    primary: bool,
    hashes: HashMap<String, String>,
}

fn page_url(kind: &str, slug: &str) -> String {
    format!("https://modrinth.com/{kind}/{slug}")
}

fn version(v: Version) -> ProjectVersion {
    ProjectVersion {
        id: v.id,
        project_id: v.project_id,
        name: v.name,
        version_number: v.version_number,
        game_versions: v.game_versions,
        loaders: v.loaders,
        channel: v.version_type,
        published_ms: millis(&v.date_published),
        files: v
            .files
            .into_iter()
            .map(|f| VersionFile {
                url: Some(f.url),
                filename: f.filename,
                size: f.size,
                sha1: f.hashes.get("sha1").cloned(),
                sha512: f.hashes.get("sha512").cloned(),
                primary: f.primary,
            })
            .collect(),
        dependencies: v
            .dependencies
            .into_iter()
            .map(|d| Dependency {
                project_id: d.project_id,
                version_id: d.version_id,
                required: d.dependency_type == "required",
            })
            .collect(),
    }
}

pub(crate) fn parse_search(raw: &str) -> Result<SearchPage, Error> {
    #[derive(Deserialize)]
    struct Page {
        hits: Vec<Hit>,
        total_hits: u64,
    }
    let page: Page = parse_json(raw)?;
    Ok(SearchPage {
        total: page.total_hits,
        projects: page
            .hits
            .into_iter()
            .map(|h| Project {
                platform: Platform::Modrinth,
                page_url: page_url(&h.project_type, &h.slug),
                loaders: h
                    .categories
                    .iter()
                    .filter(|c| MOD_LOADERS.contains(&c.as_str()))
                    .cloned()
                    .collect(),
                id: h.project_id,
                slug: h.slug,
                title: h.title,
                summary: h.description,
                author: h.author,
                icon_url: h.icon_url.filter(|u| !u.is_empty()),
                downloads: h.downloads,
                game_versions: h.versions,
                server_side: h.server_side.unwrap_or_else(|| "unknown".into()),
                updated_ms: millis(&h.date_modified),
            })
            .collect(),
    })
}

pub(crate) fn parse_versions(raw: &str) -> Result<Vec<ProjectVersion>, Error> {
    Ok(parse_json::<Vec<Version>>(raw)?
        .into_iter()
        .map(version)
        .collect())
}

/// Builds Modrinth's facet filter (an AND of OR-groups).
fn facets(kind: ProjectKind, query: &SearchQuery) -> String {
    let mut groups = vec![vec![format!(
        "project_type:{}",
        match kind {
            ProjectKind::Modpack => "modpack",
            ProjectKind::Mod => "mod",
        }
    )]];
    // Content that can't run on a server is no use here.
    groups.push(vec![
        "server_side:required".into(),
        "server_side:optional".into(),
    ]);
    if let Some(v) = query.mc_version.as_deref().filter(|v| !v.is_empty()) {
        groups.push(vec![format!("versions:{v}")]);
    }
    if let Some(l) = query.loader.as_deref().filter(|l| !l.is_empty()) {
        groups.push(vec![format!("categories:{l}")]);
    }
    json!(groups).to_string()
}

impl Catalog {
    /// Searches Modrinth.
    pub async fn modrinth_search(
        &self,
        kind: ProjectKind,
        query: &SearchQuery,
    ) -> Result<SearchPage, Error> {
        let limit = query.limit.clamp(1, 50).to_string();
        let offset = query.offset.to_string();
        let facets = facets(kind, query);
        let index = if query.text.trim().is_empty() {
            "downloads"
        } else {
            "relevance"
        };
        let request = self
            .http
            .get(format!("{}/v2/search", self.endpoints.modrinth))
            .query(&[
                ("query", query.text.trim()),
                ("facets", facets.as_str()),
                ("index", index),
                ("limit", limit.as_str()),
                ("offset", offset.as_str()),
            ]);
        parse_search(&self.fetch_text(request).await?)
    }

    /// One Modrinth project, by ID or slug.
    pub async fn modrinth_project(&self, id: &str) -> Result<Project, Error> {
        let request = self.http.get(format!(
            "{}/v2/project/{}",
            self.endpoints.modrinth,
            crate::versions::enc(id)
        ));
        let p: FullProject = parse_json(&self.fetch_text(request).await?)?;
        Ok(Project {
            platform: Platform::Modrinth,
            page_url: page_url(&p.project_type, &p.slug),
            id: p.id,
            slug: p.slug,
            title: p.title,
            summary: p.description,
            // Full project records name a team, not a person.
            author: String::new(),
            icon_url: p.icon_url.filter(|u| !u.is_empty()),
            downloads: p.downloads,
            loaders: p.loaders,
            game_versions: p.game_versions,
            server_side: p.server_side.unwrap_or_else(|| "unknown".into()),
            updated_ms: millis(&p.updated),
        })
    }

    /// A project's versions, newest first, optionally only for one
    /// Minecraft version and loader.
    pub async fn modrinth_versions(
        &self,
        project: &str,
        mc_version: Option<&str>,
        loader: Option<&str>,
    ) -> Result<Vec<ProjectVersion>, Error> {
        let mut request = self.http.get(format!(
            "{}/v2/project/{}/version",
            self.endpoints.modrinth,
            crate::versions::enc(project)
        ));
        if let Some(v) = mc_version.filter(|v| !v.is_empty()) {
            request = request.query(&[("game_versions", json!([v]).to_string())]);
        }
        if let Some(l) = loader.filter(|l| !l.is_empty()) {
            request = request.query(&[("loaders", json!([l]).to_string())]);
        }
        parse_versions(&self.fetch_text(request).await?)
    }

    /// One version, by ID.
    pub async fn modrinth_version(&self, id: &str) -> Result<ProjectVersion, Error> {
        let request = self.http.get(format!(
            "{}/v2/version/{}",
            self.endpoints.modrinth,
            crate::versions::enc(id)
        ));
        Ok(version(parse_json(&self.fetch_text(request).await?)?))
    }

    /// Identifies files by SHA-1: the version each one is, for those
    /// Modrinth knows.
    pub async fn modrinth_identify(
        &self,
        sha1s: &[String],
    ) -> Result<HashMap<String, ProjectVersion>, Error> {
        if sha1s.is_empty() {
            return Ok(HashMap::new());
        }
        let request = self
            .http
            .post(format!("{}/v2/version_files", self.endpoints.modrinth))
            .json(&json!({ "hashes": sha1s, "algorithm": "sha1" }));
        let found: HashMap<String, Version> = parse_json(&self.fetch_text(request).await?)?;
        Ok(found
            .into_iter()
            .map(|(hash, v)| (hash, version(v)))
            .collect())
    }

    /// The newest version of each file's project for this Minecraft version
    /// and loader, keyed by the file's SHA-1.
    pub async fn modrinth_updates(
        &self,
        sha1s: &[String],
        mc_version: &str,
        loader: &str,
    ) -> Result<HashMap<String, ProjectVersion>, Error> {
        if sha1s.is_empty() {
            return Ok(HashMap::new());
        }
        let request = self
            .http
            .post(format!(
                "{}/v2/version_files/update",
                self.endpoints.modrinth
            ))
            .json(&json!({
                "hashes": sha1s,
                "algorithm": "sha1",
                "loaders": [loader],
                "game_versions": [mc_version],
            }));
        let found: HashMap<String, Version> = parse_json(&self.fetch_text(request).await?)?;
        Ok(found
            .into_iter()
            .map(|(hash, v)| (hash, version(v)))
            .collect())
    }

    /// Several projects at once (names and icons for installed mods).
    pub async fn modrinth_projects(&self, ids: &[String]) -> Result<Vec<Project>, Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let request = self
            .http
            .get(format!("{}/v2/projects", self.endpoints.modrinth))
            .query(&[("ids", json!(ids).to_string())]);
        let projects: Vec<FullProject> = parse_json(&self.fetch_text(request).await?)?;
        Ok(projects
            .into_iter()
            .map(|p| Project {
                platform: Platform::Modrinth,
                page_url: page_url(&p.project_type, &p.slug),
                id: p.id,
                slug: p.slug,
                title: p.title,
                summary: p.description,
                // Full project records name a team, not a person.
                author: String::new(),
                icon_url: p.icon_url.filter(|u| !u.is_empty()),
                downloads: p.downloads,
                loaders: p.loaders,
                game_versions: p.game_versions,
                server_side: p.server_side.unwrap_or_else(|| "unknown".into()),
                updated_ms: millis(&p.updated),
            })
            .collect())
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
        let page = parse_search(&fixture("modrinth-search.json")).unwrap();
        assert!(page.total > 100);
        let first = &page.projects[0];
        assert_eq!(first.platform, Platform::Modrinth);
        assert!(first.page_url.starts_with("https://modrinth.com/modpack/"));
        assert!(
            first
                .loaders
                .iter()
                .all(|l| MOD_LOADERS.contains(&l.as_str()))
        );
        assert!(first.updated_ms > 0);
    }

    #[test]
    fn versions_with_hashes() {
        let versions = parse_versions(&fixture("modrinth-versions.json")).unwrap();
        let file = versions[0].primary_file().unwrap();
        assert!(file.filename.ends_with(".mrpack"));
        assert_eq!(file.sha1.as_ref().map(String::len), Some(40));
        assert_eq!(file.sha512.as_ref().map(String::len), Some(128));
        assert_eq!(versions[0].loaders, ["fabric"]);
    }

    #[test]
    fn facets_filter_to_server_content() {
        let f = facets(
            ProjectKind::Mod,
            &SearchQuery {
                mc_version: Some("1.20.1".into()),
                loader: Some("fabric".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            f,
            r#"[["project_type:mod"],["server_side:required","server_side:optional"],["versions:1.20.1"],["categories:fabric"]]"#
        );
    }
}
