//! Bedrock (UDP) listeners, over real sockets with in-process fake servers.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nn_events::EventBus;
use nn_mcproto::bedrock::{self as raknet, MAGIC, Packet, Status};
use nn_proxy::{
    BackendSpec, BoxFuture, Limits, ListenerKind, ListenerSpec, ListenerState, PlayerJoin, Proxy,
    ProxyConfig, ProxyHooks, RouteSpec, SLEEPING_SUFFIX, ServerState, WakeOutcome,
};
use tokio::net::UdpSocket;

#[derive(Default)]
struct Hooks {
    wakes: Mutex<Vec<String>>,
}

impl ProxyHooks for Hooks {
    fn wake<'a>(&'a self, server_id: &'a str) -> BoxFuture<'a, WakeOutcome> {
        self.wakes.lock().unwrap().push(server_id.to_owned());
        Box::pin(async { WakeOutcome::Starting })
    }

    fn player_joined<'a>(&'a self, _join: &'a PlayerJoin) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }

    fn player_left(&self, _session_id: String) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// A Bedrock server that answers pings and echoes everything else.
async fn fake_server(status: &'static str) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok((n, from)) = socket.recv_from(&mut buf).await {
            let reply = match raknet::classify(&buf[..n]) {
                Packet::Ping { time } => raknet::encode_pong(time, 42, status),
                _ => [b"ECHO:".as_slice(), &buf[..n]].concat(),
            };
            let _ = socket.send_to(&reply, from).await;
        }
    });
    addr
}

fn join_packet() -> Vec<u8> {
    let mut packet = vec![raknet::ID_OPEN_CONNECTION_REQUEST_1];
    packet.extend_from_slice(&MAGIC);
    packet.push(11);
    packet.extend(std::iter::repeat_n(0, 100));
    packet
}

struct Harness {
    addr: SocketAddr,
    hooks: Arc<Hooks>,
    _proxy: Proxy,
}

async fn start(server: SocketAddr, state: ServerState) -> Harness {
    let hooks = Arc::new(Hooks::default());
    let proxy = Proxy::new(hooks.clone(), EventBus::default(), Limits::default());
    let statuses = proxy
        .apply(ProxyConfig {
            listeners: vec![ListenerSpec {
                id: "bedrock".into(),
                bind: "127.0.0.1:0".parse().unwrap(),
                proxy_protocol: false,
                kind: ListenerKind::Bedrock,
            }],
            routes: vec![RouteSpec {
                id: "r".into(),
                pattern: "*".into(),
                server_id: Some("bed".into()),
                listener_id: Some("bedrock".into()),
                priority: 0,
            }],
            backends: vec![BackendSpec {
                remote: false,
                server_id: "bed".into(),
                name: "Survival".into(),
                slug: "survival".into(),
                addr: server,
                state,
            }],
        })
        .await;
    let ListenerState::Listening(addr) = statuses[0].state.clone() else {
        panic!("listener failed: {statuses:?}")
    };
    Harness {
        addr,
        hooks,
        _proxy: proxy,
    }
}

async fn client() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.unwrap()
}

async fn receive(socket: &UdpSocket) -> Vec<u8> {
    let mut buf = [0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buf))
        .await
        .expect("no answer from the proxy")
        .unwrap();
    buf[..n].to_vec()
}

const REAL: &str = "MCPE;Real Server;860;1.21.50;3;10;12345;World;Survival;1;19132;19133;";

#[tokio::test]
async fn a_running_servers_ping_is_answered_by_the_server() {
    let server = fake_server(REAL).await;
    let proxy = start(server, ServerState::Running).await;
    let player = client().await;
    player.connect(proxy.addr).await.unwrap();
    player.send(&raknet::encode_ping(99, 7)).await.unwrap();
    let pong = raknet::decode_pong(&receive(&player).await).unwrap();
    assert_eq!(pong.time, 99, "the client's timestamp is echoed");
    let status = Status::parse(&pong.status).unwrap();
    assert_eq!(
        (status.motd.as_str(), status.online, status.max),
        ("Real Server", 3, 10)
    );
}

#[tokio::test]
async fn a_sleeping_server_answers_pings_itself_and_wakes_when_joined() {
    // Nothing listens at the server's address: it's asleep.
    let asleep = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let proxy = start(asleep.local_addr().unwrap(), ServerState::Sleeping).await;
    let player = client().await;
    player.connect(proxy.addr).await.unwrap();

    player.send(&raknet::encode_ping(1, 7)).await.unwrap();
    let pong = raknet::decode_pong(&receive(&player).await).unwrap();
    let status = Status::parse(&pong.status).unwrap();
    assert!(status.motd.starts_with("Survival"), "{}", status.motd);
    assert!(status.motd.ends_with(SLEEPING_SUFFIX), "{}", status.motd);
    assert_eq!(status.online, 0);
    assert!(
        proxy.hooks.wakes.lock().unwrap().is_empty(),
        "a ping never wakes a server"
    );

    // Trying to join wakes it, once however often they try.
    for _ in 0..3 {
        player.send(&join_packet()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(*proxy.hooks.wakes.lock().unwrap(), ["bed"]);
}

#[tokio::test]
async fn a_running_servers_sessions_are_relayed_both_ways() {
    let server = fake_server(REAL).await;
    let proxy = start(server, ServerState::Running).await;
    let player = client().await;
    player.connect(proxy.addr).await.unwrap();

    let join = join_packet();
    player.send(&join).await.unwrap();
    assert_eq!(
        receive(&player).await,
        [b"ECHO:".as_slice(), &join].concat()
    );
    // Later datagrams of the session aren't pings or joins; they still arrive.
    player.send(&[0x84, 1, 2, 3]).await.unwrap();
    assert_eq!(receive(&player).await, b"ECHO:\x84\x01\x02\x03");

    // Another player has a session of their own.
    let other = client().await;
    other.connect(proxy.addr).await.unwrap();
    other.send(&join).await.unwrap();
    assert_eq!(receive(&other).await, [b"ECHO:".as_slice(), &join].concat());
    assert!(proxy.hooks.wakes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn datagrams_that_are_not_a_ping_or_a_join_are_ignored_without_a_session() {
    let server = fake_server(REAL).await;
    let proxy = start(server, ServerState::Running).await;
    let stranger = client().await;
    stranger.connect(proxy.addr).await.unwrap();
    stranger.send(&[0x84, 1, 2, 3]).await.unwrap();
    let mut buf = [0u8; 64];
    assert!(
        tokio::time::timeout(Duration::from_millis(300), stranger.recv(&mut buf))
            .await
            .is_err(),
        "the proxy answered noise"
    );
}
