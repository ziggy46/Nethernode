//! Minecraft versions (Mojang) and server software builds (Fabric, Quilt,
//! Forge, NeoForge, Paper, Purpur).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{Catalog, Error};

/// A Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McVersion {
    /// `1.21.11`, `26.3`, `26.4-snapshot-4`.
    pub id: String,
    /// Snapshots, pre-releases, and release candidates.
    pub snapshot: bool,
    /// Release time, unix ms.
    pub released_ms: i64,
}

/// A build of server software for one Minecraft version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderVersion {
    /// What to pin, e.g. `0.19.5` (Fabric), `47.3.0` (Forge), `132` (Paper).
    pub version: String,
    /// Not a beta or experimental build.
    pub stable: bool,
    /// The one to pick by default.
    pub recommended: bool,
}

/// Server software NetherNode can install, by loader id.
pub const LOADERS: &[&str] = &[
    "vanilla", "paper", "purpur", "fabric", "quilt", "forge", "neoforge",
];

impl Catalog {
    /// Minecraft versions `loader` supports, newest first.
    pub async fn mc_versions(
        &self,
        loader: &str,
        snapshots: bool,
    ) -> Result<Vec<McVersion>, Error> {
        let all = self.mojang().await?;
        let supported: Option<HashSet<String>> = match loader {
            "vanilla" => None,
            "paper" => Some(self.paper_versions().await?),
            "purpur" => Some(self.purpur_versions().await?),
            "fabric" => Some(self.fabric_games().await?),
            "quilt" => Some(self.quilt_games().await?),
            "forge" => Some(forge_mc_versions(&self.forge_maven().await?)),
            "neoforge" => Some(
                neoforge_by_mc(&self.neoforge_maven().await?)
                    .into_keys()
                    .collect(),
            ),
            other => {
                return Err(Error::Unsupported(format!(
                    "unknown server software {other}"
                )));
            }
        };
        Ok(all
            .into_iter()
            .filter(|v| snapshots || !v.snapshot)
            .filter(|v| supported.as_ref().is_none_or(|s| s.contains(&v.id)))
            .collect())
    }

    /// Builds of `loader` for Minecraft `mc`, newest first. Empty when the
    /// software has no builds for that version.
    pub async fn loader_versions(
        &self,
        loader: &str,
        mc: &str,
    ) -> Result<Vec<LoaderVersion>, Error> {
        let mut out = match loader {
            "vanilla" => Vec::new(),
            "fabric" => {
                let raw = self
                    .cached_text(
                        &format!("fabric-loader-{mc}"),
                        &format!("{}/v2/versions/loader/{}", self.endpoints.fabric, enc(mc)),
                    )
                    .await?;
                parse_meta_loaders(&raw)?
            }
            "quilt" => {
                let raw = self
                    .cached_text(
                        &format!("quilt-loader-{mc}"),
                        &format!("{}/v3/versions/loader/{}", self.endpoints.quilt, enc(mc)),
                    )
                    .await?;
                let mut versions = parse_meta_loaders(&raw)?;
                versions.sort_by(|a, b| compare_versions(&b.version, &a.version));
                versions
            }
            "forge" => {
                let promos = self
                    .cached_text("forge-promotions", &self.endpoints.forge_promotions)
                    .await?;
                forge_builds(&self.forge_maven().await?, &promos, mc)?
            }
            "neoforge" => {
                let mut versions = neoforge_by_mc(&self.neoforge_maven().await?)
                    .remove(mc)
                    .unwrap_or_default();
                versions.sort_by(|a, b| compare_versions(b, a));
                versions
                    .into_iter()
                    .map(|v| LoaderVersion {
                        stable: !is_prerelease(&v),
                        version: v,
                        recommended: false,
                    })
                    .collect()
            }
            "paper" => {
                let raw = self
                    .cached_text(
                        &format!("paper-builds-{mc}"),
                        &format!(
                            "{}/v3/projects/paper/versions/{}/builds",
                            self.endpoints.paper,
                            enc(mc)
                        ),
                    )
                    .await?;
                parse_paper_builds(&raw)?
            }
            "purpur" => {
                let raw = self
                    .cached_text(
                        &format!("purpur-builds-{mc}"),
                        &format!("{}/v2/purpur/{}", self.endpoints.purpur, enc(mc)),
                    )
                    .await?;
                parse_purpur_builds(&raw)?
            }
            other => {
                return Err(Error::Unsupported(format!(
                    "unknown server software {other}"
                )));
            }
        };
        if !out.iter().any(|v| v.recommended)
            && let Some(first) = out.iter_mut().find(|v| v.stable)
        {
            first.recommended = true;
        }
        Ok(out)
    }

