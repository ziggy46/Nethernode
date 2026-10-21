//! Cloudflare, managed by NetherNode: with an API token, NetherNode makes a
//! Cloudflare Tunnel, runs its connector (`cloudflared`, see
//! `nn_orchestrator::tunnel`), and keeps the tunnel's routes and the DNS
//! records for modules' addresses (`map.mc1.example.com`) in step with
//! what's open. Players' Minecraft connections don't use any of it.
//!
//! The token needs Account › Cloudflare Tunnel › Edit, Zone › Zone › Read
//! and Zone › DNS › Edit. DNS changes are off until someone has seen what
//! they'd be (Preview) and turned them on. See [`plan`] for the rules DNS
//! changes follow.

pub(crate) mod api;
pub(crate) mod plan;

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use connectrpc::ConnectError;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::module_sites;
use api::{Api, Zone};
use plan::Change;

const SETTING: &str = "cloudflare";
const SYNC_EVERY: Duration = Duration::from_secs(10 * 60);

/// What's saved (secrets sealed with the data key).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    token: String,
    account_id: String,
    account_name: String,
    tunnel_id: String,
    tunnel_token: String,
    /// DNS records and routes are kept in step.
    #[serde(default)]
    dns: bool,
}

/// How the last sync went, for Settings.
#[derive(Debug, Clone, Default)]
pub(crate) struct Status {
    pub last_error: Option<String>,
    pub last_sync_ms: Option<i64>,
    /// Addresses leading to the tunnel.
    pub routed: Vec<String>,
    /// Why some addresses can't.
    pub conflicts: Vec<String>,
    /// Addresses outside the token's zones (set up by hand).
    pub outside: Vec<String>,
    /// Changes made (or, from a preview, that would be).
    pub changes: Vec<String>,
    pub zones: Vec<String>,
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(Default::default);
static SYNC: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

pub(crate) fn status() -> Status {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_status(update: impl FnOnce(&mut Status)) {
    update(&mut STATUS.lock().unwrap_or_else(|e| e.into_inner()));
}

fn base(state: &AppState) -> &str {
    match state.config.cloudflare_api.as_str() {
        "" => api::DEFAULT_BASE,
        other => other,
    }
}

/// Where the tunnel sends visitors: the panel's own web port.
fn service(state: &AppState) -> String {
    let port = match state.config.panel_port {
        0 => 8080,
        port => port,
    };
    format!("http://127.0.0.1:{port}")
}

async fn saved(state: &AppState) -> Option<Saved> {
    let sealed: Saved = state
        .auth
        .store()
        .get_setting(SETTING)
        .await
        .ok()
        .flatten()?;
    let secrets = state.content.secrets()?;
    Some(Saved {
        token: secrets.open(&sealed.token).ok()?,
        tunnel_token: secrets.open(&sealed.tunnel_token).ok()?,
        ..sealed
    })
}

async fn save(state: &AppState, saved: &Saved) -> Result<(), ConnectError> {
    let secrets = state.content.secrets().ok_or_else(|| {
        ConnectError::unavailable("the data key isn't available, so the token can't be saved")
    })?;
    let sealed = Saved {
        token: secrets.seal(&saved.token),
        tunnel_token: secrets.seal(&saved.tunnel_token),
        ..saved.clone()
    };
    state
        .auth
        .store()
        .set_setting(SETTING, &sealed)
        .await
        .map_err(crate::rpc::store_error)
}

/// What Settings shows about the connection.
pub(crate) struct Connection {
    pub account_name: String,
    pub tunnel_id: String,
    pub dns: bool,
    pub token_hint: String,
}

/// The saved connection, if there is one.
pub(crate) async fn connection(state: &AppState) -> Option<Connection> {
    let saved = saved(state).await?;
    let tail: String = saved
        .token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(Connection {
        account_name: saved.account_name,
        tunnel_id: saved.tunnel_id,
        dns: saved.dns,
        token_hint: format!("…{tail}"),
    })
}

/// Connects with `token`: checks what it can reach, makes the tunnel (or
/// keeps the one it already made), and starts the connector. DNS changes
/// stay off until turned on.
pub(crate) async fn connect(state: &AppState, token: &str) -> Result<(), ConnectError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(ConnectError::invalid_argument("Paste an API token."));
    }
    let api = Api::new(base(state), token);
    let zones = api.zones().await.map_err(|e| {
        ConnectError::failed_precondition(format!(
            "Cloudflare didn't accept the token: {e}. It needs Zone › Zone › Read and Zone › DNS › Edit for your domain, and Account › Cloudflare Tunnel › Edit."
        ))
    })?;
    if zones.is_empty() {
        return Err(ConnectError::failed_precondition(
            "The token can't see any zones. Give it Zone › Zone › Read and Zone › DNS › Edit for your domain.",
        ));
    }
    // The zones say which account they're in. (Listing accounts needs a
    // permission the token otherwise has no use for.)
    let account = match zones.iter().find_map(|z| z.account.clone()) {
        Some(account) => account,
        None => api
            .accounts()
            .await
            .ok()
            .and_then(|a| a.into_iter().next())
            .ok_or_else(|| {
                ConnectError::failed_precondition(
                    "Couldn't tell which Cloudflare account the domain is in.",
                )
            })?,
    };
    let _sync = SYNC.lock().await;
    let previous = saved(state).await;
    let tunnel_id = match previous
        .as_ref()
        .filter(|p| p.account_id == account.id && !p.tunnel_id.is_empty())
    {
        Some(p) => p.tunnel_id.clone(),
        None => {
            let suffix = &nn_store::new_id()[20..];
            api.create_tunnel(
                &account.id,
                &format!("nethernode-{}", suffix.to_ascii_lowercase()),
            )
            .await
            .map_err(|e| {
                ConnectError::failed_precondition(format!("Couldn't make a tunnel: {e}"))
            })?
        }
    };
    let tunnel_token = api
        .tunnel_token(&account.id, &tunnel_id)
        .await
        .map_err(|e| {
            ConnectError::failed_precondition(format!("Couldn't get the tunnel's token: {e}"))
        })?;
    let next = Saved {
        token: token.to_owned(),
        account_id: account.id,
        account_name: account.name,
        tunnel_id,
        tunnel_token,
        dns: previous.is_some_and(|p| p.dns),
    };
    save(state, &next).await?;
    set_status(|s| {
        s.zones = zones.iter().map(|z| z.name.clone()).collect();
        s.last_error = None;
    });
    start_connector(state, &next).await;
    Ok(())
}

