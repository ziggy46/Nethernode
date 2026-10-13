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
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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
