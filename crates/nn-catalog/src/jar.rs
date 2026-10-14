//! What's inside a mod jar: name, version, loader, and icon, from
//! `fabric.mod.json`, `quilt.mod.json`, `META-INF/neoforge.mods.toml`, or
//! `META-INF/mods.toml`. Jars are untrusted: entries are read with size
//! limits and parse failures just mean "unknown".

use std::io::{Read, Seek};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::Digest;

/// Largest metadata file read from a jar.
const MAX_METADATA: u64 = 256 * 1024;
/// Largest icon read from a jar.
pub const MAX_ICON: u64 = 1024 * 1024;

/// A mod's own description of itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModInfo {
    /// Mod ID (`sodium`, `create`).
    pub id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Version.
    pub version: Option<String>,
    /// One-line description.
    pub description: Option<String>,
    /// Authors.
    pub authors: Vec<String>,
    /// `fabric`, `quilt`, `forge`, or `neoforge`, from which metadata file
    /// it has.
    pub loader: Option<String>,
    /// Path of the icon inside the jar.
    pub icon: Option<String>,
    /// Fabric/Quilt `environment`: `client`, `server`, or `*`.
    pub environment: Option<String>,
}

fn read_entry<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Option<Vec<u8>> {
    let entry = zip.by_name(name).ok()?;
    if entry.size() > limit {
        return None;
    }
    let mut data = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
    entry.take(limit).read_to_end(&mut data).ok()?;
    Some(data)
}

/// Reads a jar's mod metadata. `None` if it isn't a readable zip.
pub fn read_mod_info<R: Read + Seek>(reader: R) -> Option<ModInfo> {
    let mut zip = zip::ZipArchive::new(reader).ok()?;
    let manifest_version = read_entry(&mut zip, "META-INF/MANIFEST.MF", MAX_METADATA)
        .and_then(|m| manifest_value(&String::from_utf8_lossy(&m), "Implementation-Version"));
    let mut info = if let Some(raw) = read_entry(&mut zip, "fabric.mod.json", MAX_METADATA) {
        fabric(&raw).map(|i| ModInfo {
            loader: Some("fabric".into()),
            ..i
        })
    } else if let Some(raw) = read_entry(&mut zip, "quilt.mod.json", MAX_METADATA) {
        quilt(&raw).map(|i| ModInfo {
            loader: Some("quilt".into()),
            ..i
        })
    } else if let Some(raw) = read_entry(&mut zip, "META-INF/neoforge.mods.toml", MAX_METADATA) {
        mods_toml(&raw).map(|i| ModInfo {
            loader: Some("neoforge".into()),
            ..i
        })
    } else if let Some(raw) = read_entry(&mut zip, "META-INF/mods.toml", MAX_METADATA) {
        mods_toml(&raw).map(|i| ModInfo {
            loader: Some("forge".into()),
            ..i
        })
    } else {
        None
    }
    .unwrap_or_default();
    // Forge mods often say `${file.jarVersion}`, filled in from the manifest.
    if info.version.as_deref().is_none_or(|v| v.contains("${")) {
        info.version = manifest_version.or(info.version.filter(|v| !v.contains("${")));
    }
    Some(info)
}

/// Reads the icon at `path` inside a jar, if it's there and small enough.
pub fn read_icon<R: Read + Seek>(reader: R, path: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(reader).ok()?;
    read_entry(&mut zip, path.trim_start_matches('/'), MAX_ICON)
}

/// SHA-1 of a whole file, hex (how Modrinth identifies files).
pub fn sha1_hex<R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut hasher = sha1::Sha1::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(300).collect())
}

fn people(v: Option<&Value>) -> Vec<String> {
    let names: Vec<String> = match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|a| text(Some(a)).or_else(|| text(a.get("name"))))
            .collect(),
        // Quilt: { "name": "role" }.
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    names.into_iter().take(10).collect()
}

/// An icon field: a path, or sizes mapped to paths (pick the largest up to
/// 128 px, else the smallest).
fn icon(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(path) => Some(path.clone()),
        Value::Object(sizes) => {
            let mut options: Vec<(u32, &str)> = sizes
                .iter()
                .filter_map(|(size, path)| Some((size.parse().ok()?, path.as_str()?)))
                .collect();
            options.sort_unstable();
            options
                .iter()
                .rev()
                .find(|(size, _)| *size <= 128)
                .or(options.first())
                .map(|(_, path)| (*path).to_owned())
        }
        _ => None,
    }
}

fn fabric(raw: &[u8]) -> Option<ModInfo> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    Some(ModInfo {
        id: text(v.get("id")),
        name: text(v.get("name")),
        version: text(v.get("version")),
        description: text(v.get("description")),
        authors: people(v.get("authors")),
        icon: icon(v.get("icon")),
        environment: text(v.get("environment")),
        loader: None,
    })
}

fn quilt(raw: &[u8]) -> Option<ModInfo> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    let loader = v.get("quilt_loader")?;
    let meta = loader.get("metadata");
    Some(ModInfo {
        id: text(loader.get("id")),
        name: text(meta.and_then(|m| m.get("name"))),
        version: text(loader.get("version")),
        description: text(meta.and_then(|m| m.get("description"))),
        authors: people(meta.and_then(|m| m.get("contributors"))),
        icon: icon(meta.and_then(|m| m.get("icon"))),
        environment: text(v.get("minecraft").and_then(|m| m.get("environment"))),
        loader: None,
    })
}

