//! The Smart Proxy: one public port, routing Minecraft Java players to the
//! right server by the hostname they connected with.
//!
//! [`Proxy`] owns the listeners and an atomically swapped [`RouteTable`].
//! Configuration changes are applied with [`Proxy::apply`]: routes swap
//! without touching live connections, and listeners are added, removed, or
//! rebound individually.
//!
//! This crate deliberately knows nothing about HTTP, Docker, or the
//! database; it talks to the rest of NetherNode through [`ProxyHooks`] and
//! the event bus.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::{ArcSwap, ArcSwapAny};
use nn_events::{Event, EventBus};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::{JoinHandle, JoinSet};

pub mod config;
mod conn;
pub mod hooks;
pub mod io;
pub mod limiter;
pub mod proxy_protocol;
pub mod registry;
pub mod routes;
mod splice;
pub mod status_cache;

pub use config::{BackendSpec, Limits, ListenerSpec, ProxyConfig, RouteSpec, ServerState};
pub use conn::{SLEEPING_SUFFIX, UNKNOWN_HOST_MESSAGE};
pub use hooks::{BoxFuture, NoopHooks, PlayerJoin, ProxyHooks, WakeOutcome};
pub use registry::ConnectionInfo;
pub use routes::{Resolution, RouteTable};

/// Metric names.
pub mod metrics_names {
    /// Counter of connections by `route` and `result`.
    pub const CONNECTIONS: &str = "proxy_connections_total";
    /// Gauge of spliced connections by `route`.
    pub const ACTIVE_CONNECTIONS: &str = "proxy_active_connections";
    /// Histogram of time to receive a complete handshake.
    pub const HANDSHAKE_SECONDS: &str = "proxy_handshake_seconds";
    /// Counter of bytes by `direction` (`up` = player → server).
    pub const BYTES: &str = "proxy_bytes_total";
}

/// State shared by listeners and connection tasks.
pub(crate) struct Shared {
    routes: ArcSwap<RouteTable>,
    hooks: Arc<dyn ProxyHooks>,
    events: EventBus,
    limits: Limits,
    slots: Arc<Semaphore>,
    ip_limiter: limiter::IpRateLimiter,
    status: status_cache::StatusCache,
    connections: registry::ConnectionRegistry,
    buffers: splice::BufferPool,
}

/// Whether a listener is accepting connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenerState {
    /// Bound and accepting on this address.
    Listening(SocketAddr),
    /// Couldn't bind; the error message.
    Failed(String),
}

/// A listener's configuration and state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerStatus {
    /// The listener's spec.
    pub spec: ListenerSpec,
    /// Current state.
    pub state: ListenerState,
}

struct ListenerEntry {
    spec: Arc<ArcSwapAny<Arc<ListenerSpec>>>,
    state: ListenerState,
    task: Option<JoinHandle<()>>,
}

impl Drop for ListenerEntry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The Smart Proxy. Cheap to clone.
#[derive(Clone)]
pub struct Proxy {
    shared: Arc<Shared>,
    listeners: Arc<Mutex<HashMap<String, ListenerEntry>>>,
}

