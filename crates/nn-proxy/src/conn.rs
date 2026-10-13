//! Handling of one player connection, from accept to close.
//!
//! 1. Read the handshake (or a legacy ping) within `handshake_timeout`.
//! 2. Resolve the hostname to a server.
//! 3. Branch on what the client wants and the server's state:
//!    - status, server running → forward to the backend (or answer from the
//!      status cache if it doesn't accept within `status_backend_timeout`)
//!    - status, otherwise → answer locally from the cache; never wake
//!    - login, running → forward the buffered bytes and splice both ways
//!    - login, sleeping → ask to wake it and disconnect with a message
//!    - login, other states or unknown host → disconnect with a message
//! 4. On close, record metrics and publish an event.
//!
//! Nothing after Login Start is parsed: the session may be encrypted.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use nn_events::Event;
use nn_mcproto::codec::frame;
use nn_mcproto::legacy::{self, LegacyDetection, MAX_LEGACY_PING_BYTES};
use nn_mcproto::login::login_disconnect;
use nn_mcproto::status::{decode_ping, decode_status_request, encode_ping, encode_status_response};
use nn_mcproto::{
    Handshake, LegacyPing, LoginStart, MAX_HANDSHAKE_FRAME, MAX_LOGIN_START_FRAME,
    MAX_STATUS_FRAME, NextState, StatusResponse, normalize_hostname,
};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::config::{ListenerSpec, ServerState};
use crate::hooks::{PlayerJoin, WakeOutcome};
use crate::io::{FrameBuffer, ReadError};
use crate::registry::ConnectionInfo;
use crate::routes::Resolution;
use crate::{Shared, metrics_names as m, proxy_protocol};

/// Shown when no route matches.
pub const UNKNOWN_HOST_MESSAGE: &str = "Unknown server address. Check the address and try again.";
/// MOTD suffix for sleeping servers in the server list.
pub const SLEEPING_SUFFIX: &str = " 💤 Sleeping — join to wake";

/// How a connection ended, for metrics and events.
struct Summary {
    route: Arc<str>,
    server_id: Option<String>,
    outcome: &'static str,
    bytes_up: u64,
    bytes_down: u64,
}

impl Summary {
    fn new(outcome: &'static str) -> Self {
        Self {
            route: Arc::from("-"),
            server_id: None,
            outcome,
            bytes_up: 0,
            bytes_down: 0,
        }
    }

    fn routed(resolution: Option<&Resolution>, outcome: &'static str) -> Self {
        match resolution {
            Some(r) => Self {
                route: r.route.clone(),
                server_id: Some(r.backend.spec.server_id.clone()),
                ..Self::new(outcome)
            },
            None => Self::new(outcome),
        }
    }
}

enum Initial {
    Legacy(LegacyPing),
    Modern(Handshake),
}

/// Serves one accepted connection to completion.
pub(crate) async fn handle(
    mut client: TcpStream,
    peer: SocketAddr,
    listener: Arc<ListenerSpec>,
    shared: Arc<Shared>,
) {
    let started = Instant::now();
    let _ = client.set_nodelay(true);
    let summary = run(&mut client, peer, &listener, &shared, started).await;

    metrics::counter!(m::CONNECTIONS, "route" => summary.route.to_string(), "result" => summary.outcome)
        .increment(1);
    if summary.bytes_up > 0 || summary.bytes_down > 0 {
        metrics::counter!(m::BYTES, "direction" => "up").increment(summary.bytes_up);
        metrics::counter!(m::BYTES, "direction" => "down").increment(summary.bytes_down);
    }
    tracing::debug!(
        %peer, route = %summary.route, outcome = summary.outcome,
        up = summary.bytes_up, down = summary.bytes_down, "connection closed"
    );
    shared.events.publish(Event::ProxyConnectionClosed {
        route: summary.route.to_string(),
        server_id: summary.server_id,
        outcome: summary.outcome.to_owned(),
        bytes_up: summary.bytes_up,
        bytes_down: summary.bytes_down,
        duration_ms: millis(started),
    });
}