fn mods_toml(raw: &[u8]) -> Option<ModInfo> {
    let doc: toml::Table = toml::from_str(&String::from_utf8_lossy(raw)).ok()?;
    let first = doc.get("mods")?.as_array()?.first()?.as_table()?;
    let s = |t: &toml::Table, key: &str| {
        t.get(key)
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(300).collect::<String>())
    };
    Some(ModInfo {
        id: s(first, "modId"),
        name: s(first, "displayName"),
        version: s(first, "version"),
        description: s(first, "description"),
        authors: s(first, "authors")
            .or_else(|| s(&doc, "authors"))
            .map(|a| {
                a.split(',')
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .take(10)
                    .collect()
            })
            .unwrap_or_default(),
        icon: s(first, "logoFile").or_else(|| s(&doc, "logoFile")),
        environment: None,
        loader: None,
    })
}

fn manifest_value(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key)
            .then(|| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use super::*;

    fn jar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            for (name, data) in files {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        out
    }

    #[test]
    fn fabric_mods() {
        let raw = br#"{"schemaVersion":1,"id":"sodium","version":"0.6.0","name":"Sodium","description":"Fast",
            "authors":["JellySquid",{"name":"IMS"}],"icon":{"16":"a16.png","128":"a128.png","256":"a256.png"},"environment":"client"}"#;
        let info = read_mod_info(Cursor::new(jar(&[
            ("fabric.mod.json", raw),
            ("a128.png", b"\x89PNG"),
        ])))
        .unwrap();
        assert_eq!(info.loader.as_deref(), Some("fabric"));
        assert_eq!(info.name.as_deref(), Some("Sodium"));
        assert_eq!(info.authors, ["JellySquid", "IMS"]);
        assert_eq!(info.icon.as_deref(), Some("a128.png"));
        assert_eq!(info.environment.as_deref(), Some("client"));
        let bytes = jar(&[("fabric.mod.json", raw), ("a128.png", b"\x89PNG")]);
        assert_eq!(
            read_icon(Cursor::new(bytes), "a128.png").unwrap(),
            b"\x89PNG"
        );
    }

    #[test]
    fn forge_and_neoforge_mods_with_manifest_versions() {
        let toml = br#"
modLoader="javafml"
loaderVersion="[47,)"
license="MIT"
[[mods]]
modId="create"
version="${file.jarVersion}"
displayName="Create"
authors="simibubi, Others"
logoFile="logo.png"
"#;
        let manifest = b"Manifest-Version: 1.0\r\nImplementation-Version: 0.5.1.f\r\n";
        let info = read_mod_info(Cursor::new(jar(&[
            ("META-INF/mods.toml", toml),
            ("META-INF/MANIFEST.MF", manifest),
        ])))
        .unwrap();
        assert_eq!(info.loader.as_deref(), Some("forge"));
        assert_eq!(info.id.as_deref(), Some("create"));
        assert_eq!(info.version.as_deref(), Some("0.5.1.f"));
        assert_eq!(info.authors, ["simibubi", "Others"]);
        assert_eq!(info.icon.as_deref(), Some("logo.png"));
        let neo =
            read_mod_info(Cursor::new(jar(&[("META-INF/neoforge.mods.toml", toml)]))).unwrap();
        assert_eq!(neo.loader.as_deref(), Some("neoforge"));
        assert_eq!(neo.version, None, "an unfilled placeholder isn't a version");
    }

    #[test]
    fn quilt_and_unknown_and_broken() {
        let raw = br#"{"schema_version":1,"quilt_loader":{"id":"qsl","version":"7.0.0","metadata":{"name":"QSL","contributors":{"Ennui":"Owner"}}}}"#;
        let info = read_mod_info(Cursor::new(jar(&[("quilt.mod.json", raw)]))).unwrap();
        assert_eq!(
            (info.loader.as_deref(), info.name.as_deref()),
            (Some("quilt"), Some("QSL"))
        );
        assert_eq!(info.authors, ["Ennui"]);
        let plain = read_mod_info(Cursor::new(jar(&[("a.class", b"")]))).unwrap();
        assert_eq!(plain, ModInfo::default());
        assert!(read_mod_info(Cursor::new(b"not a zip".to_vec())).is_none());
        let bad = read_mod_info(Cursor::new(jar(&[("fabric.mod.json", b"{not json")]))).unwrap();
        assert_eq!(bad.loader, None);
    }

    #[test]
    fn oversized_entries_are_skipped() {
        let huge = vec![b' '; (MAX_METADATA + 1) as usize];
        let info = read_mod_info(Cursor::new(jar(&[("fabric.mod.json", &huge)]))).unwrap();
        assert_eq!(info, ModInfo::default());
    }

    #[test]
    fn sha1_of_files() {
        assert_eq!(
            sha1_hex(Cursor::new(b"")).unwrap(),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
    }
}
