//! Backing up and restoring server folders.
//!
//! Archives live in `<data>/backups/<slug>-<time>-<id>.tar.zst`. A running
//! server's saving is paused (`save-off`, `save-all flush`) while its folder
//! is read, and turned back on (`save-on`) however the backup ends. The
//! server's lifecycle lock is held throughout, so nothing starts, stops, or
//! wakes it mid-backup.

pub mod http;
pub mod tasks;

use std::path::{Path, PathBuf};

use connectrpc::ConnectError;
use nn_events::Event;
use nn_files::FileError;
use nn_files::backup::{REGENERATED, restore_into};
use nn_store::{Backup, Server};
use serde_json::json;

use crate::AppState;
use crate::rpc::store_error;

/// Free space kept back when making a backup.
const DISK_MARGIN: u64 = 1024 * 1024 * 1024;

/// What kind of backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Made by someone.
    Manual,
    /// Made by a schedule (subject to its retention).
    Scheduled,
    /// Made automatically before a restore.
    PreRestore,
}

impl Kind {
    /// The stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Manual => "manual",
            Kind::Scheduled => "scheduled",
            Kind::PreRestore => "pre-restore",
        }
    }
}

/// `<data>/backups`.
pub(crate) fn backups_dir(state: &AppState) -> Option<PathBuf> {
    Some(
        state
            .nodes
            .local_node()
            .servers_dir()?
            .parent()?
            .join("backups"),
    )
}

/// The archive file of a backup record, refusing anything but a plain name.
pub(crate) fn archive_path(state: &AppState, backup: &Backup) -> Option<PathBuf> {
    let name = &backup.file_name;
    if name.contains(['/', '\\']) || name.starts_with('.') || !name.ends_with(".tar.zst") {
        return None;
    }
    Some(backups_dir(state)?.join(name))
}

fn job_events(
    state: &AppState,
    server_id: &str,
    job_id: &str,
    kind: &'static str,
) -> impl FnMut(u64, u64) + Send + use<> {
    let events = state.events.clone();
    let (server_id, job_id) = (server_id.to_owned(), job_id.to_owned());
    let mut last = None;
    move |done, total| {
        let percent = (done.min(total) * 100)
            .checked_div(total)
            .map_or(0, |p| u8::try_from(p).unwrap_or(100));
        if last != Some(percent) {
            last = Some(percent);
            events.publish(Event::FileJobProgress {
                server_id: server_id.clone(),
                job_id: job_id.clone(),
                kind: kind.to_owned(),
                percent,
            });
        }
    }
}

fn finished(
    state: &AppState,
    server_id: &str,
    job_id: &str,
    kind: &str,
    result: &Result<String, String>,
) {
    let (message, failed) = match result {
        Ok(m) => (m.clone(), false),
        Err(m) => (m.clone(), true),
    };
    state.events.publish(Event::FileJobFinished {
        server_id: server_id.to_owned(),
        job_id: job_id.to_owned(),
        kind: kind.to_owned(),
        message,
        failed,
    });
}

/// Starts backing a server up in the background. Returns the backup and
/// job IDs.
pub(crate) async fn start_backup(
    state: &AppState,
    server: &Server,
    kind: Kind,
    user_id: Option<&str>,
) -> Result<(String, String), ConnectError> {
    let (backup_id, file) = begin(state, server, kind, user_id).await?;
    let job_id = nn_store::new_id();
    let (state2, server2, job2, backup2) = (
        state.clone(),
        server.clone(),
        job_id.clone(),
        backup_id.clone(),
    );
    tokio::spawn(async move {
        // The outcome is reported through file_job_finished.
        let _ = run(&state2, &server2, &backup2, &file, &job2).await;
    });
    Ok((backup_id, job_id))
}

/// Backs a server up and waits for it to finish.
pub(crate) async fn backup_and_wait(
    state: &AppState,
    server: &Server,
    kind: Kind,
) -> Result<String, String> {
    let (backup_id, file) = begin(state, server, kind, None)
        .await
        .map_err(|e| e.message.unwrap_or_default())?;
    run(state, server, &backup_id, &file, &nn_store::new_id()).await
}

async fn run(
    state: &AppState,
    server: &Server,
    backup_id: &str,
    file: &Path,
    job_id: &str,
) -> Result<String, String> {
    let result = run_locked(state, server, async |current| {
        make(state, &current, backup_id, file, job_id).await
    })
    .await;
    if let Err(message) = &result {
        // Failed before the archive was started (the lock, say).
        let _ = state
            .auth
            .store()
            .finish_backup(backup_id, "failed", 0, 0, 0, Some(message))
            .await;
    }
    finished(state, &server.id, job_id, "backup", &result);
    result
}