async fn run(
    client: &mut TcpStream,
    peer: SocketAddr,
    listener: &ListenerSpec,
    shared: &Shared,
    started: Instant,
) -> Summary {
    let limits = &shared.limits;
    let mut buf = FrameBuffer::new();
    let initial = match timeout(
        limits.handshake_timeout,
        read_initial(client, &mut buf, shared),
    )
    .await
    {
        Err(_) => return Summary::new("timeout"),
        Ok(Err(_)) => return Summary::new("bad_handshake"),
        Ok(Ok(initial)) => initial,
    };
    metrics::histogram!(m::HANDSHAKE_SECONDS).record(started.elapsed().as_secs_f64());

    let handshake = match initial {
        Initial::Legacy(ping) => {
            return legacy_ping(client, peer, listener, shared, &buf, ping).await;
        }
        Initial::Modern(handshake) => handshake,
    };
    let host = handshake.hostname();
    let resolution = shared.routes.load().resolve(&listener.id, &host);

    match handshake.next_state {
        NextState::Status => {
            status(
                client, peer, listener, shared, &mut buf, &handshake, resolution,
            )
            .await
        }
        NextState::Login | NextState::Transfer => {
            login(
                client, peer, listener, shared, &mut buf, &handshake, &host, resolution,
            )
            .await
        }
    }
}

/// Reads either a legacy ping or a complete modern handshake.
async fn read_initial(
    client: &mut TcpStream,
    buf: &mut FrameBuffer,
    shared: &Shared,
) -> Result<Initial, ReadError> {
    loop {
        if buf.all().first() == Some(&0xfe) {
            match legacy::detect(buf.all(), false) {
                LegacyDetection::Legacy(ping) => return Ok(Initial::Legacy(ping)),
                LegacyDetection::Modern => {}
                LegacyDetection::NeedMore => {
                    if buf.all().len() > MAX_LEGACY_PING_BYTES {
                        return Err(ReadError::Eof);
                    }
                    // Bare `FE` / `FE 01` clients send nothing more and wait.
                    match timeout(shared.limits.legacy_wait, buf.read_more(client)).await {
                        Ok(Ok(_)) => continue,
                        Ok(Err(e)) if !matches!(e, ReadError::Eof) => return Err(e),
                        _ => {
                            return match legacy::detect(buf.all(), true) {
                                LegacyDetection::Legacy(ping) => Ok(Initial::Legacy(ping)),
                                _ => Err(ReadError::Eof),
                            };
                        }
                    }
                }
            }
        }
        if buf.all().is_empty() {
            buf.read_more(client).await?;
            continue;
        }
        let range = buf.next_frame(client, MAX_HANDSHAKE_FRAME).await?;
        return Ok(Initial::Modern(Handshake::decode(buf.payload(range))?));
    }
}

async fn status(
    client: &mut TcpStream,
    peer: SocketAddr,
    listener: &ListenerSpec,
    shared: &Shared,
    buf: &mut FrameBuffer,
    handshake: &Handshake,
    resolution: Option<Resolution>,
) -> Summary {
    if let Some(res) = &resolution
        && res.backend.spec.state == ServerState::Running
        && let Ok(Ok(backend)) = timeout(
            shared.limits.status_backend_timeout,
            TcpStream::connect(res.backend.spec.addr),
        )
        .await
    {
        let mut summary = Summary::routed(Some(res), "status_proxied");
        (summary.bytes_up, summary.bytes_down) = splice(
            client,
            backend,
            peer,
            listener,
            shared,
            &res.route,
            buf.all(),
        )
        .await;
        return summary;
    }

    let response = local_status(shared, resolution.as_ref(), handshake.protocol_version);
    let answered = timeout(
        shared.limits.pre_login_timeout,
        answer_status(client, buf, &response),
    )
    .await;
    let outcome = if matches!(answered, Ok(Ok(()))) {
        "status_local"
    } else {
        "status_aborted"
    };
    Summary::routed(resolution.as_ref(), outcome)
}

/// Builds the status to show when the backend can't answer itself.
fn local_status(
    shared: &Shared,
    resolution: Option<&Resolution>,
    client_protocol: i32,
) -> StatusResponse {
    let Some(res) = resolution else {
        return StatusResponse::simple(
            "NetherNode",
            client_protocol,
            "Unknown server — check the address",
            0,
            0,
        );
    };
    let spec = &res.backend.spec;
    let mut status = shared
        .status
        .get(&spec.server_id)
        .map(|cached| cached.response)
        .unwrap_or_else(|| StatusResponse::simple("NetherNode", client_protocol, &spec.name, 0, 0));
    let suffix = match spec.state {
        ServerState::Running => return status,
        ServerState::Sleeping => SLEEPING_SUFFIX,
        ServerState::Starting => " ⏳ Starting…",
        ServerState::Stopping => " Stopping…",
        ServerState::Stopped | ServerState::Crashed | ServerState::Installing => " ● Offline",
    };
    status.clear_players();
    status.append_to_description(suffix);
    status
}

