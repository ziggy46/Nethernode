//! In-process event bus.
//!
//! Server state changes, player joins/leaves, log lines, and metrics flow
//! through a single [`tokio::sync::broadcast`] channel. SSE and WebSocket
//! endpoints subscribe to it so nothing in the UI has to poll.

use serde::Serialize;
use tokio::sync::broadcast;

/// Default number of events buffered per subscriber before it starts lagging.
pub const DEFAULT_CAPACITY: usize = 1024;

/// An event published on the bus.
///
/// New variants are added as later phases introduce servers, players, and
/// the proxy.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Event {
    /// A user signed in to the panel.
    UserLoggedIn {
        /// ID of the user.
        user_id: String,
    },
    /// A user signed out of the panel.
    UserLoggedOut {
        /// ID of the user.
        user_id: String,
    },
    /// A player connected to a server through the Smart Proxy.
    PlayerJoined {
        /// Server joined.
        server_id: String,
        /// Username sent at login (verified later by the server, not the proxy).
        username: String,
        /// Player UUID, when the client's protocol version sends one.
        uuid: Option<String>,
    },
    /// A player's proxied connection ended.
    PlayerLeft {
        /// Server left.
        server_id: String,
        /// Username sent at login.
        username: String,
        /// Player UUID, if known.
        uuid: Option<String>,
        /// How long the player was connected.
        duration_ms: u64,
    },
    /// A proxied connection closed.
    ProxyConnectionClosed {
        /// Route pattern that matched, or `-` if none did.
        route: String,
        /// Destination server, if routed.
        server_id: Option<String>,
        /// How the connection ended (e.g. `spliced`, `unknown_host`).
        outcome: String,
        /// Bytes sent from the player to the server.
        bytes_up: u64,
        /// Bytes sent from the server to the player.
        bytes_down: u64,
        /// Connection lifetime.
        duration_ms: u64,
    },
    /// A proxy listener started, stopped, or failed to bind.
    ProxyListenerChanged {
        /// Listener ID.
        listener_id: String,
    },
    /// A server template was saved or deleted.
    TemplatesChanged,
    /// A node's agent connected or disconnected, or a node was added,
    /// renamed, or removed.
    NodeChanged {
        /// Node ID.
        node_id: String,
        /// Whether its agent is connected now.
        online: bool,
    },
    /// A server was created, deleted, or changed lifecycle state.
    ServerStatusChanged {
        /// Server ID.
        server_id: String,
        /// New status (`installing`, `stopped`, `starting`, `running`,
        /// `stopping`, `sleeping`, `crashed`), or `deleted`.
        status: String,
    },
    /// A resource sample for a running server, about once per second.
    ServerStats {
        /// Server ID.
        server_id: String,
        /// CPU use, where 100 = one full core.
        cpu_percent: f64,
        /// Memory in use, excluding page cache.
        memory_bytes: u64,
        /// Container memory limit.
        memory_limit_bytes: u64,
        /// Total bytes received.
        net_rx_bytes: u64,
        /// Total bytes sent.
        net_tx_bytes: u64,
        /// Total bytes read from disk.
        #[serde(default)]
        disk_read_bytes: u64,
        /// Total bytes written to disk.
        #[serde(default)]
        disk_write_bytes: u64,
        /// Players online, from the latest status ping.
        #[serde(default)]
        players_online: Option<u32>,
        /// Average milliseconds per tick, when the server reports it.
        #[serde(default)]
        mspt: Option<f64>,
        /// The slowest 5% of recent ticks took at least this long.
        #[serde(default)]
        mspt_p95: Option<f64>,
        /// Ticks per second achieved (20 is full speed).
        #[serde(default)]
        tps: Option<f64>,
        /// Paused because nobody's online (Minecraft 1.21.2+).
        #[serde(default)]
        paused: bool,
    },
    /// Players online, from the server's status ping.
    ServerPlayers {
        /// Server ID.
        server_id: String,
        /// Players online.
        online: u32,
        /// Player limit.
        max: u32,
    },
    /// Progress downloading a server's container image.
    ImagePullProgress {
        /// Server the image is for.
        server_id: String,
        /// Image reference.
        image: String,
        /// Overall progress, 0–100, once layer sizes are known.
        percent: Option<u8>,
        /// Latest status line from Docker.
        status: String,
    },
    /// A lifecycle operation failed.
    ServerOperationFailed {
        /// Server ID.
        server_id: String,
        /// The operation (`create`, `start`, `stop`, …).
        operation: String,
        /// What went wrong, suitable for display.
        message: String,
    },
    /// A server's player lists or online players changed (bans, operators,
    /// allowlist, or someone joined or left). `None` means every server.
    PlayersChanged {
        /// Server ID.
        server_id: Option<String>,
    },
    /// A background file job (extract, compress) made progress.
    FileJobProgress {
        /// Server whose files are involved.
        server_id: String,
        /// Job ID returned when it started.
        job_id: String,
        /// `extract` or `compress`.
        kind: String,
        /// 0–100.
        percent: u8,
    },
    /// A background file job finished.
    FileJobFinished {
        /// Server whose files are involved.
        server_id: String,
        /// Job ID returned when it started.
        job_id: String,
        /// `extract` or `compress`.
        kind: String,
        /// What happened, suitable for display.
        message: String,
        /// Whether it failed.
        failed: bool,
    },
    /// A resumable upload finished: it's in place, was skipped because the
    /// name was taken, or couldn't be placed.
    UploadFinished {
        /// Destination server.
        server_id: String,
        /// Upload ID.
        upload_id: String,
        /// Where the file ended up, relative to the server root (empty if
        /// it wasn't placed).
        path: String,
        /// What happened, suitable for display.
        message: String,
        /// The name was taken and the upload was discarded, as asked.
        skipped: bool,
        /// Whether it failed.
        failed: bool,
    },
}

/// Cloneable handle to the process-wide event bus.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    /// Creates a bus that buffers up to `capacity` events per subscriber.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publishes an event to all current subscribers.
    ///
    /// Returns the number of subscribers that received it. Publishing with no
    /// subscribers is not an error.
    pub fn publish(&self, event: Event) -> usize {
        self.tx.send(event).unwrap_or(0)
    }

    /// Subscribes to events published after this call.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_events() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let event = Event::UserLoggedIn {
            user_id: "u1".into(),
        };
        assert_eq!(bus.publish(event.clone()), 1);
        assert_eq!(rx.recv().await.unwrap(), event);
    }

    #[test]
    fn publishing_without_subscribers_is_ok() {
        let bus = EventBus::default();
        assert_eq!(
            bus.publish(Event::UserLoggedOut {
                user_id: "u1".into()
            }),
            0
        );
    }
}
