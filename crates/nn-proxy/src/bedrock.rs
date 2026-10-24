//! Bedrock listeners: UDP ports that relay RakNet to a server.
//!
//! A Bedrock client doesn't tell the proxy which address it typed until
//! long after the connection starts, so a Bedrock server gets a port of its
//! own and everything arriving on it goes to that server. The proxy:
//!
//! - answers server list pings itself when the server isn't running (with
//!   a "sleeping" or "starting" name), and asks the server when it is;
//! - wakes a sleeping server when someone tries to join (the client shows
//!   "unable to connect"; a retry in a moment works);
//! - relays a running server's sessions datagram by datagram, one backend
//!   socket per client, for as long as the session stays active.
//!
//! Servers on other machines are reached through the hooks' tunnel, which
//! carries each datagram behind a two-byte length.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use arc_swap::ArcSwapAny;
use nn_mcproto::bedrock::{self as raknet, Packet, Status};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use crate::Shared;
use crate::config::{BackendSpec, ListenerKind, ProxyConfig, ServerState};
use crate::conn::SLEEPING_SUFFIX;
use crate::hooks::{Admission, Tunnel};
use crate::registry::ConnectionInfo;
use crate::routes::Backend;
use crate::{ListenerSpec, metrics_names as m};

/// How long a session may stay silent before it's let go. Connected
/// clients and servers keep each other alive every few seconds.
const SESSION_IDLE: Duration = Duration::from_secs(60);
/// How long a running server has to answer a ping before the cached answer
/// is used.
const PING_TIMEOUT: Duration = Duration::from_millis(500);
/// A sleeping server is asked to wake at most this often.
const WAKE_EVERY: Duration = Duration::from_secs(10);
/// Datagrams queued for one session before more are dropped (UDP may drop).
const QUEUE: usize = 256;

/// MOTD suffix while a server boots.
pub const STARTING_SUFFIX: &str = " ⏳ Starting…";
/// MOTD suffix for a server an admin stopped.
pub const OFFLINE_SUFFIX: &str = " ● Offline";

/// What the Bedrock side of the proxy remembers.
#[derive(Default)]
pub(crate) struct State {
    /// Last status each server answered with.
    statuses: RwLock<HashMap<String, Status>>,
    /// When each server was last asked to wake.
    woken: Mutex<HashMap<String, Instant>>,
    /// Servers that Bedrock listeners route to.
    servers: RwLock<HashSet<String>>,
}

impl State {
    /// Records which servers Bedrock listeners route to.
    pub(crate) fn set_servers(&self, config: &ProxyConfig) {
        let listeners: HashSet<&str> = config
            .listeners
            .iter()
            .filter(|l| l.kind == ListenerKind::Bedrock)
            .map(|l| l.id.as_str())
            .collect();
        let servers: HashSet<String> = config
            .routes
            .iter()
            .filter(|r| {
                r.listener_id
                    .as_deref()
                    .is_some_and(|id| listeners.contains(id))
            })
            .filter_map(|r| r.server_id.clone())
            .collect();
        self.statuses
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id, _| servers.contains(id));
        *self.servers.write().unwrap_or_else(|e| e.into_inner()) = servers;
    }

    /// Whether `server_id` is reached through a Bedrock listener.
    pub(crate) fn is_bedrock(&self, server_id: &str) -> bool {
        self.servers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(server_id)
    }

    fn status(&self, server_id: &str) -> Option<Status> {
        self.statuses
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(server_id)
            .cloned()
    }

    fn put(&self, server_id: &str, status: Status) {
        self.statuses
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(server_id.to_owned(), status);
    }

    fn should_wake(&self, server_id: &str) -> bool {
        let mut woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        let due = woken
            .get(server_id)
            .is_none_or(|at| at.elapsed() >= WAKE_EVERY);
        if due {
            woken.insert(server_id.to_owned(), Instant::now());
        }
        due
    }
}

/// Live sessions on one listener, by client address.
type Sessions = Arc<Mutex<HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>>>;

/// Removes a session from the table when its task ends.
struct SessionGuard {
    sessions: Sessions,
    peer: SocketAddr,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.peer);
    }
}