    async fn mojang(&self) -> Result<Vec<McVersion>, Error> {
        let raw = self.cached_text("mojang", &self.endpoints.mojang).await?;
        parse_mojang(&raw)
    }

    async fn paper_versions(&self) -> Result<HashSet<String>, Error> {
        let raw = self
            .cached_text(
                "paper",
                &format!("{}/v3/projects/paper", self.endpoints.paper),
            )
            .await?;
        parse_paper_versions(&raw)
    }

    async fn purpur_versions(&self) -> Result<HashSet<String>, Error> {
        let raw = self
            .cached_text("purpur", &format!("{}/v2/purpur", self.endpoints.purpur))
            .await?;
        #[derive(Deserialize)]
        struct Project {
            versions: Vec<String>,
        }
        Ok(parse_json::<Project>(&raw)?.versions.into_iter().collect())
    }

    async fn fabric_games(&self) -> Result<HashSet<String>, Error> {
        let raw = self
            .cached_text(
                "fabric-game",
                &format!("{}/v2/versions/game", self.endpoints.fabric),
            )
            .await?;
        parse_game_list(&raw)
    }

    async fn quilt_games(&self) -> Result<HashSet<String>, Error> {
        let raw = self
            .cached_text(
                "quilt-game",
                &format!("{}/v3/versions/game", self.endpoints.quilt),
            )
            .await?;
        parse_game_list(&raw)
    }

    async fn forge_maven(&self) -> Result<Vec<String>, Error> {
        Ok(maven_versions(
            &self
                .cached_text("forge-maven", &self.endpoints.forge_maven)
                .await?,
        ))
    }

    async fn neoforge_maven(&self) -> Result<Vec<String>, Error> {
        Ok(maven_versions(
            &self
                .cached_text("neoforge-maven", &self.endpoints.neoforge_maven)
                .await?,
        ))
    }
}

pub(crate) fn enc(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<T, Error> {
    serde_json::from_str(raw).map_err(|e| Error::Upstream(format!("unexpected answer: {e}")))
}

fn parse_mojang(raw: &str) -> Result<Vec<McVersion>, Error> {
    #[derive(Deserialize)]
    struct Manifest {
        versions: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        #[serde(rename = "type")]
        kind: String,
        #[serde(rename = "releaseTime")]
        release_time: String,
    }
    let manifest: Manifest = parse_json(raw)?;
    Ok(manifest
        .versions
        .into_iter()
        // Old alphas and betas can't run as servers in the image.
        .filter(|v| v.kind == "release" || v.kind == "snapshot")
        .map(|v| McVersion {
            snapshot: v.kind == "snapshot",
            released_ms: v
                .release_time
                .parse::<jiff::Timestamp>()
                .map(|t| t.as_millisecond())
                .unwrap_or(0),
            id: v.id,
        })
        .collect())
}

fn parse_game_list(raw: &str) -> Result<HashSet<String>, Error> {
    #[derive(Deserialize)]
    struct Game {
        version: String,
    }
    Ok(parse_json::<Vec<Game>>(raw)?
        .into_iter()
        .map(|g| g.version)
        .collect())
}

fn parse_meta_loaders(raw: &str) -> Result<Vec<LoaderVersion>, Error> {
    #[derive(Deserialize)]
    struct Entry {
        loader: Loader,
    }
    #[derive(Deserialize)]
    struct Loader {
        version: String,
        stable: Option<bool>,
    }
    Ok(parse_json::<Vec<Entry>>(raw)?
        .into_iter()
        .map(|e| LoaderVersion {
            stable: e
                .loader
                .stable
                .unwrap_or_else(|| !is_prerelease(&e.loader.version)),
            version: e.loader.version,
            recommended: false,
        })
        .collect())
}

/// `<version>` values from Maven metadata, in file order.
fn maven_versions(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<version>") {
        rest = &rest[start + "<version>".len()..];
        let Some(end) = rest.find("</version>") else {
            break;
        };
        out.push(rest[..end].trim().to_owned());
        rest = &rest[end..];
    }
    out
}

/// Minecraft versions with Forge builds (`1.20.1-47.3.0` → `1.20.1`).
fn forge_mc_versions(versions: &[String]) -> HashSet<String> {
    versions
        .iter()
        .filter_map(|v| v.split_once('-').map(|(mc, _)| mc.to_owned()))
        .collect()
}

