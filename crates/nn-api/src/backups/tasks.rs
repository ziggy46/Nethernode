//! Cleaning up after a backup or restore that NetherNode was stopped in
//! the middle of.

use crate::AppState;

/// Marks interrupted backups failed and removes their partial files, and
/// finishes or undoes interrupted restores.
pub async fn recover(state: &AppState) {
    let store = state.auth.store();
    if let Ok(names) = store.fail_unfinished_backups().await
        && let Some(dir) = super::backups_dir(state)
    {
        for name in names.iter().filter(|n| !n.contains(['/', '\\'])) {
            let _ = tokio::fs::remove_file(dir.join(name)).await;
            let _ = tokio::fs::remove_file(dir.join(format!("{name}.tmp"))).await;
            let _ =
                tokio::fs::remove_file(dir.join(name.replace(".tar.zst", ".tar.zst.tmp"))).await;
        }
    }
    if let Some(dir) = super::backups_dir(state)
        && let Ok(mut entries) = tokio::fs::read_dir(&dir).await
    {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_name().to_string_lossy().ends_with(".tmp") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
    let Some(servers_dir) = state.servers.servers_dir.clone() else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || {
        let Ok(entries) = std::fs::read_dir(&servers_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if let Some((slug, _)) = name.split_once(".restoring-") {
                // Never swapped in: the live folder is untouched.
                tracing::info!(server = %slug, "removing an interrupted restore");
                let _ = std::fs::remove_dir_all(&path);
            } else if let Some((slug, _)) = name.split_once(".replaced-") {
                let live = servers_dir.join(slug);
                if live.exists() {
                    // The restore finished; this is the old copy.
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    // Stopped between the two renames: put the original back.
                    tracing::warn!(server = %slug, "undoing an interrupted restore");
                    let _ = std::fs::rename(&path, &live);
                }
            }
        }
    })
    .await;
}