/// Serves a Bedrock listener until its task is aborted.
pub(crate) async fn serve(
    socket: UdpSocket,
    spec: Arc<ArcSwapAny<Arc<ListenerSpec>>>,
    shared: Arc<Shared>,
) {
    let socket = Arc::new(socket);
    let sessions: Sessions = Arc::default();
    let mut buf = vec![0u8; raknet::MAX_DATAGRAM];
    loop {
        let (len, peer) = match socket.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(error) => {
                // Windows reports an earlier send to a closed port here;
                // anything else that repeats shouldn't spin.
                tracing::debug!(%error, "bedrock receive failed");
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
        };
        let data = &buf[..len];

        let existing = sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&peer)
            .cloned();
        if let Some(queue) = existing {
            // A full queue drops the datagram, as a congested network would.
            let _ = queue.try_send(data.to_vec());
            continue;
        }

        let packet = raknet::classify(data);
        if packet == Packet::Other {
            continue;
        }
        let listener = spec.load_full();
        let Some(resolution) = shared.routes.load().resolve(&listener.id, "") else {
            continue;
        };
        if !shared.ip_limiter.allow(peer.ip()) {
            metrics::counter!(m::CONNECTIONS, "route" => "-", "result" => "rate_limited")
                .increment(1);
            continue;
        }
        let backend = resolution.backend;
        match packet {
            Packet::Ping { time } => {
                tokio::spawn(answer_ping(
                    socket.clone(),
                    peer,
                    time,
                    backend,
                    shared.clone(),
                ));
            }
            Packet::OpenConnection => {
                join(
                    &socket,
                    &sessions,
                    peer,
                    data,
                    &listener,
                    resolution.route,
                    backend,
                    &shared,
                )
                .await;
            }
            Packet::Other => {}
        }
    }
}

/// Handles a client starting to join: relays it if the server is up, wakes
/// it if it sleeps.
#[allow(clippy::too_many_arguments)]
async fn join(
    socket: &Arc<UdpSocket>,
    sessions: &Sessions,
    peer: SocketAddr,
    first: &[u8],
    listener: &Arc<ListenerSpec>,
    route: Arc<str>,
    backend: Arc<Backend>,
    shared: &Arc<Shared>,
) {
    let server_id = backend.spec.server_id.clone();
    match backend.spec.state {
        ServerState::Running => {}
        ServerState::Sleeping => {
            if shared.bedrock.should_wake(&server_id) {
                tracing::info!(server = %server_id, %peer, "a Bedrock player tried to join a sleeping server");
                let _ = shared.hooks.wake(&server_id).await;
            }
            metrics::counter!(m::CONNECTIONS, "route" => route.to_string(), "result" => "waking")
                .increment(1);
            return;
        }
        _ => {
            metrics::counter!(m::CONNECTIONS, "route" => route.to_string(), "result" => "not_running")
                .increment(1);
            return;
        }
    }
    let admission = Admission {
        server_id: server_id.clone(),
        ip: peer.ip(),
        username: None,
        uuid: None,
    };
    if shared.hooks.admit(&admission).await.is_some() {
        metrics::counter!(m::CONNECTIONS, "route" => route.to_string(), "result" => "banned")
            .increment(1);
        return;
    }
    let Ok(permit) = shared.slots.clone().try_acquire_owned() else {
        metrics::counter!(m::CONNECTIONS, "route" => route.to_string(), "result" => "over_capacity")
            .increment(1);
        return;
    };

    let (queue, inbox) = mpsc::channel::<Vec<u8>>(QUEUE);
    let _ = queue.try_send(first.to_vec());
    sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(peer, queue);
    let guard = SessionGuard {
        sessions: sessions.clone(),
        peer,
    };
    let (socket, shared, listener_id) = (socket.clone(), shared.clone(), listener.id.clone());
    tokio::spawn(async move {
        let _permit = permit;
        let _guard = guard;
        let _live = shared.connections.register(ConnectionInfo {
            id: 0,
            listener_id,
            client: peer,
            hostname: String::new(),
            server_id: backend.spec.server_id.clone(),
            route: route.to_string(),
            username: None,
            started_at: SystemTime::now(),
        });
        let started = Instant::now();
        let gauge = metrics::gauge!(m::ACTIVE_CONNECTIONS, "route" => route.to_string());
        gauge.increment(1.0);
        let outcome = if backend.spec.remote {
            relay_remote(&socket, peer, inbox, &backend.spec, &shared).await
        } else {
            relay_local(&socket, peer, inbox, backend.spec.addr).await
        };
        gauge.decrement(1.0);
        tracing::debug!(%peer, route = %route, ?outcome, took = ?started.elapsed(), "bedrock session ended");
        metrics::counter!(m::CONNECTIONS, "route" => route.to_string(), "result" => "bedrock")
            .increment(1);
    });
}