fn forge_builds(
    versions: &[String],
    promotions: &str,
    mc: &str,
) -> Result<Vec<LoaderVersion>, Error> {
    #[derive(Deserialize)]
    struct Promotions {
        promos: HashMap<String, String>,
    }
    let promos = parse_json::<Promotions>(promotions)?.promos;
    let recommended = promos.get(&format!("{mc}-recommended"));
    let prefix = format!("{mc}-");
    let mut builds: Vec<String> = versions
        .iter()
        .filter_map(|v| v.strip_prefix(&prefix))
        // Old builds repeat the Minecraft version: `10.13.4.1614-1.7.10`.
        .map(|b| b.strip_suffix(&format!("-{mc}")).unwrap_or(b).to_owned())
        .collect();
    builds.sort_by(|a, b| compare_versions(b, a));
    builds.dedup();
    Ok(builds
        .into_iter()
        .map(|b| LoaderVersion {
            stable: true,
            recommended: recommended == Some(&b),
            version: b,
        })
        .collect())
}

/// NeoForge builds grouped by Minecraft version. Before Minecraft 26 the
/// build's first two numbers are the Minecraft version without its `1.`
/// (`21.11.42` → 1.21.11, `20.4.1` → 1.20.4); from 26 on, the first three
/// are (`26.1.2.115` → 26.1.2, `26.3.0.76` → 26.3). Snapshot builds are
/// left out.
fn neoforge_by_mc(versions: &[String]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for v in versions.iter().filter(|v| !v.contains("+snapshot")) {
        let numbers: Vec<u32> = v
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map_while(|p| p.parse().ok())
            .collect();
        let mc = match numbers.as_slice() {
            [major, minor, _] if *major < 26 => {
                if *minor == 0 {
                    format!("1.{major}")
                } else {
                    format!("1.{major}.{minor}")
                }
            }
            [major, minor, patch, _] if *major >= 26 => {
                if *patch == 0 {
                    format!("{major}.{minor}")
                } else {
                    format!("{major}.{minor}.{patch}")
                }
            }
            _ => continue,
        };
        out.entry(mc).or_default().push(v.clone());
    }
    out
}

fn parse_paper_versions(raw: &str) -> Result<HashSet<String>, Error> {
    #[derive(Deserialize)]
    struct Project {
        versions: HashMap<String, Vec<String>>,
    }
    Ok(parse_json::<Project>(raw)?
        .versions
        .into_values()
        .flatten()
        .collect())
}

fn parse_paper_builds(raw: &str) -> Result<Vec<LoaderVersion>, Error> {
    #[derive(Deserialize)]
    struct Build {
        id: u64,
        channel: String,
    }
    let mut builds: Vec<Build> = parse_json(raw)?;
    builds.sort_by_key(|b| std::cmp::Reverse(b.id));
    Ok(builds
        .into_iter()
        .map(|b| LoaderVersion {
            version: b.id.to_string(),
            stable: b.channel.eq_ignore_ascii_case("stable"),
            recommended: false,
        })
        .collect())
}

fn parse_purpur_builds(raw: &str) -> Result<Vec<LoaderVersion>, Error> {
    #[derive(Deserialize)]
    struct Project {
        builds: Builds,
    }
    #[derive(Deserialize)]
    struct Builds {
        latest: String,
        all: Vec<String>,
    }
    let builds = parse_json::<Project>(raw)?.builds;
    let mut all = builds.all;
    all.sort_by(|a, b| compare_versions(b, a));
    Ok(all
        .into_iter()
        .map(|b| LoaderVersion {
            recommended: b == builds.latest,
            version: b,
            stable: true,
        })
        .collect())
}

fn is_prerelease(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    ["alpha", "beta", "rc", "pre", "snapshot"]
        .iter()
        .any(|w| v.contains(w))
}

