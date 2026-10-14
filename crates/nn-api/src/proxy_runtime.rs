//! Connects the Smart Proxy to the database: builds its configuration from
//! stored listeners, routes, and servers, and records player sessions.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use nn_proxy::{
    BackendSpec, BoxFuture, ListenerSpec, ListenerStatus, PlayerJoin, Proxy, ProxyConfig,
    ProxyHooks, RouteSpec, ServerState, WakeOutcome,
};
use nn_store::{NewPlayerSession, Store};
use tokio::sync::Mutex;

/// The proxy plus what it needs to reload itself from the database.
pub struct ProxyRuntime {
    proxy: Proxy,
    store: Store,
    enabled: bool,
    // Serializes reloads so concurrent edits apply in order.
    reload_lock: Mutex<()>,
    backend_ip: std::net::IpAddr,
}

impl ProxyRuntime {
    /// Wraps a proxy. When `enabled` is false, routes are still loaded (for
    /// display) but no listener is bound.
    pub fn new(proxy: Proxy, store: Store, enabled: bool) -> Self {
        Self {
            proxy,
            store,
            enabled,
            reload_lock: Mutex::new(()),
            backend_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        }
    }

    /// Dials servers at `ip` instead of loopback (see the `docker.backend_ip`
    /// setting).
    pub fn with_backend_ip(mut self, ip: std::net::IpAddr) -> Self {
        self.backend_ip = ip;
        self
    }

    /// The proxy.
    pub fn proxy(&self) -> &Proxy {
        &self.proxy
    }

    /// Whether the proxy is enabled in the NetherNode config.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Rebuilds the proxy configuration from the database and applies it.
    pub async fn reload(&self) -> nn_store::Result<Vec<ListenerStatus>> {
        let _guard = self.reload_lock.lock().await;
        let config = self.load_config().await?;
        Ok(self.proxy.apply(config).await)
    }

    async fn load_config(&self) -> nn_store::Result<ProxyConfig> {
        let listeners = if self.enabled {
            self.store
                .list_listeners()
                .await?
                .into_iter()
                .filter(|l| l.enabled)
                .filter_map(|l| {
                    let ip: IpAddr = l.bind_addr.parse().ok()?;
                    let port = u16::try_from(l.port).ok()?;
                    Some(ListenerSpec {
                        id: l.id,
                        bind: SocketAddr::new(ip, port),
                        proxy_protocol: l.proxy_protocol,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let routes = self
            .store
            .list_routes()
            .await?
            .into_iter()
            .map(|r| RouteSpec {
                id: r.id,
                pattern: r.hostname,
                server_id: r.server_id,
                listener_id: r.listener_id,
                priority: r.priority,
            })
            .collect();
        let backends = self
            .store
            .list_servers()
            .await?
            .into_iter()
            .filter_map(|s| {
                let port = u16::try_from(s.internal_port).ok()?;
                Some(BackendSpec {
                    addr: SocketAddr::new(self.backend_ip, port),
                    state: ServerState::from_status(&s.status),
                    server_id: s.id,
                    name: s.name,
                    slug: s.slug,
                })
            })
            .collect();
        Ok(ProxyConfig {
            listeners,
            routes,
            backends,
        })
    }
}

/// Proxy hooks backed by the database and, when Docker is available, the
/// orchestrator (to wake sleeping servers).
pub struct StoreProxyHooks {
    store: Store,
    orchestrator: std::sync::OnceLock<Arc<nn_orchestrator::Orchestrator>>,
    bans: Arc<crate::players::ProxyBans>,
}

impl StoreProxyHooks {
    /// Creates hooks writing to `store`. Waking is unavailable until
    /// [`StoreProxyHooks::set_orchestrator`] is called.
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Self {
            store,
            orchestrator: std::sync::OnceLock::new(),
            bans: Arc::default(),
        })
    }

    /// Enables wake-on-connect.
    pub fn set_orchestrator(&self, orchestrator: Arc<nn_orchestrator::Orchestrator>) {
        let _ = self.orchestrator.set(orchestrator);
    }

    /// The bans this proxy enforces, shared with player management.
    pub fn bans(&self) -> Arc<crate::players::ProxyBans> {
        self.bans.clone()
    }
}

impl ProxyHooks for StoreProxyHooks {
    fn wake<'a>(&'a self, server_id: &'a str) -> BoxFuture<'a, WakeOutcome> {
        Box::pin(async move {
            let Some(orchestrator) = self.orchestrator.get().cloned() else {
                tracing::info!(server = %server_id, "wake-on-connect requested, but Docker isn't available");
                return WakeOutcome::Unavailable;
            };
            tracing::info!(server = %server_id, "waking server for a joining player");
            // Starting can take a while (and may pull an image); the player is
            // told to retry while it boots.
            let id = server_id.to_owned();
            tokio::spawn(async move {
                if let Err(error) = orchestrator.wake(&id).await {
                    tracing::warn!(server = %id, %error, "wake-on-connect failed");
                }
            });
            WakeOutcome::Starting
        })
    }

    fn player_joined<'a>(&'a self, join: &'a PlayerJoin) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let ip = join.ip.to_string();
            let session = NewPlayerSession {
                server_id: &join.server_id,
                uuid: join.uuid.as_deref(),
                username: &join.username,
                ip: Some(&ip),
            };
            match self.store.open_player_session(session).await {
                Ok(id) => Some(id),
                Err(error) => {
                    tracing::warn!(%error, "failed to record player join");
                    None
                }
            }
        })
    }

    fn player_left(&self, session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Err(error) = self.store.close_player_session(&session_id).await {
                tracing::warn!(%error, "failed to record player leave");
            }
        })
    }

    fn admit<'a>(&'a self, login: &'a nn_proxy::Admission) -> BoxFuture<'a, Option<String>> {
        let refusal = self.bans.refusal(login);
        Box::pin(async move { refusal })
    }
}
