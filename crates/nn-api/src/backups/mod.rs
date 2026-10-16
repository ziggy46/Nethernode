//! Backing up and restoring server folders.
//!
//! The panel keeps the records and names the archives
//! (`<slug>-<time>-<id>.tar.zst`); the server's node makes, restores, and
//! stores them (see `nn_node::Node::backup`).

pub mod http;
pub mod tasks;

use connectrpc::ConnectError;
use nn_events::Event;
use nn_node::Archive;
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

pub(crate) fn finished(
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
    crate::moves::not_moving(state, &server.id)?;
    let (backup_id, name) = begin(state, server, kind, user_id).await?;
    let job_id = nn_store::new_id();
    let (state2, server2, job2, backup2) = (
        state.clone(),
        server.clone(),
        job_id.clone(),
        backup_id.clone(),
    );
    tokio::spawn(async move {
        // The outcome is reported through file_job_finished.
        let _ = run(&state2, &server2, &backup2, &name, &job2).await;
    });
    Ok((backup_id, job_id))
}

/// Backs a server up and waits for it to finish.
pub(crate) async fn backup_and_wait(
    state: &AppState,
    server: &Server,
    kind: Kind,
) -> Result<String, String> {
    let (backup_id, name) = begin(state, server, kind, None)
        .await
        .map_err(|e| e.message.unwrap_or_default())?;
    run(state, server, &backup_id, &name, &nn_store::new_id()).await
}

async fn run(
    state: &AppState,
    server: &Server,
    backup_id: &str,
    name: &str,
    job_id: &str,
) -> Result<String, String> {
    let made = state
        .nodes
        .for_server(server)
        .backup(server, name, job_id)
        .await;
    let result = record(state, server, backup_id, made).await;
    finished(state, &server.id, job_id, "backup", &result);
    result
}

/// Records how making an archive went.
pub(crate) async fn record(
    state: &AppState,
    server: &Server,
    backup_id: &str,
    made: Result<Archive, String>,
) -> Result<String, String> {
    let store = state.auth.store();
    match made {
        Ok(Archive { size, report }) => {
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
            let _ = store
                .finish_backup(backup_id, "failed", 0, 0, 0, Some(&message))
                .await;
            Err(format!("The backup of {} failed: {message}", server.name))
        }
    }
}

/// Checks space and records a backup about to be made. Returns its ID and
/// archive name.
pub(crate) async fn begin(
    state: &AppState,
    server: &Server,
    kind: Kind,
    user_id: Option<&str>,
) -> Result<(String, String), ConnectError> {
    let free = state
        .nodes
        .for_server(server)
        .backups_free_space()
        .await
        .map_err(ConnectError::unavailable)?;
    if let Some(free) = free
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
    Ok((id, name))
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
    crate::moves::not_moving(state, &server.id)?;
    if backup.node_id != server.node_id {
        let machine = match state.auth.store().find_node(&backup.node_id).await {
            Ok(Some(node)) => node.name,
            _ => "a machine that was removed".to_owned(),
        };
        return Err(ConnectError::failed_precondition(format!(
            "This backup is on {machine}, but the server now runs on another machine. Download it instead."
        )));
    }
    let node = state.nodes.for_server(&server);
    if !node.has_backup(&backup.file_name).await {
        return Err(ConnectError::not_found("the backup's file is missing"));
    }
    // Without Docker no server can be running, so there's nothing to stop.
    let docker = node.unavailable().is_none();
    if start_after && !docker {
        return Err(ConnectError::unavailable(
            "Docker isn't available, so the server can't be started afterwards",
        ));
    }
    let job_id = nn_store::new_id();
    let (state2, job2, user) = (state.clone(), job_id.clone(), user_id.to_owned());
    tokio::spawn(async move {
        let result = async {
            if docker && server.status != "stopped" && server.status != "crashed" {
                node.stop(&server.id, false)
                    .await
                    .map_err(|e| format!("couldn't stop the server: {e}"))?;
            }
            // A copy of what's being replaced, just in case.
            let (pre_id, pre_name) = begin(&state2, &server, Kind::PreRestore, Some(&user))
                .await
                .map_err(|e| e.message.unwrap_or_default())?;
            let restored = match node
                .restore(&server, &backup.file_name, &pre_name, &job2)
                .await
            {
                Ok(restored) => restored,
                Err(message) => {
                    // Nothing was made or changed.
                    let _ = state2.auth.store().delete_backup(&pre_id).await;
                    return Err(message);
                }
            };
            record(&state2, &server, &pre_id, restored.pre_restore).await?;
            let files = restored.files?;
            let message = format!(
                "Restored {} ({files} files). What was there before is kept as a pre-restore backup.",
                server.name
            );
            if start_after && let Err(e) = node.start(&server.id).await {
                return Ok(format!("{message} It didn't start again: {e}"));
            }
            Ok(message)
        }
        .await;
        finished(&state2, &server.id, &job2, "restore", &result);
    });
    Ok(job_id)
}

/// The node holding a backup's archive.
pub(crate) async fn node_of(state: &AppState, backup: &Backup) -> nn_node::Node {
    state.nodes.get(&backup.node_id)
}

/// Deletes a backup's file and record.
pub(crate) async fn delete(state: &AppState, backup: &Backup) -> Result<(), ConnectError> {
    if backup.status == "creating" {
        return Err(ConnectError::failed_precondition(
            "this backup is still being made",
        ));
    }
    node_of(state, backup)
        .await
        .delete_backup(&backup.file_name)
        .await
        .map_err(ConnectError::internal)?;
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