/// Compares dotted versions number by number; a release sorts after its
/// pre-releases (`0.20.0` > `0.20.0-beta.9`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |s: &str| -> Vec<String> {
        s.split(['.', '-', '+', '_'])
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (pa, pb) = (parts(a), parts(b));
    for i in 0..pa.len().max(pb.len()) {
        match (pa.get(i), pb.get(i)) {
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    (Ok(_), Err(_)) => Ordering::Greater,
                    (Err(_), Ok(_)) => Ordering::Less,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            // A version that ends where the other goes on with a word
            // (`-beta`) is the release, so it's newer.
            (None, Some(y)) => {
                return if y.parse::<u64>().is_ok() {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            (Some(x), None) => {
                return if x.parse::<u64>().is_ok() {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
            }
            (None, None) => break,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn mojang_versions_newest_first_without_old_alphas() {
        let versions = parse_mojang(&fixture("mojang.json")).unwrap();
        let ids: Vec<_> = versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids.first(), Some(&"26.4-snapshot-4"));
        assert!(versions[0].snapshot);
        assert!(ids.contains(&"1.20.1") && !ids.contains(&"b1.7.3"));
        assert!(
            versions
                .iter()
                .find(|v| v.id == "1.20.1")
                .unwrap()
                .released_ms
                > 1_600_000_000_000
        );
    }

    #[test]
    fn neoforge_maps_both_numbering_schemes() {
        let by_mc = neoforge_by_mc(&maven_versions(&fixture("neoforge-maven.xml")));
        assert!(by_mc["1.21.11"].contains(&"21.11.45".to_owned()));
        assert!(by_mc["26.1.2"].contains(&"26.1.2.116".to_owned()));
        assert!(by_mc["26.3"].contains(&"26.3.0.76".to_owned()));
        assert!(by_mc["26.3"].contains(&"26.3.0.75-beta".to_owned()));
        assert!(
            !by_mc.keys().any(|k| k.contains("snapshot") || k == "26.1"),
            "{by_mc:?}"
        );
        let more = neoforge_by_mc(&["20.4.237".into(), "21.0.1-beta".into()]);
        assert!(more.contains_key("1.20.4") && more.contains_key("1.21"));
    }

    #[test]
    fn forge_builds_with_the_recommended_one() {
        let maven = maven_versions(&fixture("forge-maven.xml"));
        let mcs = forge_mc_versions(&maven);
        assert!(mcs.contains("1.20.1") && mcs.contains("1.7.10"));
        let builds = forge_builds(&maven, &fixture("forge-promotions.json"), "1.20.1").unwrap();
        assert!(!builds.is_empty());
        assert!(
            builds
                .windows(2)
                .all(|w| compare_versions(&w[0].version, &w[1].version).is_ge())
        );
        let old = forge_builds(&maven, r#"{"promos":{}}"#, "1.7.10").unwrap();
        assert_eq!(
            old[0].version, "10.13.4.1614",
            "old builds drop the repeated Minecraft version"
        );
    }

    #[test]
    fn paper_purpur_fabric_quilt() {
        assert!(
            parse_paper_versions(&fixture("paper-project.json"))
                .unwrap()
                .contains("26.3")
        );
        let paper = parse_paper_builds(&fixture("paper-builds.json")).unwrap();
        assert!(
            paper.windows(2).all(
                |w| w[0].version.parse::<u64>().unwrap() > w[1].version.parse::<u64>().unwrap()
            )
        );
        assert!(paper.iter().any(|b| b.stable));
        let purpur = parse_purpur_builds(&fixture("purpur-builds.json")).unwrap();
        assert!(purpur[0].recommended);
        let fabric = parse_meta_loaders(&fixture("fabric-loader.json")).unwrap();
        assert!(fabric[0].stable);
        assert!(
            parse_game_list(&fixture("fabric-game.json"))
                .unwrap()
                .contains("1.20.1")
        );
        let mut quilt = parse_meta_loaders(&fixture("quilt-loader.json")).unwrap();
        quilt.sort_by(|a, b| compare_versions(&b.version, &a.version));
        assert_eq!(quilt[0].version, "0.20.0-beta.9");
        assert!(!quilt[0].stable);
        assert!(
            parse_game_list(&fixture("quilt-game.json"))
                .unwrap()
                .contains("1.20.1")
        );
    }

    #[test]
    fn version_order() {
        use Ordering::*;
        assert_eq!(compare_versions("0.20.0", "0.20.0-beta.9"), Greater);
        assert_eq!(compare_versions("0.20.0-beta.10", "0.20.0-beta.9"), Greater);
        assert_eq!(compare_versions("47.3.0", "47.2.20"), Greater);
        assert_eq!(compare_versions("26.3.0.76", "26.3.0.75-beta"), Greater);
        assert_eq!(compare_versions("1.21", "1.21.1"), Less);
        assert_eq!(compare_versions("2568", "2567"), Greater);
    }

    #[test]
    fn url_segments_are_encoded() {
        assert_eq!(enc("1.20.1"), "1.20.1");
        assert_eq!(enc("a/../b"), "a%2F..%2Fb");
    }
}
