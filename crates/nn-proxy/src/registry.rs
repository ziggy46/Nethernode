//! Live proxied connections, for the UI.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// A connection currently being spliced to a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// Process-unique ID.
    pub id: u64,
    /// Listener it arrived on.
    pub listener_id: String,
    /// Client address.
    pub client: SocketAddr,
    /// Hostname the client used (normalized).
    pub hostname: String,
    /// Destination server.
    pub server_id: String,
    /// The route pattern that matched (`play.example.com`, `*.example.com`, `*`).
    pub route: String,
    /// Username, for logins.
    pub username: Option<String>,
    /// When splicing started.
    pub started_at: SystemTime,
}

/// The set of live connections.
#[derive(Debug, Default)]
pub struct ConnectionRegistry {
    next_id: AtomicU64,
    live: Mutex<HashMap<u64, ConnectionInfo>>,
}

impl ConnectionRegistry {
    /// Registers a connection; it's removed when the guard drops.
    pub fn register(&self, mut info: ConnectionInfo) -> RegistryGuard<'_> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        info.id = id;
        self.live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, info);
        RegistryGuard { registry: self, id }
    }

    /// A snapshot of live connections, oldest first.
    pub fn snapshot(&self) -> Vec<ConnectionInfo> {
        let mut list: Vec<_> = self
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        list.sort_by_key(|c| (c.started_at, c.id));
        list
    }

    /// Number of live connections.
    pub fn len(&self) -> usize {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether there are no live connections.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Removes its connection from the registry on drop.
#[derive(Debug)]
pub struct RegistryGuard<'a> {
    registry: &'a ConnectionRegistry,
    id: u64,
}

impl Drop for RegistryGuard<'_> {
    fn drop(&mut self) {
        self.registry
            .live
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