/// How a session ended.
#[derive(Debug, Clone, Copy)]
enum Ended {
    Idle,
    BackendGone,
    Unreachable,
}

/// Relays one client's datagrams to a server on this machine.
async fn relay_local(
    socket: &UdpSocket,
    peer: SocketAddr,
    mut inbox: mpsc::Receiver<Vec<u8>>,
    backend: SocketAddr,
) -> Ended {
    let bind: SocketAddr = if backend.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let Ok(upstream) = UdpSocket::bind(bind).await else {
        return Ended::Unreachable;
    };
    if upstream.connect(backend).await.is_err() {
        return Ended::Unreachable;
    }
    let mut buf = vec![0u8; raknet::MAX_DATAGRAM];
    loop {
        tokio::select! {
            outgoing = inbox.recv() => {
                let Some(datagram) = outgoing else { return Ended::Idle };
                if upstream.send(&datagram).await.is_err() {
                    return Ended::BackendGone;
                }
            }
            incoming = upstream.recv(&mut buf) => {
                let Ok(len) = incoming else { return Ended::BackendGone };
                let _ = socket.send_to(&buf[..len], peer).await;
            }
            () = tokio::time::sleep(SESSION_IDLE) => return Ended::Idle,
        }
    }
}

/// Relays one client's datagrams to a server on another machine, through
/// a tunnel that carries each behind a two-byte length.
async fn relay_remote(
    socket: &UdpSocket,
    peer: SocketAddr,
    mut inbox: mpsc::Receiver<Vec<u8>>,
    spec: &BackendSpec,
    shared: &Shared,
) -> Ended {
    let Ok(tunnel) = shared.hooks.dial_udp(&spec.server_id).await else {
        return Ended::Unreachable;
    };
    let (mut read, mut write) = tokio::io::split(tunnel);
    let up = async {
        while let Ok(Some(datagram)) = tokio::time::timeout(SESSION_IDLE, inbox.recv()).await {
            if write_datagram(&mut write, &datagram).await.is_err() {
                return Ended::BackendGone;
            }
        }
        Ended::Idle
    };
    let down = async {
        loop {
            match tokio::time::timeout(SESSION_IDLE, read_datagram(&mut read)).await {
                Ok(Ok(datagram)) => {
                    let _ = socket.send_to(&datagram, peer).await;
                }
                Ok(Err(_)) => return Ended::BackendGone,
                Err(_) => return Ended::Idle,
            }
        }
    };
    tokio::select! {
        ended = up => ended,
        ended = down => ended,
    }
}

/// Writes one datagram to a tunnel.
pub async fn write_datagram<W>(out: &mut W, datagram: &[u8]) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let len = u16::try_from(datagram.len())
        .map_err(|_| std::io::Error::other("datagram too large for the tunnel"))?;
    let mut framed = Vec::with_capacity(2 + datagram.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(datagram);
    out.write_all(&framed).await
}

/// Reads one datagram from a tunnel.
pub async fn read_datagram<R>(input: &mut R) -> std::io::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut len = [0u8; 2];
    input.read_exact(&mut len).await?;
    let mut datagram = vec![0u8; usize::from(u16::from_be_bytes(len))];
    input.read_exact(&mut datagram).await?;
    Ok(datagram)
}

