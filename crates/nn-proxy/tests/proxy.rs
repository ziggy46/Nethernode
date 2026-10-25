//! End-to-end proxy tests over real TCP with in-process fake backends.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nn_events::EventBus;
use nn_mcproto::codec::{Reader, frame};
use nn_mcproto::login::login_disconnect;
use nn_mcproto::status::{decode_ping, decode_status_response, encode_ping, encode_status_request};
use nn_mcproto::{Handshake, LoginStart, NextState, StatusResponse, legacy, parse_frame};
use nn_proxy::{
    Admission, BackendSpec, BoxFuture, Limits, ListenerSpec, ListenerState, PlayerJoin, Proxy,
    ProxyConfig, ProxyHooks, RouteSpec, SLEEPING_SUFFIX, ServerState, UNKNOWN_HOST_MESSAGE,
    WakeOutcome,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use uuid::Uuid;

const PROTOCOL: i32 = 777;
const UUID: Uuid = Uuid::from_u128(0x069a79f4_44e9_4726_a5be_fca90e38aaf5);
const BACKEND_HELLO: &[u8] = b"BACKEND";

// ---------- fixtures ----------

#[derive(Default)]
struct RecordingHooks {
    /// Usernames to refuse at login.
    refuse: Mutex<Vec<String>>,
    wakes: Mutex<Vec<String>>,
    joins: Mutex<Vec<PlayerJoin>>,
    lefts: Mutex<Vec<String>>,
}

impl ProxyHooks for RecordingHooks {
    fn wake<'a>(&'a self, server_id: &'a str) -> BoxFuture<'a, WakeOutcome> {
        self.wakes.lock().unwrap().push(server_id.to_owned());
        Box::pin(async { WakeOutcome::Starting })
    }

    fn player_joined<'a>(&'a self, join: &'a PlayerJoin) -> BoxFuture<'a, Option<String>> {
        self.joins.lock().unwrap().push(join.clone());
        Box::pin(async { Some("session-1".to_owned()) })
    }

    fn player_left(&self, session_id: String) -> BoxFuture<'_, ()> {
        self.lefts.lock().unwrap().push(session_id);
        Box::pin(async {})
    }

    fn admit<'a>(&'a self, login: &'a Admission) -> BoxFuture<'a, Option<String>> {
        let refused = login
            .username
            .as_ref()
            .is_some_and(|u| self.refuse.lock().unwrap().contains(u));
        Box::pin(async move { refused.then(|| "You are banned from this server.".to_owned()) })
    }
}

/// A backend that reports what it receives on a channel, greets the client,
/// and then echoes.
async fn fake_backend() -> (SocketAddr, mpsc::UnboundedReceiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut received = Vec::new();
                let mut chunk = [0u8; 4096];
                // Collect the replayed prelude (arrives in one burst).
                if let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_secs(2), stream.read(&mut chunk)).await
                {
                    received.extend_from_slice(&chunk[..n]);
                }
                while let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_millis(50), stream.read(&mut chunk)).await
                {
                    if n == 0 {
                        break;
                    }
                    received.extend_from_slice(&chunk[..n]);
                }
                let _ = tx.send(received);
                let _ = stream.write_all(BACKEND_HELLO).await;
                // Echo until the client goes away.
                loop {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&chunk[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    (addr, rx)
}

/// A backend that answers status pings like a Minecraft server.
async fn fake_status_server(motd: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = nn_proxy::io::FrameBuffer::new();
                let Ok(_) = buf.next_frame(&mut stream, 2048).await else {
                    return;
                };
                let Ok(_) = buf.next_frame(&mut stream, 64).await else {
                    return;
                };
                let status = StatusResponse::simple("Paper 1.21.4", 769, motd, 3, 20);
                let reply = nn_mcproto::status::encode_status_response(&status.to_json());
                let _ = stream.write_all(&frame(&reply)).await;
                if let Ok(ping) = buf.next_frame(&mut stream, 64).await {
                    let ts = decode_ping(buf.payload(ping)).unwrap();
                    let _ = stream.write_all(&frame(&encode_ping(ts))).await;
                }
            });
        }
    });
    addr
}

