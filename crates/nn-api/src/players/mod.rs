//! Player management: how to reach each server, name resolution, ban
//! bookkeeping, and the background work (ban expiry, change notifications).

pub(crate) mod backend;
pub(crate) mod heads;
pub(crate) mod resolve;
pub mod tasks;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use connectrpc::ConnectError;
use nn_players::mgmt::{MgmtClient, MgmtError};
use nn_players::mojang::{Mojang, MojangError};
use nn_players::profile::Profile;
use nn_store::{Server, Store};

use crate::rpc::store_error;

/// How long a Mojang lookup stays cached.
const PROFILE_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
/// How long "no such account" stays cached.
const MISSING_TTL_MS: i64 = 24 * 3600 * 1000;

/// Bans recorded by NetherNode, enforced by the proxy at login. This is the
/// only place IP bans can work (servers behind the proxy only see its
/// address), and it backs up name bans that an offline-mode server dropped
/// because it had never seen the player. Kept in memory for the connection
/// path; reloaded on change.
#[derive(Default)]
pub struct ProxyBans {
    rules: RwLock<Vec<BanRule>>,
}

#[derive(Debug, Clone)]
struct BanRule {
    server_id: Option<String>,
    uuid: Option<String>,
    name: String,
    ip: Option<IpAddr>,
    reason: Option<String>,
    expires_at: Option<i64>,
}

impl BanRule {
    fn matches(&self, login: &nn_proxy::Admission) -> bool {
        self.ip == Some(login.ip)
            || login
                .username
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(&self.name))
            || login
                .uuid
                .as_deref()
                .zip(self.uuid.as_deref())
                .is_some_and(|(a, b)| normalize(a) == normalize(b))
    }
}

fn normalize(uuid: &str) -> String {
    uuid.replace('-', "").to_ascii_lowercase()
}

impl ProxyBans {
    /// The disconnect message for this login, if it's banned.
    pub fn refusal(&self, login: &nn_proxy::Admission) -> Option<String> {
        let now = nn_store::now_ms();
        let rules = self.rules.read().unwrap_or_else(|e| e.into_inner());
        let rule = rules.iter().find(|r| {
            r.server_id.as_deref().is_none_or(|s| s == login.server_id)
                && r.expires_at.is_none_or(|e| e > now)
                && r.matches(login)
        })?;
        let mut message = "You are banned from this server.".to_owned();
        if let Some(reason) = rule.reason.as_deref().filter(|r| !r.is_empty()) {
            message.push_str(&format!("\nReason: {reason}"));
        }
        if let Some(expires) = rule.expires_at {
            message.push_str(&format!(
                "\nThe ban ends {}.",
                nn_players::lists::mc_date(expires)
            ));
        }
        Some(message)
    }

    /// Reloads the rules from the store.
    pub async fn reload(&self, store: &Store) {
        match store.active_player_bans(None).await {
            Ok(bans) => {
                let rules = bans
                    .into_iter()
                    .map(|b| BanRule {
                        ip: b.ip.as_deref().and_then(|ip| ip.parse().ok()),
                        server_id: b.server_id,
                        uuid: b.uuid,
                        name: b.name,
                        reason: b.reason,
                        expires_at: b.expires_at,
                    })
                    .collect();
                *self.rules.write().unwrap_or_else(|e| e.into_inner()) = rules;
            }
            Err(e) => tracing::warn!(error = %e, "couldn't load bans"),
        }
    }
}

enum MgmtSlot {
    Connected {
        client: Arc<MgmtClient>,
        run: i64,
    },
    /// No management protocol in this run of the server (too old, or the
    /// container predates its port); try again after the next start.
    Unsupported {
        run: i64,
    },
}

/// Player management state shared by the RPC service and background tasks.
pub struct PlayerManager {
    pub(crate) mojang: Mojang,
    pub(crate) bans: Arc<ProxyBans>,
    /// Limits concurrent skin fetches, so a page of new players doesn't
    /// trip Mojang's rate limit.
    pub(crate) head_fetches: tokio::sync::Semaphore,
    mgmt: tokio::sync::Mutex<HashMap<String, MgmtSlot>>,
}

impl Default for PlayerManager {
    fn default() -> Self {
        Self::new(Arc::default())
    }
}

impl PlayerManager {
    /// A manager sharing `bans` with the proxy hooks.
    pub fn new(bans: Arc<ProxyBans>) -> Self {
        Self {
            mojang: Mojang::new(),
            bans,
            head_fetches: tokio::sync::Semaphore::new(2),
            mgmt: tokio::sync::Mutex::default(),
        }
    }

    /// The server's management protocol connection, if it has one this run.
    pub(crate) async fn mgmt(
        &self,
        node: &nn_node::Node,
        server: &Server,
    ) -> Option<Arc<MgmtClient>> {
        let run = server.last_started_at.unwrap_or(0);
        let mut slots = self.mgmt.lock().await;
        match slots.get(&server.id) {
            Some(MgmtSlot::Connected { client, run: r }) if *r == run && !client.is_closed() => {
                return Some(client.clone());
            }
            Some(MgmtSlot::Unsupported { run: r }) if *r == run => return None,
            _ => {}
        }
        match node.management(&server.id, Duration::from_secs(3)).await? {
            Ok(client) => {
                let client = Arc::new(client);
                slots.insert(
                    server.id.clone(),
                    MgmtSlot::Connected {
                        client: client.clone(),
                        run,
                    },
                );
                tracing::debug!(server = %server.id, "connected to the management protocol");
                Some(client)
            }
            Err(e) => {
                if matches!(e, MgmtError::Unauthorized) {
                    tracing::warn!(server = %server.id, "the management protocol rejected NetherNode's secret");
                }
                slots.insert(server.id.clone(), MgmtSlot::Unsupported { run });
                None
            }
        }
    }

    /// Drops a server's connection (it stopped, or the connection failed).
    pub(crate) async fn forget_mgmt(&self, server_id: &str) {
        self.mgmt.lock().await.remove(server_id);
    }

    /// The Mojang account with this name (cached), or `None` if none exists.
    pub(crate) async fn lookup(
        &self,
        store: &Store,
        name: &str,
    ) -> Result<Option<Profile>, ConnectError> {
        let now = nn_store::now_ms();
        if let Some(hit) = store.cached_profile(name).await.map_err(store_error)? {
            let fresh = match &hit.uuid {
                Some(_) => now - hit.fetched_at < PROFILE_TTL_MS,
                None => now - hit.fetched_at < MISSING_TTL_MS,
            };
            if fresh {
                return Ok(hit.uuid.map(|uuid| Profile {
                    uuid,
                    name: hit.name.unwrap_or_else(|| name.to_owned()),
                }));
            }
        }
        match self.mojang.lookup(name).await {
            Ok(found) => {
                store
                    .cache_profile(
                        name,
                        found.as_ref().map(|p| p.uuid.as_str()),
                        found.as_ref().map(|p| p.name.as_str()),
                    )
                    .await
                    .map_err(store_error)?;
                Ok(found)
            }
            Err(MojangError::RateLimited) => Err(ConnectError::resource_exhausted(
                "Mojang is rate limiting username lookups. Try again in a minute.",
            )),
            Err(e) => Err(ConnectError::unavailable(format!(
                "Couldn't look up {name} with Mojang: {e}"
            ))),
        }
    }
}
