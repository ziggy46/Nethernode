//! The last known status of each server, so pings can be answered while a
//! server sleeps or responds slowly.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use nn_mcproto::StatusResponse;

use crate::proxy_protocol;

/// A cached status response.
#[derive(Debug, Clone)]
pub struct CachedStatus {
    /// The server's response.
    pub response: StatusResponse,
    /// When it was fetched.
    pub fetched_at: Instant,
}

/// Last known status per server ID.
#[derive(Debug, Default)]
pub struct StatusCache {
    entries: RwLock<HashMap<String, CachedStatus>>,
}

impl StatusCache {
    /// The cached status for a server.
    pub fn get(&self, server_id: &str) -> Option<CachedStatus> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(server_id)
            .cloned()
    }

    /// Stores a fresh status.
    pub fn put(&self, server_id: &str, response: StatusResponse) {
        let entry = CachedStatus {
            response,
            fetched_at: Instant::now(),
        };
        self.entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(server_id.to_owned(), entry);
    }

    /// Drops entries for servers that no longer exist.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id, _| keep(id));
    }
}

pub use nn_mcproto::client::PingError;

/// Asks a backend for its status, as a client would.
///
/// When the backend expects PROXY protocol, a `LOCAL` header is sent first
/// (the form meant for health checks).
pub async fn ping_backend(
    addr: SocketAddr,
    proxy_protocol: bool,
    timeout: Duration,
) -> Result<StatusResponse, PingError> {
    let prelude = if proxy_protocol {
        proxy_protocol::local()
    } else {
        Vec::new()
    };
    nn_mcproto::client::ping(addr, &prelude, timeout).await
}

/// Asks a server on another machine for its status, through the hooks'
/// tunnel to it.
pub async fn ping_through(
    hooks: &dyn crate::ProxyHooks,
    spec: &crate::BackendSpec,
    proxy_protocol: bool,
    timeout: Duration,
) -> Result<StatusResponse, PingError> {
    let prelude = if proxy_protocol {
        proxy_protocol::local()
    } else {
        Vec::new()
    };
    tokio::time::timeout(timeout, async {
        let mut tunnel = hooks.dial(&spec.server_id).await?;
        nn_mcproto::client::ping_on(&mut tunnel, "localhost", spec.addr.port(), &prelude).await
    })
    .await
    .map_err(|_| PingError::Timeout)?
}