fn backend(id: &str, addr: SocketAddr, state: ServerState) -> BackendSpec {
    BackendSpec {
        remote: false,
        server_id: id.into(),
        name: format!("{id} server"),
        slug: id.into(),
        addr,
        state,
    }
}

fn route(pattern: &str, server: Option<&str>) -> RouteSpec {
    RouteSpec {
        id: pattern.into(),
        pattern: pattern.into(),
        server_id: server.map(Into::into),
        listener_id: None,
        priority: 0,
    }
}

struct Harness {
    proxy: Proxy,
    addr: SocketAddr,
    hooks: Arc<RecordingHooks>,
    events: EventBus,
}

async fn start(
    routes: Vec<RouteSpec>,
    backends: Vec<BackendSpec>,
    limits: Limits,
    proxy_protocol: bool,
) -> Harness {
    let hooks = Arc::new(RecordingHooks::default());
    let events = EventBus::default();
    let proxy = Proxy::new(hooks.clone(), events.clone(), limits);
    let listener = ListenerSpec {
        id: "main".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        proxy_protocol,
    };
    let statuses = proxy
        .apply(ProxyConfig {
            listeners: vec![listener],
            routes,
            backends,
        })
        .await;
    let ListenerState::Listening(addr) = statuses[0].state.clone() else {
        panic!("listener failed: {statuses:?}")
    };
    Harness {
        proxy,
        addr,
        hooks,
        events,
    }
}

fn handshake(address: &str, next: NextState) -> Vec<u8> {
    let hs = Handshake {
        protocol_version: PROTOCOL,
        server_address: address.into(),
        server_port: 25565,
        next_state: next,
    };
    frame(&hs.encode())
}

fn login_start(name: &str) -> Vec<u8> {
    frame(
        &LoginStart {
            username: name.into(),
            uuid: Some(UUID),
        }
        .encode(PROTOCOL),
    )
}

async fn connect(addr: SocketAddr) -> TcpStream {
    TcpStream::connect(addr).await.unwrap()
}

async fn read_exact(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0; n];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    buf
}

/// Reads one frame from the proxy and returns its payload.
async fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    loop {
        if let Some(f) = parse_frame(&buf, 1 << 20).unwrap() {
            return buf[f.payload].to_vec();
        }
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0, "connection closed before a full frame");
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Reads `n` frames, keeping bytes that arrive together in one buffer.
async fn read_frames(stream: &mut TcpStream, n: usize) -> Vec<Vec<u8>> {
    let mut buf = nn_proxy::io::FrameBuffer::new();
    let mut frames = Vec::new();
    for _ in 0..n {
        let range = tokio::time::timeout(Duration::from_secs(3), buf.next_frame(stream, 1 << 20))
            .await
            .unwrap()
            .unwrap();
        frames.push(buf.payload(range).to_vec());
    }
    frames
}

/// Asserts the proxy closes the connection without sending anything.
async fn assert_closed_silently(stream: &mut TcpStream, within: Duration) {
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(within, stream.read(&mut buf))
        .await
        .expect("not closed in time")
        .unwrap_or(0);
    assert_eq!(n, 0, "expected close, got {:?}", &buf[..n]);
}

fn disconnect_reason(payload: &[u8]) -> String {
    let mut r = Reader::new(payload);
    assert_eq!(r.varint().unwrap(), 0x00, "login Disconnect");
    let json: serde_json::Value = serde_json::from_str(r.string(262_144).unwrap()).unwrap();
    json["text"].as_str().unwrap().to_owned()
}

// ---------- tests ----------