async fn start_connector(state: &AppState, saved: &Saved) {
    if let Err(e) = state.nodes.local().run_tunnel(&saved.tunnel_token).await {
        tracing::warn!(error = %e, "couldn't start the Cloudflare Tunnel connector");
        set_status(|s| s.last_error = Some(format!("The connector didn't start: {e}")));
    }
}

/// Turns keeping DNS and the tunnel's routes in step on or off. Off removes
/// what NetherNode added.
pub(crate) async fn set_dns(state: &AppState, on: bool) -> Result<(), ConnectError> {
    let Some(mut saved) = saved(state).await else {
        return Err(ConnectError::failed_precondition(
            "Connect Cloudflare first.",
        ));
    };
    saved.dns = on;
    save(state, &saved).await?;
    sync(state, Mode::Apply)
        .await
        .map_err(ConnectError::unavailable)?;
    Ok(())
}

/// Disconnects: removes the records NetherNode added, the tunnel and its
/// connector, and forgets the token.
pub(crate) async fn disconnect(state: &AppState) -> Result<(), ConnectError> {
    if let Some(mut saved) = saved(state).await {
        saved.dns = false;
        save(state, &saved).await?;
        // Removes NetherNode's records (nothing is wanted any more).
        if let Err(e) = sync(state, Mode::Apply).await {
            return Err(ConnectError::unavailable(format!(
                "Couldn't remove NetherNode's DNS records, so nothing was disconnected: {e}"
            )));
        }
        let _sync = SYNC.lock().await;
        if let Err(e) = state.nodes.local().stop_tunnel().await {
            tracing::warn!(error = %e, "couldn't remove the Cloudflare Tunnel connector");
        }
        let api = Api::new(base(state), &saved.token);
        if let Err(e) = api.delete_tunnel(&saved.account_id, &saved.tunnel_id).await {
            tracing::warn!(error = %e, "couldn't delete the Cloudflare Tunnel");
        }
    }
    state
        .auth
        .store()
        .delete_setting(SETTING)
        .await
        .map_err(crate::rpc::store_error)?;
    set_status(|s| *s = Status::default());
    Ok(())
}

