//! The panel's download cache (`<data>/cache`): Minecraft clients and
//! copies of mod files for drawing items, project icons, and player faces.
//! Everything in it is downloaded again when needed, so a daily sweep
//! removes what hasn't been used for a while, and an admin can clear it.
//!
//! Version lists (`cache/catalog`) are left alone: they're small, and they
//! keep the panel working while Mojang, Modrinth or CurseForge are down.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::AppState;

/// The parts of the cache, by folder.
pub(crate) const KINDS: [&str; 4] = ["minecraft", "mods", "icons", "heads"];

/// How often the sweep runs.
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);
/// Minecraft clients and mod copies unused this long are removed.
const UNUSED: Duration = Duration::from_secs(14 * 24 * 3600);
/// Icons and faces older than this are removed (they're refreshed long
/// before then when in use).
const STALE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Half-written downloads older than this are removed.
const ABANDONED: Duration = Duration::from_secs(24 * 3600);

/// Space one part of the cache takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub kind: &'static str,
    pub bytes: u64,
    pub files: u64,
}

fn dir(state: &AppState, kind: &str) -> PathBuf {
    state.content.cache_dir().join(kind)
}

/// Space each part of the cache takes.
pub(crate) async fn usage(state: &AppState) -> Vec<Usage> {
    let dirs: Vec<_> = KINDS.iter().map(|k| (*k, dir(state, k))).collect();
    tokio::task::spawn_blocking(move || {
        dirs.into_iter()
            .map(|(kind, dir)| {
                let (bytes, files) = measure(&dir);
                Usage { kind, bytes, files }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Removes what hasn't been used for a while. Returns the bytes freed.
pub(crate) async fn sweep(state: &AppState) -> u64 {
    let content = &state.content;
    // Clients of versions a server runs stay, used lately or not.
    let mut keep = HashSet::new();
    for server in state.auth.store().list_servers().await.unwrap_or_default() {
        if let Ok(version) = content.catalog.release_id(&server.mc_version).await {
            keep.insert(version);
        }
    }
    let mut doomed = content.item_textures.prunable(&keep, UNUSED);
    doomed.extend(content.mod_files.prunable(&dir(state, "mods"), UNUSED));
    let stale = [dir(state, "icons"), dir(state, "heads")];
    let all: Vec<_> = KINDS.iter().map(|k| dir(state, k)).collect();
    let freed = tokio::task::spawn_blocking(move || {
        let mut freed = doomed.iter().map(|p| remove(p)).sum::<u64>();
        for dir in &stale {
            freed += remove_older(dir, STALE, |_| true);
        }
        for dir in &all {
            freed += remove_older(dir, ABANDONED, is_partial);
        }
        freed
    })
    .await
    .unwrap_or(0);
    if freed > 0 {
        tracing::info!(
            mb = freed / (1024 * 1024),
            "removed downloads that hadn't been used for a while"
        );
    }
    freed
}

/// Empties the cache. Returns the bytes freed.
pub(crate) async fn clear(state: &AppState) -> u64 {
    state.content.item_textures.forget_all();
    state.content.mod_files.forget_all();
    let dirs: Vec<_> = KINDS.iter().map(|k| dir(state, k)).collect();
    tokio::task::spawn_blocking(move || dirs.iter().map(|d| remove(d)).sum())
        .await
        .unwrap_or(0)
}

/// Sweeps the cache once a day, starting a few minutes after start-up.
pub fn spawn_sweeper(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5 * 60)).await;
        loop {
            sweep(&state).await;
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    });
}

/// Bytes and files under `path` (symlinks aren't followed).
fn measure(path: &Path) -> (u64, u64) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return (0, 0);
    };
    if !meta.is_dir() {
        return (meta.len(), 1);
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    entries.flatten().fold((0, 0), |(bytes, files), entry| {
        let (b, f) = measure(&entry.path());
        (bytes + b, files + f)
    })
}

/// Deletes a file or folder. Returns the bytes freed.
fn remove(path: &Path) -> u64 {
    let (bytes, _) = measure(path);
    let removed = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => return 0,
    };
    match removed {
        Ok(()) => bytes,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "couldn't remove a cached download");
            0
        }
    }
}

/// Deletes files under `dir` that match `which` and are older than `age`.
fn remove_older(dir: &Path, age: Duration, which: fn(&Path) -> bool) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut freed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => freed += remove_older(&path, age, which),
            Ok(t) if t.is_file() => {
                if which(&path) && !nn_catalog::items::modified_within(&path, age) {
                    freed += remove(&path);
                }
            }
            _ => {}
        }
    }
    freed
}

/// A download that never finished.
fn is_partial(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "part" | "tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, days: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::now() - Duration::from_secs(days * 24 * 3600))
            .unwrap();
    }

    #[test]
    fn only_old_files_of_the_chosen_kind_go() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("1.21.4");
        std::fs::create_dir_all(&nested).unwrap();
        for name in ["old.png", "new.png", "old.part"] {
            std::fs::write(dir.path().join(name), b"12345").unwrap();
        }
        std::fs::write(nested.join("client.jar.part"), b"123").unwrap();
        age(&dir.path().join("old.png"), 40);
        age(&dir.path().join("old.part"), 2);
        age(&nested.join("client.jar.part"), 2);

        assert_eq!(remove_older(dir.path(), ABANDONED, is_partial), 8);
        assert!(
            dir.path().join("old.png").exists(),
            "not a partial download"
        );
        assert_eq!(remove_older(dir.path(), STALE, |_| true), 5);
        assert!(!dir.path().join("old.png").exists());
        assert!(dir.path().join("new.png").exists());
        assert_eq!(measure(dir.path()), (5, 1));
    }
}