/// Checks space and records a backup about to be made.
async fn begin(
    state: &AppState,
    server: &Server,
    kind: Kind,
    user_id: Option<&str>,
) -> Result<(String, PathBuf), ConnectError> {
    let dir = backups_dir(state).ok_or_else(|| {
        ConnectError::unavailable("the data folder isn't available, so backups can't be made")
    })?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ConnectError::internal(format!("couldn't create the backups folder: {e}")))?;
    if let Some(free) = nn_uploads::available_space(&dir)
        && free < DISK_MARGIN
    {
        return Err(ConnectError::resource_exhausted(format!(
            "less than {} free; free up space before backing up",
            human(DISK_MARGIN)
        )));
    }
    let stamp = jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S").to_string();
    // A ULID's leading characters are its timestamp; the tail is random.
    let ulid = nn_store::new_id();
    let name = format!(
        "{}-{stamp}-{}.tar.zst",
        server.slug,
        ulid[ulid.len() - 8..].to_ascii_lowercase()
    );
    let id = state
        .auth
        .store()
        .insert_backup(&server.id, &server.name, &name, kind.as_str(), user_id)
        .await
        .map_err(store_error)?;
    Ok((id, dir.join(name)))
}

/// Holds the lifecycle lock (when Docker is available) for `work`.
async fn run_locked<T>(
    state: &AppState,
    server: &Server,
    work: impl AsyncFnOnce(Server) -> Result<T, String>,
) -> Result<T, String> {
    match &state.nodes.local_node().orchestrator_for_backups() {
        Some(orch) => orch
            .locked(&server.id, work)
            .await
            .map_err(|e| e.to_string())?,
        None => work(server.clone()).await,
    }
}

/// Makes the archive (lock already held). Pauses saving on a running server.
async fn make(
    state: &AppState,
    server: &Server,
    backup_id: &str,
    file: &Path,
    job_id: &str,
) -> Result<String, String> {
    let running = server.status == "running";
    let orch = state.nodes.local_node().orchestrator_for_backups();
    if running && let Some(orch) = &orch {
        orch.command(&server.id, "save-off")
            .await
            .map_err(|e| format!("couldn't pause saving: {e}"))?;
        if let Err(e) = orch.command(&server.id, "save-all flush").await {
            let _ = orch.command(&server.id, "save-on").await;
            return Err(format!("couldn't save the world first: {e}"));
        }
    }
    let result = write_archive(state, server, file, job_id).await;
    if running && let Some(orch) = &orch {
        // Always turn saving back on, whatever happened.
        if let Err(e) = orch.command(&server.id, "save-on").await {
            tracing::warn!(server = %server.id, error = %e, "couldn't turn saving back on after a backup");
        }
    }
    let store = state.auth.store();
    match result {
        Ok((size, report)) => {
            let note = (report.skipped_links > 0)
                .then(|| format!("{} links left out", report.skipped_links));
            if let Err(e) = store
                .finish_backup(
                    backup_id,
                    "ready",
                    size as i64,
                    report.files as i64,
                    report.bytes as i64,
                    note.as_deref(),
                )
                .await
            {
                tracing::warn!(error = %e, "couldn't record a finished backup");
            }
            Ok(format!(
                "Backed up {} ({}, {} files).",
                server.name,
                human(size),
                report.files
            ))
        }
        Err(message) => {
            let _ = tokio::fs::remove_file(file).await;
            let _ = store
                .finish_backup(backup_id, "failed", 0, 0, 0, Some(&message))
                .await;
            Err(format!("The backup of {} failed: {message}", server.name))
        }
    }
}