/// The addresses that should lead to the tunnel: every open module's own
/// addresses.
async fn wanted(state: &AppState) -> Vec<String> {
    let store = state.auth.store();
    let (Ok(modules), Ok(servers), Ok(routes)) = (
        store.list_modules(None).await,
        store.list_servers().await,
        store.list_routes().await,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for module in modules
        .iter()
        .filter(|m| m.web_public && m.web_port.is_some())
    {
        let (Some(host), Some(server)) = (
            module.web_host.as_deref(),
            servers.iter().find(|s| s.id == module.server_id),
        ) else {
            continue;
        };
        out.extend(module_sites::addresses(
            host,
            &module_sites::server_addresses(server, &routes),
        ));
    }
    out.sort();
    out.dedup();
    out
}

/// The zone a hostname belongs to: the longest one it ends with.
fn zone_of<'z>(zones: &'z [Zone], host: &str) -> Option<&'z Zone> {
    zones
        .iter()
        .filter(|z| host == z.name || host.ends_with(&format!(".{}", z.name)))
        .max_by_key(|z| z.name.len())
}

/// Whether [`sync`] makes the changes or only works them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// What turning DNS management on would do now; nothing is changed.
    Preview,
    /// Make the changes: with management on, every open module's
    /// addresses; with it off, removing what NetherNode added.
    Apply,
}

/// Works out (and with [`Mode::Apply`] makes) the DNS changes and the
/// tunnel's routes. Returns the changes, described.
pub(crate) async fn sync(state: &AppState, mode: Mode) -> Result<Vec<String>, String> {
    let _sync = SYNC.lock().await;
    let Some(saved) = saved(state).await else {
        return Ok(Vec::new());
    };
    let api = Api::new(base(state), &saved.token);
    let result = async {
        let zones = api.zones().await?;
        let all = wanted(state).await;
        let wanted = match mode {
            Mode::Preview => all.clone(),
            Mode::Apply if saved.dns => all.clone(),
            Mode::Apply => Vec::new(),
        };
        let target = format!("{}.cfargotunnel.com", saved.tunnel_id);
        let (mut described, mut routed, mut conflicts) = (Vec::new(), Vec::new(), Vec::new());
        for zone in &zones {
            let mine: Vec<String> = wanted
                .iter()
                .filter(|h| zone_of(&zones, h).is_some_and(|z| z.id == zone.id))
                .cloned()
                .collect();
            let records = api.records(&zone.id).await?;
            let plan = plan::plan(&zone.name, &records, &target, &mine);
            if mode == Mode::Apply {
                for change in &plan.changes {
                    match change {
                        Change::Create(r) => api.create_record(&zone.id, r).await,
                        Change::Update { id, record } => {
                            api.update_record(&zone.id, id, record).await
                        }
                        Change::Delete(r) => api.delete_record(&zone.id, &r.id).await,
                    }
                    .map_err(|e| format!("{}: {e}", change.describe()))?;
                }
            }
            described.extend(plan.changes.iter().map(Change::describe));
            routed.extend(plan.routed);
            conflicts.extend(plan.conflicts);
        }
        let outside: Vec<String> = all
            .iter()
            .filter(|h| zone_of(&zones, h).is_none())
            .cloned()
            .collect();
        if mode == Mode::Apply {
            api.set_routes(
                &saved.account_id,
                &saved.tunnel_id,
                &routed,
                &service(state),
            )
            .await?;
        }
        Ok::<_, String>((zones, described, routed, conflicts, outside))
    }
    .await;
    match result {
        Ok((zones, described, routed, conflicts, outside)) => {
            set_status(|s| {
                s.zones = zones.into_iter().map(|z| z.name).collect();
                s.changes = described.clone();
                s.conflicts = conflicts;
                s.outside = outside;
                s.last_error = None;
                if mode == Mode::Apply {
                    s.routed = routed;
                    s.last_sync_ms = Some(nn_store::now_ms());
                }
            });
            Ok(described)
        }
        Err(e) => {
            set_status(|s| s.last_error = Some(e.clone()));
            Err(e)
        }
    }
}

async fn managing(state: &AppState) -> bool {
    saved(state).await.is_some_and(|s| s.dns)
}

/// Syncs soon, in the background (modules' addresses changed).
pub(crate) fn nudge(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        if managing(&state).await
            && let Err(e) = sync(&state, Mode::Apply).await
        {
            tracing::warn!(error = %e, "couldn't sync Cloudflare");
        }
    });
}

/// Starts the connector if Cloudflare is connected, and syncs every ten
/// minutes (copies of wildcards follow the wildcard's address).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        if let Some(saved) = saved(&state).await {
            start_connector(&state, &saved).await;
        }
        loop {
            if managing(&state).await
                && let Err(e) = sync(&state, Mode::Apply).await
            {
                tracing::warn!(error = %e, "couldn't sync Cloudflare");
            }
            tokio::time::sleep(SYNC_EVERY).await;
        }
    });
}