impl Proxy {
    /// Creates a proxy with no listeners or routes.
    pub fn new(hooks: Arc<dyn ProxyHooks>, events: EventBus, limits: Limits) -> Self {
        let shared = Shared {
            routes: ArcSwap::from_pointee(RouteTable::default()),
            hooks,
            events,
            slots: Arc::new(Semaphore::new(limits.max_connections)),
            ip_limiter: limiter::IpRateLimiter::new(limits.per_ip_burst, limits.per_ip_per_second),
            status: status_cache::StatusCache::default(),
            connections: registry::ConnectionRegistry::default(),
            buffers: splice::BufferPool::new(limits.buffer_size),
            limits,
        };
        Self {
            shared: Arc::new(shared),
            listeners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Applies a configuration: swaps the route table and reconciles
    /// listeners. Live connections are never interrupted, even on listeners
    /// that are removed. Returns every listener's status.
    pub async fn apply(&self, config: ProxyConfig) -> Vec<ListenerStatus> {
        let table = RouteTable::build(&config.routes, &config.backends, &config.listeners);
        self.shared.status.retain(|id| table.backend(id).is_some());
        self.shared.routes.store(Arc::new(table));

        let mut listeners = self.listeners.lock().await;
        let wanted: HashMap<&str, &ListenerSpec> = config
            .listeners
            .iter()
            .map(|l| (l.id.as_str(), l))
            .collect();
        let removed: Vec<String> = listeners
            .keys()
            .filter(|id| !wanted.contains_key(id.as_str()))
            .cloned()
            .collect();
        for id in removed {
            listeners.remove(&id);
            tracing::info!(listener = %id, "proxy listener removed");
            self.shared
                .events
                .publish(Event::ProxyListenerChanged { listener_id: id });
        }

        for spec in &config.listeners {
            if let Some(entry) = listeners.get(&spec.id)
                && entry.spec.load().bind == spec.bind
                && matches!(entry.state, ListenerState::Listening(_))
            {
                // Same socket; only flags changed. Applies to new connections.
                entry.spec.store(Arc::new(spec.clone()));
                continue;
            }
            listeners.remove(&spec.id);
            let entry = self.start_listener(spec.clone()).await;
            self.shared.events.publish(Event::ProxyListenerChanged {
                listener_id: spec.id.clone(),
            });
            listeners.insert(spec.id.clone(), entry);
        }
        statuses(&listeners)
    }

    async fn start_listener(&self, spec: ListenerSpec) -> ListenerEntry {
        let shared_spec = Arc::new(ArcSwapAny::new(Arc::new(spec.clone())));
        match bind(spec.bind) {
            Ok(listener) => {
                let addr = listener.local_addr().unwrap_or(spec.bind);
                tracing::info!(listener = %spec.id, %addr, proxy_protocol = spec.proxy_protocol, "proxy listening");
                let task = tokio::spawn(accept_loop(
                    listener,
                    shared_spec.clone(),
                    self.shared.clone(),
                ));
                ListenerEntry {
                    spec: shared_spec,
                    state: ListenerState::Listening(addr),
                    task: Some(task),
                }
            }
            Err(error) => {
                tracing::error!(listener = %spec.id, bind = %spec.bind, %error, "proxy listener failed to bind");
                ListenerEntry {
                    spec: shared_spec,
                    state: ListenerState::Failed(error.to_string()),
                    task: None,
                }
            }
        }
    }

    /// Every listener's status.
    pub async fn listener_statuses(&self) -> Vec<ListenerStatus> {
        statuses(&*self.listeners.lock().await)
    }

    /// Connections currently spliced to a backend.
    pub fn active_connections(&self) -> Vec<ConnectionInfo> {
        self.shared.connections.snapshot()
    }

    /// The current route table.
    pub fn routes(&self) -> Arc<RouteTable> {
        self.shared.routes.load_full()
    }

    /// The cached status for a server, if any.
    pub fn cached_status(&self, server_id: &str) -> Option<status_cache::CachedStatus> {
        self.shared.status.get(server_id)
    }

    /// Spawns the background task that refreshes the status cache from
    /// running servers every `limits.status_refresh`.
    pub fn spawn_status_refresher(&self) -> JoinHandle<()> {
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(shared.limits.status_refresh);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                refresh_status(&shared).await;
            }
        })
    }

    /// Refreshes the status cache once, now.
    pub async fn refresh_status_now(&self) {
        refresh_status(&self.shared).await;
    }

    /// Stops all listeners. Live connections finish on their own.
    pub async fn shutdown(&self) {
        self.listeners.lock().await.clear();
    }
}

fn statuses(listeners: &HashMap<String, ListenerEntry>) -> Vec<ListenerStatus> {
    let mut list: Vec<_> = listeners
        .values()
        .map(|e| ListenerStatus {
            spec: (**e.spec.load()).clone(),
            state: e.state.clone(),
        })
        .collect();
    list.sort_by(|a, b| {
        a.spec
            .bind
            .port()
            .cmp(&b.spec.bind.port())
            .then(a.spec.id.cmp(&b.spec.id))
    });
    list
}

fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.set_nodelay(true)?;
    socket.bind(addr)?;
    socket.listen(4096)
}

async fn accept_loop(
    listener: TcpListener,
    spec: Arc<ArcSwapAny<Arc<ListenerSpec>>>,
    shared: Arc<Shared>,
) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                // Usually fd exhaustion; back off instead of spinning.
                tracing::warn!(%error, "proxy accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Ok(permit) = shared.slots.clone().try_acquire_owned() else {
            metrics::counter!(metrics_names::CONNECTIONS, "route" => "-", "result" => "over_capacity").increment(1);
            continue;
        };
        if !shared.ip_limiter.allow(peer.ip()) {
            metrics::counter!(metrics_names::CONNECTIONS, "route" => "-", "result" => "rate_limited").increment(1);
            continue;
        }
        let spec = spec.load_full();
        let shared = shared.clone();
        tokio::spawn(async move {
            let _permit = permit;
            conn::handle(stream, peer, spec, shared).await;
        });
    }
}

async fn refresh_status(shared: &Shared) {
    let table = shared.routes.load_full();
    let mut pings = JoinSet::new();
    for backend in table
        .backends()
        .filter(|b| b.spec.state == ServerState::Running)
    {
        let backend = backend.clone();
        pings.spawn(async move {
            let result = status_cache::ping_backend(
                backend.spec.addr,
                backend.expects_proxy_protocol,
                Duration::from_secs(3),
            )
            .await;
            (backend.spec.server_id.clone(), result)
        });
    }
    while let Some(Ok((server_id, result))) = pings.join_next().await {
        match result {
            Ok(status) => shared.status.put(&server_id, status),
            Err(error) => tracing::debug!(server = %server_id, %error, "status ping failed"),
        }
    }
}