#[tokio::test]
async fn routes_by_hostname_and_replays_original_bytes() {
    let (a_addr, mut a_rx) = fake_backend().await;
    let (b_addr, mut b_rx) = fake_backend().await;
    let h = start(
        vec![
            route("a.example.com", Some("a")),
            route("b.example.com", Some("b")),
        ],
        vec![
            backend("a", a_addr, ServerState::Running),
            backend("b", b_addr, ServerState::Running),
        ],
        Limits::default(),
        false,
    )
    .await;

    let mut client = connect(h.addr).await;
    // Mixed case and FQDN form both resolve to b.example.com.
    let mut sent = handshake("B.Example.com.", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();

    assert_eq!(
        read_exact(&mut client, BACKEND_HELLO.len()).await,
        BACKEND_HELLO
    );
    assert_eq!(
        b_rx.recv().await.unwrap(),
        sent,
        "backend gets the client's exact bytes"
    );
    client.write_all(b"ping").await.unwrap();
    assert_eq!(
        read_exact(&mut client, 4).await,
        b"ping",
        "spliced both ways"
    );
    assert!(a_rx.try_recv().is_err(), "other backend untouched");
}

#[tokio::test]
async fn forge_markers_route_and_are_preserved() {
    let (addr, mut rx) = fake_backend().await;
    let h = start(
        vec![route("mods.example.com", Some("forge"))],
        vec![backend("forge", addr, ServerState::Running)],
        Limits::default(),
        false,
    )
    .await;
    for marker in ["\0FML\0", "\0FML2\0", "\0FML3\0"] {
        let mut client = connect(h.addr).await;
        let mut sent = handshake(&format!("mods.example.com{marker}"), NextState::Login);
        sent.extend(login_start("Alex"));
        client.write_all(&sent).await.unwrap();
        assert_eq!(
            read_exact(&mut client, BACKEND_HELLO.len()).await,
            BACKEND_HELLO,
            "{marker:?}"
        );
        let received = rx.recv().await.unwrap();
        assert_eq!(received, sent);
    }
}

#[tokio::test]
async fn wildcard_and_default_routes() {
    let (smp, mut smp_rx) = fake_backend().await;
    let (lobby, mut lobby_rx) = fake_backend().await;
    let h = start(
        vec![route("*.mc.example.com", None), route("*", Some("lobby"))],
        vec![
            backend("smp", smp, ServerState::Running),
            backend("lobby", lobby, ServerState::Running),
        ],
        Limits::default(),
        false,
    )
    .await;
    for (host, rx) in [
        ("smp.mc.example.com", &mut smp_rx),
        ("whatever.net", &mut lobby_rx),
    ] {
        let mut client = connect(h.addr).await;
        let mut sent = handshake(host, NextState::Login);
        sent.extend(login_start("Steve"));
        client.write_all(&sent).await.unwrap();
        assert_eq!(
            read_exact(&mut client, BACKEND_HELLO.len()).await,
            BACKEND_HELLO,
            "{host}"
        );
        assert_eq!(rx.recv().await.unwrap(), sent);
    }
}

#[tokio::test]
async fn unknown_host_login_gets_a_disconnect_message() {
    let h = start(vec![], vec![], Limits::default(), false).await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("nowhere.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    assert_eq!(
        disconnect_reason(&read_frame(&mut client).await),
        UNKNOWN_HOST_MESSAGE
    );
    assert_eq!(login_disconnect(UNKNOWN_HOST_MESSAGE)[0], 0x00);
}

#[tokio::test]
async fn unknown_host_status_is_answered_locally() {
    let h = start(vec![], vec![], Limits::default(), false).await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("nowhere.example.com", NextState::Status);
    sent.extend(frame(&encode_status_request()));
    sent.extend(frame(&encode_ping(42)));
    client.write_all(&sent).await.unwrap();
    let frames = read_frames(&mut client, 2).await;
    let status = StatusResponse::from_json(&decode_status_response(&frames[0]).unwrap()).unwrap();
    assert!(
        status
            .description
            .unwrap()
            .to_string()
            .contains("Unknown server")
    );
    assert_eq!(
        status.version.protocol, PROTOCOL,
        "echoes the client's protocol"
    );
    assert_eq!(decode_ping(&frames[1]).unwrap(), 42);
}

#[tokio::test]
async fn rejects_oversize_handshakes_immediately() {
    let h = start(vec![], vec![], Limits::default(), false).await;
    let mut client = connect(h.addr).await;
    // Declares a 16383-byte handshake; limit is 2 KiB.
    client.write_all(&[0xff, 0x7f, 0x00]).await.unwrap();
    assert_closed_silently(&mut client, Duration::from_millis(500)).await;

    // A handshake whose address exceeds 255 characters.
    let mut client = connect(h.addr).await;
    let long = Handshake {
        protocol_version: PROTOCOL,
        server_address: "a".repeat(300),
        server_port: 25565,
        next_state: NextState::Login,
    };
    client.write_all(&frame(&long.encode())).await.unwrap();
    assert_closed_silently(&mut client, Duration::from_millis(500)).await;
}

#[tokio::test]
async fn slow_handshakes_time_out() {
    let limits = Limits {
        handshake_timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let h = start(vec![], vec![], limits, false).await;
    let mut client = connect(h.addr).await;
    let started = Instant::now();
    client.write_all(&[0x10, 0x00]).await.unwrap(); // a partial handshake, then nothing
    assert_closed_silently(&mut client, Duration::from_secs(2)).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(150),
        "closed too early: {elapsed:?}"
    );
}

#[tokio::test]
async fn sleeping_servers_answer_pings_from_cache_without_waking() {
    let (addr, _rx) = fake_backend().await;
    let h = start(
        vec![route("smp.example.com", Some("smp"))],
        vec![backend("smp", addr, ServerState::Sleeping)],
        Limits::default(),
        false,
    )
    .await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("smp.example.com", NextState::Status);
    sent.extend(frame(&encode_status_request()));
    client.write_all(&sent).await.unwrap();
    let status =
        StatusResponse::from_json(&decode_status_response(&read_frame(&mut client).await).unwrap())
            .unwrap();
    let description = status.description.unwrap().to_string();
    assert!(
        description.contains(SLEEPING_SUFFIX.trim()),
        "{description}"
    );
    assert_eq!(status.players.unwrap().online, 0);
    assert!(
        h.hooks.wakes.lock().unwrap().is_empty(),
        "never wake on a ping"
    );
}

#[tokio::test]
async fn login_to_a_sleeping_server_wakes_it_and_disconnects() {
    let (addr, mut rx) = fake_backend().await;
    let h = start(
        vec![route("smp.example.com", Some("smp"))],
        vec![backend("smp", addr, ServerState::Sleeping)],
        Limits::default(),
        false,
    )
    .await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("smp.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    let reason = disconnect_reason(&read_frame(&mut client).await);
    assert!(reason.contains("waking up"), "{reason}");
    assert_eq!(*h.hooks.wakes.lock().unwrap(), ["smp"]);
    assert!(rx.try_recv().is_err(), "nothing forwarded");
}

#[tokio::test]
async fn refused_logins_never_reach_or_wake_the_server() {
    for state in [ServerState::Running, ServerState::Sleeping] {
        let (addr, mut rx) = fake_backend().await;
        let h = start(
            vec![route("smp.example.com", Some("smp"))],
            vec![backend("smp", addr, state)],
            Limits::default(),
            false,
        )
        .await;
        h.hooks.refuse.lock().unwrap().push("Griefer".into());
        let mut client = connect(h.addr).await;
        let mut sent = handshake("smp.example.com", NextState::Login);
        sent.extend(login_start("Griefer"));
        client.write_all(&sent).await.unwrap();
        let reason = disconnect_reason(&read_frame(&mut client).await);
        assert!(reason.contains("banned"), "{reason}");
        assert!(
            h.hooks.wakes.lock().unwrap().is_empty(),
            "{state:?}: not woken"
        );
        assert!(
            h.hooks.joins.lock().unwrap().is_empty(),
            "{state:?}: no session"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(rx.try_recv().is_err(), "{state:?}: nothing forwarded");
    }
}

#[tokio::test]
async fn offline_and_starting_servers_explain_themselves() {
    let (addr, _rx) = fake_backend().await;
    for (state, expected) in [
        (ServerState::Stopped, "is offline"),
        (ServerState::Starting, "is starting"),
    ] {
        let h = start(
            vec![route("smp.example.com", Some("smp"))],
            vec![backend("smp", addr, state)],
            Limits::default(),
            false,
        )
        .await;
        let mut client = connect(h.addr).await;
        let mut sent = handshake("smp.example.com", NextState::Login);
        sent.extend(login_start("Steve"));
        client.write_all(&sent).await.unwrap();
        let reason = disconnect_reason(&read_frame(&mut client).await);
        assert!(reason.contains(expected), "{state:?}: {reason}");
        assert!(h.hooks.wakes.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn unreachable_backend_gets_a_message() {
    // Bind then drop to get a port nothing listens on.
    let dead = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let h = start(
        vec![route("*", Some("x"))],
        vec![backend("x", dead, ServerState::Running)],
        Limits::default(),
        false,
    )
    .await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("x.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    assert!(disconnect_reason(&read_frame(&mut client).await).contains("Couldn't reach"));
}

#[tokio::test]
async fn status_is_proxied_to_running_servers_and_cached() {
    let addr = fake_status_server("Hello from Paper").await;
    let h = start(
        vec![route("*", Some("paper"))],
        vec![backend("paper", addr, ServerState::Running)],
        Limits::default(),
        false,
    )
    .await;

    let mut client = connect(h.addr).await;
    let mut sent = handshake("play.example.com", NextState::Status);
    sent.extend(frame(&encode_status_request()));
    sent.extend(frame(&encode_ping(7)));
    client.write_all(&sent).await.unwrap();
    let frames = read_frames(&mut client, 2).await;
    let status = StatusResponse::from_json(&decode_status_response(&frames[0]).unwrap()).unwrap();
    assert_eq!(status.version.name, "Paper 1.21.4");
    assert_eq!(decode_ping(&frames[1]).unwrap(), 7);

    h.proxy.refresh_status_now().await;
    let cached = h.proxy.cached_status("paper").expect("status cached");
    assert_eq!(cached.response.players.unwrap().online, 3);
}

#[tokio::test]
async fn proxy_protocol_header_is_sent_when_enabled() {
    let (addr, mut rx) = fake_backend().await;
    let h = start(
        vec![route("*", Some("a"))],
        vec![backend("a", addr, ServerState::Running)],
        Limits::default(),
        true,
    )
    .await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("a.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    read_exact(&mut client, BACKEND_HELLO.len()).await;
    let received = rx.recv().await.unwrap();
    assert_eq!(&received[..12], b"\r\n\r\n\0\r\nQUIT\n");
    assert_eq!(received[13], 0x11, "TCP over IPv4");
    let client_port = client.local_addr().unwrap().port();
    assert_eq!(
        &received[24..26],
        &client_port.to_be_bytes(),
        "source port is the player's"
    );
    assert_eq!(&received[28..], &sent[..]);
}

#[tokio::test]
async fn player_sessions_are_tracked() {
    let (addr, _rx) = fake_backend().await;
    let h = start(
        vec![route("*", Some("a"))],
        vec![backend("a", addr, ServerState::Running)],
        Limits::default(),
        false,
    )
    .await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("a.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    read_exact(&mut client, BACKEND_HELLO.len()).await;

    {
        let joins = h.hooks.joins.lock().unwrap();
        assert_eq!(joins.len(), 1);
        assert_eq!(joins[0].username, "Steve");
        assert_eq!(
            joins[0].uuid.as_deref(),
            Some(UUID.hyphenated().to_string().as_str())
        );
    }
    let live = h.proxy.active_connections();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].username.as_deref(), Some("Steve"));

    drop(client);
    for _ in 0..50 {
        if !h.hooks.lefts.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(*h.hooks.lefts.lock().unwrap(), ["session-1"]);
    assert!(h.proxy.active_connections().is_empty());
}

#[tokio::test]
async fn legacy_pings_get_a_kick_response() {
    let h = start(vec![], vec![], Limits::default(), false).await;
    let mut client = connect(h.addr).await;
    client.write_all(&[0xfe, 0x01]).await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply[0], 0xff);
    let expected = legacy::kick_response(
        &nn_mcproto::LegacyPing::V1_4,
        "NetherNode",
        "Unknown server",
        0,
        0,
    );
    assert_eq!(reply, expected);
}

#[tokio::test]
async fn per_ip_rate_limit_drops_excess_connections() {
    let limits = Limits {
        per_ip_burst: 2,
        per_ip_per_second: 0.01,
        ..Limits::default()
    };
    let h = start(vec![], vec![], limits, false).await;
    let _a = connect(h.addr).await;
    let _b = connect(h.addr).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut c = connect(h.addr).await;
    assert_closed_silently(&mut c, Duration::from_millis(500)).await;
}

#[tokio::test]
async fn route_changes_do_not_drop_live_connections() {
    let (addr, _rx) = fake_backend().await;
    let routes = vec![route("*", Some("a"))];
    let backends = vec![backend("a", addr, ServerState::Running)];
    let h = start(routes, backends.clone(), Limits::default(), false).await;
    let mut client = connect(h.addr).await;
    let mut sent = handshake("a.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    read_exact(&mut client, BACKEND_HELLO.len()).await;

    // Remove every route but keep the listener.
    let listener = h.proxy.listener_statuses().await[0].spec.clone();
    let statuses = h
        .proxy
        .apply(ProxyConfig {
            listeners: vec![listener],
            routes: vec![],
            backends,
        })
        .await;
    assert!(
        matches!(statuses[0].state, ListenerState::Listening(a) if a == h.addr),
        "not rebound"
    );

    client.write_all(b"still here").await.unwrap();
    assert_eq!(read_exact(&mut client, 10).await, b"still here");

    // New connections see the new table.
    let mut fresh = connect(h.addr).await;
    fresh.write_all(&sent).await.unwrap();
    assert_eq!(
        disconnect_reason(&read_frame(&mut fresh).await),
        UNKNOWN_HOST_MESSAGE
    );
}

#[tokio::test]
async fn listeners_report_bind_failures_independently() {
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let taken_addr = taken.local_addr().unwrap();
    let proxy = Proxy::new(
        Arc::new(RecordingHooks::default()),
        EventBus::default(),
        Limits::default(),
    );
    let listeners = vec![
        ListenerSpec {
            id: "ok".into(),
            bind: "127.0.0.1:0".parse().unwrap(),
            proxy_protocol: false,
        },
        ListenerSpec {
            id: "clash".into(),
            bind: taken_addr,
            proxy_protocol: false,
        },
    ];
    let statuses = proxy
        .apply(ProxyConfig {
            listeners,
            ..ProxyConfig::default()
        })
        .await;
    let by_id = |id: &str| {
        statuses
            .iter()
            .find(|s| s.spec.id == id)
            .unwrap()
            .state
            .clone()
    };
    assert!(matches!(by_id("ok"), ListenerState::Listening(_)));
    assert!(matches!(by_id("clash"), ListenerState::Failed(_)));
}

#[tokio::test]
async fn joined_players_are_listed_before_the_event_fires() {
    let (addr, _rx) = fake_backend().await;
    let h = start(
        vec![route("*", Some("a"))],
        vec![backend("a", addr, ServerState::Running)],
        Limits::default(),
        false,
    )
    .await;
    let mut events = h.events.subscribe();
    let mut client = connect(h.addr).await;
    let mut sent = handshake("a.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(event, nn_events::Event::PlayerJoined { .. }) {
            // A UI refetching on this event must see the player.
            assert_eq!(h.proxy.active_connections().len(), 1);
            break;
        }
    }
}

/// Hooks that reach "other machines" by dialing a local fake backend (or
/// report the machine offline).
struct TunnelHooks {
    far: Option<SocketAddr>,
    dials: Mutex<Vec<String>>,
}

impl ProxyHooks for TunnelHooks {
    fn dial<'a>(
        &'a self,
        server_id: &'a str,
    ) -> BoxFuture<'a, std::io::Result<Box<dyn nn_proxy::Tunnel>>> {
        self.dials.lock().unwrap().push(server_id.to_owned());
        Box::pin(async move {
            match self.far {
                Some(addr) => {
                    Ok(Box::new(TcpStream::connect(addr).await?) as Box<dyn nn_proxy::Tunnel>)
                }
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "offline",
                )),
            }
        })
    }

    fn wake<'a>(&'a self, _server_id: &'a str) -> BoxFuture<'a, WakeOutcome> {
        Box::pin(async { WakeOutcome::Starting })
    }

    fn player_joined<'a>(&'a self, _join: &'a PlayerJoin) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }

    fn player_left(&self, _session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

async fn start_tunneled(far: Option<SocketAddr>) -> (Proxy, SocketAddr, Arc<TunnelHooks>) {
    let hooks = Arc::new(TunnelHooks {
        far,
        dials: Mutex::default(),
    });
    let proxy = Proxy::new(hooks.clone(), EventBus::default(), Limits::default());
    let mut far_server = backend(
        "far",
        "0.0.0.0:30000".parse().unwrap(),
        ServerState::Running,
    );
    far_server.remote = true;
    let statuses = proxy
        .apply(ProxyConfig {
            listeners: vec![ListenerSpec {
                id: "main".into(),
                bind: "127.0.0.1:0".parse().unwrap(),
                proxy_protocol: false,
            }],
            routes: vec![route("far.example.com", Some("far"))],
            backends: vec![far_server],
        })
        .await;
    let ListenerState::Listening(addr) = statuses[0].state.clone() else {
        panic!("listener failed: {statuses:?}")
    };
    (proxy, addr, hooks)
}

#[tokio::test]
async fn servers_on_other_machines_are_reached_through_the_dial_hook() {
    let (far_addr, mut far_rx) = fake_backend().await;
    let (_proxy, addr, hooks) = start_tunneled(Some(far_addr)).await;

    let mut client = connect(addr).await;
    let mut sent = handshake("far.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    assert_eq!(
        read_exact(&mut client, BACKEND_HELLO.len()).await,
        BACKEND_HELLO
    );
    assert_eq!(far_rx.recv().await.unwrap(), sent, "the exact bytes cross");
    client.write_all(b"ping").await.unwrap();
    assert_eq!(read_exact(&mut client, 4).await, b"ping", "both ways");
    assert_eq!(*hooks.dials.lock().unwrap(), vec!["far".to_owned()]);
}

#[tokio::test]
async fn a_server_whose_machine_is_offline_says_so() {
    let (_proxy, addr, _hooks) = start_tunneled(None).await;
    let mut client = connect(addr).await;
    let mut sent = handshake("far.example.com", NextState::Login);
    sent.extend(login_start("Steve"));
    client.write_all(&sent).await.unwrap();
    let reason = disconnect_reason(&read_frame(&mut client).await);
    assert!(reason.contains("offline"), "{reason}");
}
