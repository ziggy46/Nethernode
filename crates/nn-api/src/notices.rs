//! What the bell in the top bar shows: things that need someone's
//! attention. Worked out fresh on each request from the servers, nodes,
//! backups and disks, plus a newer NetherNode release and newer modpack
//! versions when checking for those is on (Settings › Notifications).

use std::sync::Mutex;
use std::time::Duration;

use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use nn_auth::Permission;
use serde::Serialize;

use crate::AppState;
use crate::agents::{compatible, older};
use crate::files_http::authorize;
use crate::middleware::CurrentUser;

/// Whether to look for new releases (on unless turned off).
pub(crate) const UPDATE_CHECK_SETTING: &str = "updates.check";
const RELEASES_URL: &str = "https://api.github.com/repos/ziggy46/Nethernode/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(12 * 60 * 60);
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Below this much free disk (or a tenth of it), a machine is low.
const LOW_DISK: u64 = 5 * 1024 * 1024 * 1024;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The newest release seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Like `0.2.3`.
    pub version: String,
    /// Its release notes.
    pub url: String,
}

static LATEST: Mutex<Option<Release>> = Mutex::new(None);

/// The newest release found by the last check, if any.
pub fn latest_release() -> Option<Release> {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether checking for new releases is on.
pub(crate) async fn update_check_on(state: &AppState) -> bool {
    state
        .auth
        .store()
        .get_setting::<bool>(UPDATE_CHECK_SETTING)
        .await
        .ok()
        .flatten()
        .unwrap_or(true)
}

/// Asks GitHub for the newest release, about twice a day while that's on.
/// Only the request itself is sent: no install ID or anything else.
pub fn spawn_update_check(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            check_now(&state).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// Checks for a new release now, if checking is on.
pub(crate) async fn check_now(state: &AppState) {
    if !update_check_on(state).await {
        *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    }
    if let Err(e) = check_regardless().await {
        tracing::debug!(error = %e, "couldn't check for a new NetherNode release");
    }
}

/// Asks GitHub for the newest release now (someone asked), and keeps it.
pub(crate) async fn check_regardless() -> Result<Release, String> {
    let release = fetch_latest().await?;
    *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(release.clone());
    Ok(release)
}

/// Whether `release` is newer than this panel.
pub(crate) fn newer(release: &Release) -> bool {
    older(VERSION, &release.version)
}

async fn fetch_latest() -> Result<Release, String> {
    #[derive(serde::Deserialize)]
    struct GitHubRelease {
        tag_name: String,
        html_url: String,
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("NetherNode/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let release: GitHubRelease = client
        .get(RELEASES_URL)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.without_url().to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let version = release.tag_name.trim_start_matches('v').to_owned();
    if !nn_node::update::valid_version(&version) {
        return Err(format!("unexpected release tag {}", release.tag_name));
    }
    Ok(Release {
        version,
        url: release.html_url,
    })
}

/// One thing that needs attention.
#[derive(Debug, Serialize)]
pub(crate) struct Notice {
    /// Stable while the situation lasts, so it can be marked as seen.
    id: String,
    /// `danger`, `warning`, or `info`.
    level: &'static str,
    title: String,
    detail: String,
    /// Where to go about it: a panel path, or a full URL.
    link: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Notices {
    notices: Vec<Notice>,
}

/// `GET /api/notices`.
pub(crate) async fn list(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
) -> Response {
    let user = match authorize(user) {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };
    let mut notices = gather(&state, user.role.can(Permission::ManageSettings)).await;
    notices.sort_by_key(|n| match n.level {
        "danger" => 0,
        "warning" => 1,
        _ => 2,
    });
    Json(Notices { notices }).into_response()
}

fn low(total: u64, free: u64) -> bool {
    total > 0 && free < LOW_DISK.min(total / 10).max(total / 20)
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
}

async fn gather(state: &AppState, admin: bool) -> Vec<Notice> {
    let store = state.auth.store();
    let mut out = Vec::new();

    if admin
        && update_check_on(state).await
        && let Some(release) = latest_release()
        && older(VERSION, &release.version)
    {
        out.push(Notice {
            id: format!("update:{}", release.version),
            level: "info",
            title: format!("NetherNode {} is available", release.version),
            detail: format!(
                "This panel runs {VERSION}. Update it from Settings › Notifications; nodes on other machines follow on their own."
            ),
            link: Some("/settings?section=notifications".into()),
        });
    }

    let local = state.nodes.local();
    if let Some(reason) = local.unavailable() {
        out.push(Notice {
            id: "docker:local".into(),
            level: "danger",
            title: "Docker isn't reachable on this machine".into(),
            detail: format!("Servers here can't be started or managed: {reason}"),
            link: Some("/host".into()),
        });
    }
    if let Some((total, free)) = local.disk_space()
        && low(total, free)
    {
        out.push(Notice {
            id: "disk:local".into(),
            level: "warning",
            title: "This machine is running out of disk space".into(),
            detail: format!(
                "{} free of {}. Backups, uploads and worlds fail once it's full.",
                gb(free),
                gb(total)
            ),
            link: Some("/host".into()),
        });
    }

    for node in store.list_nodes().await.unwrap_or_default() {
        if node.is_local() {
            continue;
        }
        let live = state.agents.live(&node.id);
        match &live {
            None if node.public_key.is_some() => out.push(Notice {
                id: format!("offline:{}", node.id),
                level: "warning",
                title: format!("{} is offline", node.name),
                detail: "Its servers can't be managed, and players can't reach them, until its agent reconnects.".into(),
                link: Some("/nodes".into()),
            }),
            Some(live) if !compatible(VERSION, &live.version) => out.push(Notice {
                id: format!("outdated:{}:{}", node.id, live.version),
                level: "warning",
                title: format!("{} needs an update", node.name),
                detail: format!(
                    "It runs NetherNode {}; this panel runs {VERSION}. Its servers can't be managed until it's updated.",
                    live.version
                ),
                link: Some("/nodes".into()),
            }),
            _ => {}
        }
        if let Some(error) = state.agents.update_state(&node.id).error {
            out.push(Notice {
                id: format!("update-failed:{}", node.id),
                level: "danger",
                title: format!("{} didn't update", node.name),
                detail: error,
                link: Some("/nodes".into()),
            });
        }
        if let Some((total, free)) = live
            .and_then(|l| l.host)
            .and_then(|h| Some((h.disk_total_bytes?, h.disk_free_bytes?)))
            && low(total, free)
        {
            out.push(Notice {
                id: format!("disk:{}", node.id),
                level: "warning",
                title: format!("{} is running out of disk space", node.name),
                detail: format!(
                    "{} free of {}. Backups, uploads and worlds fail once it's full.",
                    gb(free),
                    gb(total)
                ),
                link: Some("/nodes".into()),
            });
        }
    }

    let checking = update_check_on(state).await;
    for server in store.list_servers().await.unwrap_or_default() {
        if let Some(failure) = crate::modpack_updates::failure(&server.id) {
            out.push(Notice {
                id: format!("modpack-failed:{}:{}", server.id, failure.at_ms),
                level: "danger",
                title: format!("{}'s modpack update failed", server.name),
                detail: failure.message,
                link: Some(format!("/servers/{}", server.id)),
            });
        }
        if checking && let Some(version) = crate::modpack_updates::available(&server) {
            out.push(Notice {
                id: format!("modpack:{}:{version}", server.id),
                level: "info",
                title: format!("A newer pack version for {}", server.name),
                detail: format!(
                    "{version} is available. Update it from the server's Mods tab; a backup is made first."
                ),
                link: Some(format!("/servers/{}", server.id)),
            });
        }
        if server.status == "crashed" {
            out.push(Notice {
                id: format!("crashed:{}:{}", server.id, server.updated_at),
                level: "danger",
                title: format!("{} crashed", server.name),
                detail:
                    "It stopped unexpectedly and wasn't restarted. Its console shows what happened."
                        .into(),
                link: Some(format!("/servers/{}", server.id)),
            });
        }
    }

    let since = nn_store::now_ms() - DAY_MS;
    for backup in store.list_backups(None).await.unwrap_or_default() {
        if backup.status == "failed" && backup.created_at >= since {
            out.push(Notice {
                id: format!("backup:{}", backup.id),
                level: "danger",
                title: format!("A backup of {} failed", backup.server_name),
                detail: backup.message.unwrap_or_else(|| "It didn't finish.".into()),
                link: Some("/backups".into()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::low;

    #[test]
    fn low_disk_is_under_five_gb_or_a_tenth() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert!(low(62 * GB, 2 * GB));
        assert!(!low(62 * GB, 46 * GB));
        // Small disks: a tenth.
        assert!(low(20 * GB, GB));
        assert!(!low(20 * GB, 3 * GB));
    }
}
