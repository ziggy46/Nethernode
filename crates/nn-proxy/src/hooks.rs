//! Callbacks from the proxy into the rest of NetherNode.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A connection to a server reached some other way than TCP (through the
/// agent on its machine, say).
pub trait Tunnel: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin {}

impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin> Tunnel for T {}

/// The result of asking for a sleeping server to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    /// The start command was accepted.
    Starting,
    /// Starting servers isn't possible right now (e.g. no orchestrator).
    Unavailable,
    /// The start command failed.
    Failed(String),
}

/// A player about to be spliced through to a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerJoin {
    /// Destination server.
    pub server_id: String,
    /// Username from Login Start.
    pub username: String,
    /// UUID from Login Start, hyphenated, if the protocol sends one.
    pub uuid: Option<String>,
    /// Client address.
    pub ip: IpAddr,
}

/// A login about to be routed, for [`ProxyHooks::admit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    /// Destination server.
    pub server_id: String,
    /// Client address. Servers behind the proxy only ever see the proxy's
    /// address, so IP bans are enforced here.
    pub ip: IpAddr,
    /// Username from Login Start, if it parsed.
    pub username: Option<String>,
    /// UUID from Login Start, if the protocol sends one.
    pub uuid: Option<String>,
}

/// What the proxy needs from the rest of the system. Implementations must
/// be quick: calls happen on the connection path.
pub trait ProxyHooks: Send + Sync + 'static {
    /// Connects to a server on another machine ([`crate::BackendSpec::remote`]).
    /// An error of kind `NotConnected` means its machine is offline.
    fn dial<'a>(&'a self, _server_id: &'a str) -> BoxFuture<'a, std::io::Result<Box<dyn Tunnel>>> {
        Box::pin(async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no way to reach other machines",
            ))
        })
    }

    /// Opens a tunnel to a Bedrock server on another machine. Each
    /// datagram crosses it behind a two-byte length ([`crate::bedrock`]).
    fn dial_udp<'a>(
        &'a self,
        _server_id: &'a str,
    ) -> BoxFuture<'a, std::io::Result<Box<dyn Tunnel>>> {
        Box::pin(async {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no way to reach other machines",
            ))
        })
    }

    /// Starts a sleeping server.
    fn wake<'a>(&'a self, server_id: &'a str) -> BoxFuture<'a, WakeOutcome>;

    /// Records a join. Returns a session ID passed back to [`ProxyHooks::player_left`].
    fn player_joined<'a>(&'a self, join: &'a PlayerJoin) -> BoxFuture<'a, Option<String>>;

    /// Records that a session ended.
    fn player_left(&self, session_id: String) -> BoxFuture<'_, ()>;

    /// Whether to let a login through. `Some(message)` disconnects the
    /// player with that message, before any server is contacted or woken.
    fn admit<'a>(&'a self, _login: &'a Admission) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

/// Hooks that do nothing: servers can't be woken and nothing is recorded.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopHooks;

impl ProxyHooks for NoopHooks {
    fn wake<'a>(&'a self, _server_id: &'a str) -> BoxFuture<'a, WakeOutcome> {
        Box::pin(async { WakeOutcome::Unavailable })
    }

    fn player_joined<'a>(&'a self, _join: &'a PlayerJoin) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }

    fn player_left(&self, _session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}