async fn write_archive(
    state: &AppState,
    server: &Server,
    file: &Path,
    job_id: &str,
) -> Result<(u64, nn_files::backup::BackupReport), String> {
    let (_, files) = crate::files::open(state, &server.id)
        .await
        .map_err(|e| e.message.unwrap_or_default())?;
    let tmp = file.with_extension("zst.tmp");
    let (file, tmp2) = (file.to_owned(), tmp.clone());
    let mut progress = job_events(state, &server.id, job_id, "backup");
    let result = tokio::task::spawn_blocking(
        move || -> Result<(u64, nn_files::backup::BackupReport), FileError> {
            let out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp2)?;
            let (out, report) = files.backup_to(
                std::io::BufWriter::with_capacity(1 << 20, out),
                &mut progress,
            )?;
            let out = out
                .into_inner()
                .map_err(|e| FileError::Io(e.into_error()))?;
            out.sync_all()?;
            let size = out.metadata()?.len();
            drop(out);
            std::fs::rename(&tmp2, &file)?;
            Ok((size, report))
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result.map_err(|e| e.to_string())
}

/// Restores `backup` over its server in the background: stops it, backs up
/// what's there, swaps the folders, and optionally starts it again.
pub(crate) async fn start_restore(
    state: &AppState,
    backup: Backup,
    server: Server,
    user_id: &str,
    start_after: bool,
) -> Result<String, ConnectError> {
    let source = archive_path(state, &backup)
        .ok_or_else(|| ConnectError::not_found("the backup's file is missing"))?;
    if !tokio::fs::try_exists(&source).await.unwrap_or(false) {
        return Err(ConnectError::not_found("the backup's file is missing"));
    }
    // Without Docker no server can be running, so there's nothing to stop.
    let orch = state.nodes.local_node().orchestrator_for_backups();
    if start_after && orch.is_none() {
        return Err(ConnectError::unavailable(
            "Docker isn't available, so the server can't be started afterwards",
        ));
    }
    let job_id = nn_store::new_id();
    let (state2, job2, user) = (state.clone(), job_id.clone(), user_id.to_owned());
    tokio::spawn(async move {
        let result = async {
            if let Some(orch) = &orch
                && server.status != "stopped"
                && server.status != "crashed"
            {
                orch.stop(&server.id, false)
                    .await
                    .map_err(|e| format!("couldn't stop the server: {e}"))?;
            }
            let restored = run_locked(&state2, &server, async |current| {
                if !matches!(
                    current.status.as_str(),
                    "stopped" | "crashed" | "sleeping" | "installing"
                ) {
                    return Err(
                        "the server started again before the restore could begin; try again"
                            .to_owned(),
                    );
                }
                // A copy of what's being replaced, just in case.
                let (pre_id, pre_file) = begin(&state2, &current, Kind::PreRestore, Some(&user))
                    .await
                    .map_err(|e| e.message.unwrap_or_default())?;
                make(&state2, &current, &pre_id, &pre_file, &job2).await?;
                swap_in(&state2, &current, &source, &job2).await
            })
            .await?;
            if start_after
                && let Some(orch) = &orch
                && let Err(e) = orch.start(&server.id).await
            {
                return Ok(format!("{restored} It didn't start again: {e}"));
            }
            Ok(restored)
        }
        .await;
        finished(&state2, &server.id, &job2, "restore", &result);
    });
    Ok(job_id)
}

/// Extracts `source` beside the server's folder and swaps it in, carrying
/// over the folders backups leave out so they aren't downloaded again.
async fn swap_in(
    state: &AppState,
    server: &Server,
    source: &Path,
    job_id: &str,
) -> Result<String, String> {
    let servers_dir = state
        .nodes
        .local_node()
        .servers_dir()
        .map(Path::to_path_buf)
        .ok_or("the data folder isn't available")?;
    let (_, files) = crate::files::open(state, &server.id)
        .await
        .map_err(|e| e.message.unwrap_or_default())?;
    let owner = files.owner();
    drop(files);
    let stamp = jiff::Timestamp::now().as_second();
    let live = servers_dir.join(&server.slug);
    let incoming_name = format!("{}.restoring-{stamp}", server.slug);
    let replaced = servers_dir.join(format!("{}.replaced-{stamp}", server.slug));
    let (source, incoming_dir) = (source.to_owned(), servers_dir.clone());
    let mut progress = job_events(state, &server.id, job_id, "restore");
    let name2 = incoming_name.clone();
    let report = tokio::task::spawn_blocking(move || {
        let size = std::fs::metadata(&source)?.len();
        let file = std::fs::File::open(&source)?;
        restore_into(
            std::io::BufReader::with_capacity(1 << 20, file),
            size,
            &incoming_dir,
            &name2,
            owner,
            nn_files::Limits::default(),
            &mut progress,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("couldn't read the backup: {e}"))?;
    let incoming = servers_dir.join(&incoming_name);

    let (live2, incoming2, replaced2) = (live.clone(), incoming.clone(), replaced.clone());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        for name in REGENERATED {
            let (from, to) = (live2.join(name), incoming2.join(name));
            if from.is_dir() && !to.exists() {
                std::fs::rename(&from, &to)?;
            }
        }
        std::fs::rename(&live2, &replaced2)?;
        if let Err(e) = std::fs::rename(&incoming2, &live2) {
            // Put the original back.
            let _ = std::fs::rename(&replaced2, &live2);
            return Err(e);
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        let _ = std::fs::remove_dir_all(&incoming);
        format!("couldn't swap the folders: {e}")
    })?;
    // The old folder is in the pre-restore backup; remove it.
    if let Ok(Err(e)) =
        tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&replaced)).await
    {
        tracing::warn!(error = %e, "couldn't remove the replaced server folder");
    }
    Ok(format!(
        "Restored {} ({} files). What was there before is kept as a pre-restore backup.",
        server.name, report.files
    ))
}

