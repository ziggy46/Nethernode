//! An agent updating itself to the panel's version
//! ([`crate::protocol::Request::SelfUpdate`]). In a container it starts a
//! copy on the new image and exits; as a plain binary it downloads the
//! release, checks it against the release's checksums, and runs it in its
//! place.

use std::path::{Path, PathBuf};
use std::time::Duration;

use nn_orchestrator::self_update::{self, Replaced};
use sha2::{Digest, Sha256};

use crate::Node;

/// Where releases are downloaded from, unless `NETHERNODE_RELEASES_URL`
/// says otherwise (a mirror, or a test server).
const RELEASES: &str = "https://github.com/ziggy46/Nethernode/releases/download";

/// Largest release archive accepted.
const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;

/// What to do once the panel has been told how it went.
#[derive(Debug)]
pub enum Next {
    /// Nothing: already on that version.
    Stay(String),
    /// Exit: a new container has taken over.
    Exit(String),
    /// Run this binary in place of this process.
    Exec(PathBuf, String),
}

impl Next {
    /// What to tell the panel.
    pub fn message(&self) -> &str {
        match self {
            Next::Stay(m) | Next::Exit(m) | Next::Exec(_, m) => m,
        }
    }
}

/// Whether `version` looks like `1.2.3`.
pub fn valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Gets ready to run `version`, from `current`.
pub async fn prepare(node: &Node, current: &str, version: &str) -> Result<Next, String> {
    if !valid_version(version) {
        return Err(format!("{version:?} isn't a NetherNode version"));
    }
    if version == current {
        return Ok(Next::Stay(format!("Already on {version}.")));
    }
    if let Some(id) = self_update::own_container() {
        let docker = match node {
            Node::Local(local) => local.orchestrator().ok().map(|o| o.docker().clone()),
            Node::Remote(_) => None,
        }
        .ok_or("Docker isn't reachable, so this agent can't replace its container")?;
        return match self_update::replace(&docker, &id, version).await? {
            Replaced::Current => Ok(Next::Stay(format!("Already on {version}."))),
            Replaced::Started { image } => Ok(Next::Exit(format!(
                "Started {image}; the agent reconnects in a moment."
            ))),
        };
    }
    if Path::new("/.dockerenv").exists() {
        return Err(
            "this agent runs in a container it can't identify; update it by hand once".into(),
        );
    }
    let exe = download(version).await?;
    Ok(Next::Exec(
        exe,
        format!("Installed {version}; the agent restarts in a moment."),
    ))
}

/// Downloads release `version` for this machine and puts its binary in
/// place of this one. Returns the binary's path.
async fn download(version: &str) -> Result<PathBuf, String> {
    let arch = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "amd64",
        ("linux", "aarch64") => "arm64",
        (os, arch) => {
            return Err(format!(
                "there are no NetherNode releases for {os} on {arch}"
            ));
        }
    };
    let base = std::env::var("NETHERNODE_RELEASES_URL").unwrap_or_else(|_| RELEASES.to_owned());
    let base = format!("{}/v{version}", base.trim_end_matches('/'));
    let file = format!("nethernode-v{version}-linux-{arch}.tar.gz");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let fetch = async |url: String| {
        let response = client
            .get(&url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| format!("couldn't download {url}: {e}"))?;
        if response.content_length().is_some_and(|n| n > MAX_ARCHIVE) {
            return Err(format!("{url} is too large"));
        }
        response
            .bytes()
            .await
            .map_err(|e| format!("couldn't download {url}: {e}"))
    };
    let sums = fetch(format!("{base}/SHA256SUMS")).await?;
    let expected = checksum_for(&String::from_utf8_lossy(&sums), &file)
        .ok_or_else(|| format!("the release's checksums don't list {file}"))?;
    let archive = fetch(format!("{base}/{file}")).await?;
    if hex::encode(Sha256::digest(&archive)) != expected {
        return Err(format!("{file} doesn't match the release's checksum"));
    }
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find this binary: {e}"))?;
    let exe2 = exe.clone();
    tokio::task::spawn_blocking(move || install(&archive, &exe2))
        .await
        .map_err(|e| e.to_string())??;
    Ok(exe)
}

/// The checksum listed for `file` in a `sha256sum` file.
fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*').trim_start_matches("./");
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// Unpacks the `nethernode` binary from a release archive over `exe`.
fn install(archive: &[u8], exe: &Path) -> Result<(), String> {
    use std::io::Read;
    let staged = exe.with_extension("new");
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut found = false;
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        if path != Path::new("nethernode") || !entry.header().entry_type().is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_ARCHIVE)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        write_executable(&staged, &bytes)
            .map_err(|e| format!("couldn't save the new binary: {e}"))?;
        found = true;
        break;
    }
    if !found {
        return Err("the release archive has no nethernode binary".into());
    }
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("couldn't replace this binary: {e}")
    })
}

#[cfg(unix)]
fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

/// Replaces this process with `exe`, keeping its arguments and environment.
/// Returns only if that fails.
pub fn exec(exe: &Path) -> std::io::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .exec()
    }
    #[cfg(not(unix))]
    {
        let _ = exe;
        std::io::Error::other("restarting in place needs Unix")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_checksums() {
        assert!(valid_version("0.2.1"));
        assert!(!valid_version("0.2"));
        assert!(!valid_version("0.2.1-rc.1"));
        assert!(!valid_version("../../x.y.z"));
        let sums = format!(
            "{a}  ./nethernode-v0.2.1-linux-amd64.tar.gz\n{b}  ./nethernode-v0.2.1-linux-arm64.tar.gz\n",
            a = "a".repeat(64),
            b = "B".repeat(64)
        );
        assert_eq!(
            checksum_for(&sums, "nethernode-v0.2.1-linux-arm64.tar.gz"),
            Some("b".repeat(64))
        );
        assert_eq!(
            checksum_for(&sums, "nethernode-v0.2.1-linux-riscv.tar.gz"),
            None
        );
    }

    #[test]
    fn the_binary_comes_out_of_a_release_archive() {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "nethernode", &b"new!"[..])
            .unwrap();
        let archive = builder.into_inner().unwrap().finish().unwrap();
        let exe = dir.path().join("nethernode");
        std::fs::write(&exe, b"old").unwrap();
        install(&archive, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new!");
        assert!(!exe.with_extension("new").exists());
    }
}