/// Answers a server list ping.
async fn answer_ping(
    socket: Arc<UdpSocket>,
    peer: SocketAddr,
    time: i64,
    backend: Arc<Backend>,
    shared: Arc<Shared>,
) {
    let status = current_status(&shared, &backend.spec).await;
    let guid = status.guid.parse::<u64>().map(|g| g as i64).unwrap_or(0);
    let pong = raknet::encode_pong(time, guid, &status.render());
    let _ = socket.send_to(&pong, peer).await;
}

/// What the server list should show for a server: its own answer while it
/// runs, otherwise the last one with a note on its state.
async fn current_status(shared: &Shared, spec: &BackendSpec) -> Status {
    let last = shared.bedrock.status(&spec.server_id);
    if spec.state == ServerState::Running {
        if let Some(fresh) = ask(shared, spec, PING_TIMEOUT).await {
            shared.bedrock.put(&spec.server_id, fresh.clone());
            return fresh;
        }
        if let Some(last) = last {
            return last;
        }
        return Status::placeholder(&format!("{}{STARTING_SUFFIX}", spec.name), None, 20);
    }
    let name = last
        .as_ref()
        .map_or(spec.name.as_str(), |s| s.motd.as_str());
    let note = match spec.state {
        ServerState::Sleeping => SLEEPING_SUFFIX,
        ServerState::Starting | ServerState::Installing => STARTING_SUFFIX,
        _ => OFFLINE_SUFFIX,
    };
    // Only the server's own answers are cached, so the name has no note yet.
    Status::placeholder(&format!("{name}{note}"), last.as_ref(), 20)
}

/// Asks a running server for its status.
async fn ask(shared: &Shared, spec: &BackendSpec, timeout: Duration) -> Option<Status> {
    let pong = if spec.remote {
        ping_through(shared, spec, timeout).await?
    } else {
        raknet::ping(spec.addr, timeout).await.ok()?
    };
    Status::parse(&pong.status)
}

/// Pings a server on another machine through a tunnel to it.
async fn ping_through(
    shared: &Shared,
    spec: &BackendSpec,
    timeout: Duration,
) -> Option<raknet::Pong> {
    tokio::time::timeout(timeout, async {
        let mut tunnel: Box<dyn Tunnel> = shared.hooks.dial_udp(&spec.server_id).await.ok()?;
        write_datagram(&mut tunnel, &raknet::encode_ping(0, 0x4e4e))
            .await
            .ok()?;
        let reply = read_datagram(&mut tunnel).await.ok()?;
        raknet::decode_pong(&reply)
    })
    .await
    .ok()
    .flatten()
}

/// Refreshes the cached status of every running Bedrock server, so the
/// server list has something to show while one is slow or remote.
pub(crate) async fn refresh(shared: &Arc<Shared>) {
    let table = shared.routes.load_full();
    let mut asks = tokio::task::JoinSet::new();
    for backend in table.backends().filter(|b| {
        b.spec.state == ServerState::Running && shared.bedrock.is_bedrock(&b.spec.server_id)
    }) {
        let spec = backend.spec.clone();
        let shared = shared.clone();
        asks.spawn(async move {
            let status = ask(&shared, &spec, Duration::from_secs(3)).await;
            (spec.server_id, status)
        });
    }
    while let Some(Ok((server_id, status))) = asks.join_next().await {
        if let Some(status) = status {
            shared.bedrock.put(&server_id, status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn datagrams_cross_a_tunnel_with_their_length() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_datagram(&mut a, b"hello").await.unwrap();
        write_datagram(&mut a, &[]).await.unwrap();
        assert_eq!(read_datagram(&mut b).await.unwrap(), b"hello");
        assert!(read_datagram(&mut b).await.unwrap().is_empty());
        let too_big = vec![0u8; 70_000];
        assert!(write_datagram(&mut a, &too_big).await.is_err());
    }

    #[test]
    fn wakes_are_throttled() {
        let state = State::default();
        assert!(state.should_wake("a"));
        assert!(!state.should_wake("a"));
        assert!(state.should_wake("b"));
    }
}
