//! Configuration snapshots the proxy is driven by.

use std::net::SocketAddr;
use std::time::Duration;

/// Everything the proxy needs, rebuilt from the database on any change and
/// applied atomically with [`crate::Proxy::apply`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProxyConfig {
    /// Enabled listeners.
    pub listeners: Vec<ListenerSpec>,
    /// Hostname routes.
    pub routes: Vec<RouteSpec>,
    /// Servers routes can point at.
    pub backends: Vec<BackendSpec>,
}

/// A listener to accept players on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerSpec {
    /// Stable ID (from the database).
    pub id: String,
    /// Address to bind.
    pub bind: SocketAddr,
    /// Send a PROXY protocol v2 header to backends.
    pub proxy_protocol: bool,
}

/// A hostname route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    /// Stable ID.
    pub id: String,
    /// `play.example.com`, `*.example.com`, or `*`.
    pub pattern: String,
    /// Destination; `None` on a wildcard means "the server whose slug is the subdomain".
    pub server_id: Option<String>,
    /// Restrict to one listener.
    pub listener_id: Option<String>,
    /// Higher wins among routes of the same kind.
    pub priority: i64,
}

/// A server the proxy can route to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSpec {
    /// Server ID.
    pub server_id: String,
    /// Display name, used in player-facing messages.
    pub name: String,
    /// Slug, used by slug wildcards.
    pub slug: String,
    /// Where the server listens (normally `127.0.0.1:<internal_port>`).
    pub addr: SocketAddr,
    /// Lifecycle state.
    pub state: ServerState,
}

/// Server lifecycle states the proxy distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerState {
    /// Accepting players.
    Running,
    /// Booting.
    Starting,
    /// Shutting down.
    Stopping,
    /// Stopped automatically while empty; wakes on login.
    Sleeping,
    /// Stopped by an admin.
    Stopped,
    /// Exited unexpectedly.
    Crashed,
    /// Being installed.
    Installing,
}

impl ServerState {
    /// Parses a database status string. Unknown values map to `Stopped`.
    pub fn from_status(status: &str) -> Self {
        match status {
            "running" => ServerState::Running,
            "starting" => ServerState::Starting,
            "stopping" => ServerState::Stopping,
            "sleeping" => ServerState::Sleeping,
            "crashed" => ServerState::Crashed,
            "installing" => ServerState::Installing,
            _ => ServerState::Stopped,
        }
    }
}

/// Connection limits and timeouts.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Time allowed to receive the complete handshake (slowloris protection).
    pub handshake_timeout: Duration,
    /// Time allowed to receive Login Start, or Status Request + Ping.
    pub pre_login_timeout: Duration,
    /// Time allowed to connect to a backend for a login.
    pub backend_connect_timeout: Duration,
    /// For status pings: if the backend doesn't accept within this, answer
    /// from the status cache instead.
    pub status_backend_timeout: Duration,
    /// How long to wait for more bytes after a bare legacy `FE`/`FE 01`.
    pub legacy_wait: Duration,
    /// Maximum simultaneous connections across all listeners.
    pub max_connections: usize,
    /// Per-IP connection burst.
    pub per_ip_burst: u32,
    /// Per-IP sustained connections per second.
    pub per_ip_per_second: f64,
    /// Size of pooled copy buffers. Only actively transferring connections
    /// hold one (per direction), so this doesn't scale with idle players.
    pub buffer_size: usize,
    /// How often to refresh the status cache from running servers.
    pub status_refresh: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(3),
            pre_login_timeout: Duration::from_secs(5),
            backend_connect_timeout: Duration::from_secs(5),
            status_backend_timeout: Duration::from_millis(500),
            legacy_wait: Duration::from_millis(150),
            max_connections: 10_000,
            per_ip_burst: 20,
            per_ip_per_second: 5.0,
            buffer_size: 64 * 1024,
            status_refresh: Duration::from_secs(30),
        }
    }
}