/// Answers Status Request and the optional Ping on the client's socket.
async fn answer_status(
    client: &mut TcpStream,
    buf: &mut FrameBuffer,
    response: &StatusResponse,
) -> Result<(), ReadError> {
    let request = buf.next_frame(client, MAX_STATUS_FRAME).await?;
    decode_status_request(buf.payload(request))?;
    client
        .write_all(&frame(&encode_status_response(&response.to_json())))
        .await?;
    // Clients usually follow with a ping to measure latency.
    if let Ok(ping) = buf.next_frame(client, MAX_STATUS_FRAME).await
        && let Ok(timestamp) = decode_ping(buf.payload(ping))
    {
        client.write_all(&frame(&encode_ping(timestamp))).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn login(
    client: &mut TcpStream,
    peer: SocketAddr,
    listener: &ListenerSpec,
    shared: &Shared,
    buf: &mut FrameBuffer,
    handshake: &Handshake,
    host: &str,
    resolution: Option<Resolution>,
) -> Summary {
    let login_start = match timeout(
        shared.limits.pre_login_timeout,
        buf.next_frame(client, MAX_LOGIN_START_FRAME),
    )
    .await
    {
        Err(_) => return Summary::routed(resolution.as_ref(), "timeout"),
        Ok(Err(_)) => return Summary::routed(resolution.as_ref(), "bad_login"),
        // Malformed-but-framed Login Start is forwarded; the backend decides.
        Ok(Ok(range)) => LoginStart::decode(buf.payload(range), handshake.protocol_version).ok(),
    };

    let Some(res) = resolution else {
        disconnect(client, UNKNOWN_HOST_MESSAGE).await;
        return Summary::new("unknown_host");
    };
    let spec = &res.backend.spec;
    let name = &spec.name;
    let (outcome, message) = match spec.state {
        ServerState::Running => {
            return forward_login(client, peer, listener, shared, buf, host, &res, login_start)
                .await;
        }
        ServerState::Sleeping => match shared.hooks.wake(&spec.server_id).await {
            WakeOutcome::Starting => (
                "woke",
                format!("{name} is waking up. Try again in about 30 seconds."),
            ),
            WakeOutcome::Unavailable => (
                "wake_unavailable",
                format!("{name} is asleep and can't be started right now."),
            ),
            WakeOutcome::Failed(error) => {
                tracing::warn!(server = %spec.server_id, %error, "wake-on-connect failed");
                (
                    "wake_failed",
                    format!("{name} couldn't be started. Ask an admin to check it."),
                )
            }
        },
        ServerState::Starting => (
            "starting",
            format!("{name} is starting. Try again in about 30 seconds."),
        ),
        ServerState::Stopping => (
            "stopping",
            format!("{name} is stopping. Try again in a moment."),
        ),
        ServerState::Stopped | ServerState::Crashed | ServerState::Installing => {
            ("offline", format!("{name} is offline."))
        }
    };
    disconnect(client, &message).await;
    Summary::routed(Some(&res), outcome)
}

#[allow(clippy::too_many_arguments)]
async fn forward_login(
    client: &mut TcpStream,
    peer: SocketAddr,
    listener: &ListenerSpec,
    shared: &Shared,
    buf: &FrameBuffer,
    host: &str,
    res: &Resolution,
    login_start: Option<LoginStart>,
) -> Summary {
    let spec = &res.backend.spec;
    let backend = match timeout(
        shared.limits.backend_connect_timeout,
        TcpStream::connect(spec.addr),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        _ => {
            disconnect(
                client,
                &format!("Couldn't reach {}. Try again shortly.", spec.name),
            )
            .await;
            return Summary::routed(Some(res), "backend_unreachable");
        }
    };

    let join = login_start.map(|ls| PlayerJoin {
        server_id: spec.server_id.clone(),
        username: ls.username,
        uuid: ls.uuid.map(|u| u.hyphenated().to_string()),
        ip: peer.ip(),
    });

    // Register before announcing, so anyone reacting to PlayerJoined by
    // listing connections sees this one.
    let joined = Instant::now();
    let _guard = shared.connections.register(ConnectionInfo {
        id: 0,
        listener_id: listener.id.clone(),
        client: peer,
        hostname: host.to_owned(),
        server_id: spec.server_id.clone(),
        username: join.as_ref().map(|j| j.username.clone()),
        started_at: std::time::SystemTime::now(),
    });
    let session = match &join {
        Some(join) => {
            let session = shared.hooks.player_joined(join).await;
            shared.events.publish(Event::PlayerJoined {
                server_id: join.server_id.clone(),
                username: join.username.clone(),
                uuid: join.uuid.clone(),
            });
            session
        }
        None => None,
    };

    let mut summary = Summary::routed(Some(res), "spliced");
    (summary.bytes_up, summary.bytes_down) = splice(
        client,
        backend,
        peer,
        listener,
        shared,
        &res.route,
        buf.all(),
    )
    .await;

    if let Some(join) = join {
        if let Some(session_id) = session {
            shared.hooks.player_left(session_id).await;
        }
        shared.events.publish(Event::PlayerLeft {
            server_id: join.server_id,
            username: join.username,
            uuid: join.uuid,
            duration_ms: millis(joined),
        });
    }
    summary
}

async fn legacy_ping(
    client: &mut TcpStream,
    peer: SocketAddr,
    listener: &ListenerSpec,
    shared: &Shared,
    buf: &FrameBuffer,
    ping: LegacyPing,
) -> Summary {
    let host = match &ping {
        LegacyPing::V1_6 {
            hostname: Some(h), ..
        } => normalize_hostname(h),
        _ => String::new(),
    };
    let resolution = shared.routes.load().resolve(&listener.id, &host);
    if let Some(res) = &resolution
        && res.backend.spec.state == ServerState::Running
        && let Ok(Ok(backend)) = timeout(
            shared.limits.status_backend_timeout,
            TcpStream::connect(res.backend.spec.addr),
        )
        .await
    {
        let mut summary = Summary::routed(Some(res), "legacy_proxied");
        (summary.bytes_up, summary.bytes_down) = splice(
            client,
            backend,
            peer,
            listener,
            shared,
            &res.route,
            buf.all(),
        )
        .await;
        return summary;
    }

    let motd = match &resolution {
        Some(res) => format!("{} (update your Minecraft to join)", res.backend.spec.name),
        None => "Unknown server".to_owned(),
    };
    let _ = client
        .write_all(&legacy::kick_response(&ping, "NetherNode", &motd, 0, 0))
        .await;
    let _ = client.shutdown().await;
    Summary::routed(resolution.as_ref(), "legacy_local")
}

/// Sends a login-state Disconnect and closes the connection.
async fn disconnect(client: &mut TcpStream, message: &str) {
    let _ = client.write_all(&frame(&login_disconnect(message))).await;
    let _ = client.shutdown().await;
}

/// Writes the PROXY header (if enabled) and the client's buffered bytes to
/// the backend, then copies both ways until either side closes. Returns
/// (bytes up, bytes down), including the replayed prelude.
async fn splice(
    client: &mut TcpStream,
    mut backend: TcpStream,
    peer: SocketAddr,
    listener: &ListenerSpec,
    shared: &Shared,
    route: &str,
    prelude: &[u8],
) -> (u64, u64) {
    let _ = backend.set_nodelay(true);
    let mut first = Vec::with_capacity(prelude.len() + 52);
    if listener.proxy_protocol {
        let local = client.local_addr().unwrap_or(listener.bind);
        first.extend_from_slice(&proxy_protocol::proxied(peer, local));
    }
    first.extend_from_slice(prelude);
    if backend.write_all(&first).await.is_err() {
        return (0, 0);
    }

    let gauge = metrics::gauge!(m::ACTIVE_CONNECTIONS, "route" => route.to_owned());
    gauge.increment(1.0);
    let (up, down) = crate::splice::copy_bidirectional(client, &mut backend, &shared.buffers).await;
    gauge.decrement(1.0);
    (up + prelude.len() as u64, down)
}

fn millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}
