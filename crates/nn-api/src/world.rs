//! World tools: each dimension's size and chunks, and trimming chunks
//! nobody spent time in (after a backup, with the server stopped and kept
//! from starting until it's done).

use connectrpc::ConnectError;
use nn_files::{Dimension, RelPath, TrimOptions, TrimReport};
use nn_node::Folder;
use nn_store::Server;

use crate::AppState;
use crate::backups;

/// The world folder: `level-name` from server.properties, or `world`.
pub(crate) async fn level_name(folder: &Folder) -> String {
    let level = match RelPath::parse("server.properties") {
        Ok(path) => folder
            .read_if_exists(&path, nn_files::EDITOR_LIMIT)
            .await
            .ok()
            .flatten()
            .and_then(|c| {
                nn_files::Properties::parse(&String::from_utf8_lossy(&c.data))
                    .get("level-name")
                    .map(str::to_owned)
            }),
        Err(_) => None,
    };
    level
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty() && RelPath::parse(l).is_ok() && !l.contains('/'))
        .unwrap_or_else(|| "world".to_owned())
}

/// The world spawn's chunk, from level.dat (0, 0 when it can't be read).
async fn spawn_chunk(folder: &Folder, level: &str) -> (i32, i32) {
    let read = async {
        let path = RelPath::parse(&format!("{level}/level.dat")).ok()?;
        let reader = folder.open_read(&path).await.ok()?;
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut reader.from_offset(0).await.ok()?, &mut bytes)
            .await
            .ok()?;
        let (_, root) = nn_players::nbt::read_gzip(&bytes).ok()?;
        let data = root.get("Data")?;
        let x = data.get("SpawnX")?.as_i64()?;
        let z = data.get("SpawnZ")?.as_i64()?;
        Some((i32::try_from(x >> 4).ok()?, i32::try_from(z >> 4).ok()?))
    };
    read.await.unwrap_or((0, 0))
}

/// The world's dimensions.
pub(crate) async fn dimensions(folder: &Folder) -> Result<(String, Vec<Dimension>), ConnectError> {
    let level = level_name(folder).await;
    let dims = folder
        .world_dimensions(level.clone())
        .await
        .map_err(crate::files::file_error)?;
    Ok((level, dims))
}

/// Ticks in a minute.
const TICKS_PER_MINUTE: i64 = 20 * 60;

/// Counts (or, with `apply`, trims) the chunks players spent less than
/// `minutes` in, in the background. Returns the job ID; the outcome comes
/// as a `file_job_finished` event (kind `world-trim`).
pub(crate) fn start_trim(
    state: &AppState,
    server: Server,
    minutes: u32,
    keep_radius: u32,
    apply: bool,
    user_id: &str,
) -> Result<String, ConnectError> {
    // Counting is fine while it runs; trimming isn't.
    if apply && !matches!(server.status.as_str(), "stopped" | "crashed") {
        return Err(ConnectError::failed_precondition(
            "Stop the server first: the world can't be trimmed while it might be in use.",
        ));
    }
    crate::moves::not_moving(state, &server.id)?;
    let options = TrimOptions {
        min_ticks: i64::from(minutes.clamp(1, 600)) * TICKS_PER_MINUTE,
        keep_radius: i32::try_from(keep_radius.min(1000)).unwrap_or(0),
    };
    let job = nn_store::new_id();
    let (state, job_id, user) = (state.clone(), job.clone(), user_id.to_owned());
    tokio::spawn(async move {
        let result = run(&state, &server, options, apply, &user).await;
        backups::finished(&state, &server.id, &job_id, "world-trim", &result);
    });
    Ok(job)
}

async fn run(
    state: &AppState,
    server: &Server,
    options: TrimOptions,
    apply: bool,
    user_id: &str,
) -> Result<String, String> {
    if apply {
        backups::labelled_backup_and_wait(state, server, user_id, "Before trimming the world")
            .await
            .map_err(|e| format!("The backup failed, so nothing was trimmed: {e}"))?;
    }
    // Nothing else may start or change the server meanwhile.
    let _busy = if apply {
        let busy = state
            .nodes
            .begin_move(&server.id)
            .ok_or("The server is busy with something else; try again in a moment.")?;
        let now = state
            .auth
            .store()
            .find_server(&server.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("The server was deleted.")?;
        if !matches!(now.status.as_str(), "stopped" | "crashed") {
            return Err("The server started meanwhile, so nothing was trimmed.".into());
        }
        Some(busy)
    } else {
        None
    };
    let folder = state
        .nodes
        .for_server(server)
        .folder(server)
        .await
        .map_err(|e| e.to_string())?;
    let level = level_name(&folder).await;
    let spawn = spawn_chunk(&folder, &level).await;
    let report = folder
        .trim_world(level, spawn, options, apply)
        .await
        .map_err(|e| e.to_string())?;
    Ok(describe(&report, apply))
}

fn describe(r: &TrimReport, apply: bool) -> String {
    let freed = r.bytes_before.saturating_sub(r.bytes_after);
    let mut out = if r.removed == 0 {
        format!("No chunks to trim among {}.", thousands(r.chunks))
    } else if apply {
        format!(
            "Trimmed {} of {} chunks, freeing {}. They generate again when someone gets there.",
            thousands(r.removed),
            thousands(r.chunks),
            size(freed)
        )
    } else {
        format!(
            "Trimming would remove {} of {} chunks and free about {}.",
            thousands(r.removed),
            thousands(r.chunks),
            size(freed)
        )
    };
    if r.unreadable > 0 {
        out.push_str(&format!(
            " {} couldn't be read and stay.",
            thousands(r.unreadable)
        ));
    }
    out
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", (b / 1e6).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_read_naturally() {
        let r = TrimReport {
            chunks: 50_000,
            removed: 12_345,
            unreadable: 2,
            bytes_before: 3_000_000_000,
            bytes_after: 1_800_000_000,
        };
        assert_eq!(
            describe(&r, false),
            "Trimming would remove 12,345 of 50,000 chunks and free about 1.2 GB. 2 couldn't be read and stay."
        );
        assert!(describe(&r, true).starts_with("Trimmed 12,345 of 50,000 chunks, freeing 1.2 GB."));
        assert_eq!(
            describe(
                &TrimReport {
                    chunks: 7,
                    ..Default::default()
                },
                false
            ),
            "No chunks to trim among 7."
        );
    }
}