/// Deletes a backup's file and record.
pub(crate) async fn delete(state: &AppState, backup: &Backup) -> Result<(), ConnectError> {
    if backup.status == "creating" {
        return Err(ConnectError::failed_precondition(
            "this backup is still being made",
        ));
    }
    if let Some(path) = archive_path(state, backup) {
        match tokio::fs::remove_file(&path).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(ConnectError::internal(format!(
                    "couldn't delete the backup: {e}"
                )));
            }
            _ => {}
        }
    }
    state
        .auth
        .store()
        .delete_backup(&backup.id)
        .await
        .map_err(store_error)?;
    Ok(())
}

/// Keeps the newest scheduled backup of each of the last `daily` days and
/// `weekly` ISO weeks (in `tz`); deletes the rest of the scheduled ones.
pub(crate) async fn prune(
    state: &AppState,
    server_id: &str,
    daily: u32,
    weekly: u32,
    tz: &jiff::tz::TimeZone,
) -> usize {
    let Ok(backups) = state.auth.store().list_backups(Some(server_id)).await else {
        return 0;
    };
    let scheduled: Vec<&Backup> = backups
        .iter()
        .filter(|b| b.kind == "scheduled" && b.status == "ready")
        .collect();
    let keep = retained(
        &scheduled
            .iter()
            .map(|b| (b.id.as_str(), b.created_at))
            .collect::<Vec<_>>(),
        daily,
        weekly,
        tz,
    );
    let mut deleted = 0;
    for b in scheduled
        .into_iter()
        .filter(|b| !keep.contains(b.id.as_str()))
    {
        if delete(state, b).await.is_ok() {
            deleted += 1;
        }
    }
    if deleted > 0 {
        let entry = nn_store::NewAuditEntry {
            user_id: None,
            action: "backup.prune",
            target_type: Some("server"),
            target_id: Some(server_id),
            details: Some(
                json!({ "deleted": deleted, "keep_daily": daily, "keep_weekly": weekly }),
            ),
        };
        let _ = state.auth.store().insert_audit(entry).await;
    }
    deleted
}

/// Which backups (id, created ms; newest first) retention keeps.
pub(crate) fn retained<'a>(
    backups: &[(&'a str, i64)],
    daily: u32,
    weekly: u32,
    tz: &jiff::tz::TimeZone,
) -> std::collections::HashSet<&'a str> {
    let mut keep = std::collections::HashSet::new();
    let mut days = Vec::new();
    let mut weeks = Vec::new();
    for (id, at) in backups {
        let Ok(ts) = jiff::Timestamp::from_millisecond(*at) else {
            continue;
        };
        let date = ts.to_zoned(tz.clone()).date();
        let week = date.iso_week_date();
        let week = (week.year(), week.week());
        if !days.contains(&date) && days.len() < daily as usize {
            days.push(date);
            keep.insert(*id);
        }
        if !weeks.contains(&week) && weeks.len() < weekly as usize {
            weeks.push(week);
            keep.insert(*id);
        }
    }
    keep
}

pub(crate) fn human(bytes: u64) -> String {
    crate::uploads::http::human_size(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_keeps_one_per_day_and_week() {
        let tz = jiff::tz::TimeZone::UTC;
        let day = 86_400_000i64;
        // Two a day for 20 days, newest first (day 0 is a Monday).
        let start = jiff::civil::date(2026, 10, 5)
            .at(12, 0, 0, 0)
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp()
            .as_millisecond();
        let ids: Vec<String> = (0..40).map(|i| format!("b{i}")).collect();
        let backups: Vec<(&str, i64)> = (0..40)
            .map(|i| (ids[i].as_str(), start + 20 * day - (i as i64) * day / 2))
            .collect();
        let keep = retained(&backups, 7, 4, &tz);
        // 7 newest days (one each) plus older weeks' newest backups.
        let daily: Vec<_> = (0..14).step_by(2).map(|i| ids[i].as_str()).collect();
        for d in &daily {
            assert!(keep.contains(d), "{d}");
        }
        assert!(!keep.contains("b1"), "second backup of a day goes");
        assert!(keep.len() <= 7 + 4);
        assert!(keep.len() > 7, "older weeks are kept too");
        assert!(retained(&backups, 0, 0, &tz).is_empty());
    }
}
