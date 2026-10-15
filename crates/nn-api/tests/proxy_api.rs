//! ProxyService and /api/events, end to end: RPCs configure a real
//! listener, and a raw Minecraft client connects through it.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, password};
use nn_events::{Event, EventBus};
use nn_mcproto::codec::{Reader, frame};
use nn_mcproto::status::{decode_status_response, encode_status_request};
use nn_mcproto::{Handshake, LoginStart, NextState, StatusResponse};
use nn_store::{NewServer, NewUser, Store};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tower::ServiceExt;

const PROXY: &str = "/api/rpc/nethernode.v1.ProxyService";
const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";

struct Harness {
    app: Router,
    store: Store,
    events: EventBus,
    owner: String,
    shutdown: tokio::sync::watch::Sender<bool>,
}

async fn harness() -> Harness {
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        StoreProxyHooks::new(store.clone()),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    let auth = Arc::new(Auth::new(store.clone(), AuthConfig::default()));
    let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let app = nn_api::router(AppState {
        proxy: Arc::new(ProxyRuntime::new(proxy, store.clone(), true)),
        auth: auth.clone(),
        events: events.clone(),
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        content: Default::default(),
        notifier: Default::default(),
        install_count: Default::default(),
        agents: Default::default(),
        nodes: Default::default(),
    });
    let owner = auth
        .setup("owner", PASSWORD, Default::default())
        .await
        .unwrap()
        .token;
    Harness {
        app,
        store,
        events,
        owner,
        shutdown,
    }
}

async fn call(app: &Router, method: &str, body: Value, session: &str) -> (StatusCode, Value) {
    let req = Request::post(format!("{PROXY}/{method}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("nn_csrf={CSRF}; nn_session={session}"),
        )
        .header("x-csrf-token", CSRF)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn free_port() -> u16 {
    tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn server(store: &Store, slug: &str, status: &str) -> String {
    let port = i64::from(free_port().await);
    let new = NewServer {
        name: "Nether SMP",
        slug,
        status,
        mc_version: "1.21.4",
        loader: "paper",
        image: "itzg/minecraft-server",
        memory_mb: 2048,
        internal_port: port,
        sleep_when_empty: true,
    };
    store.insert_server(new).await.unwrap().id
}

fn handshake(host: &str, next: NextState) -> Vec<u8> {
    frame(
        &Handshake {
            protocol_version: 777,
            server_address: host.into(),
            server_port: 25565,
            next_state: next,
        }
        .encode(),
    )
}

async fn read_frame_payload(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = nn_proxy::io::FrameBuffer::new();
    let range = tokio::time::timeout(Duration::from_secs(3), buf.next_frame(stream, 1 << 20))
        .await
        .unwrap()
        .unwrap();
    buf.payload(range).to_vec()
}

#[tokio::test]
async fn routes_configured_over_rpc_take_effect_on_a_real_listener() {
    let h = harness().await;
    let server_id = server(&h.store, "smp", "sleeping").await;
    let port = free_port().await;

    let (status, body) = call(
        &h.app,
        "UpsertListener",
        json!({ "bindAddr": "127.0.0.1", "port": port, "enabled": true }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["listener"]["state"], "LISTENER_STATE_LISTENING");

    let (status, body) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "SMP.Example.com.", "serverId": server_id }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["route"]["hostname"], "smp.example.com",
        "stored normalized"
    );
    assert_eq!(body["route"]["kind"], "ROUTE_KIND_EXACT");
    assert_eq!(body["route"]["serverName"], "Nether SMP");

    // A server list ping shows the sleeping server without waking it.
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut sent = handshake("smp.example.com", NextState::Status);
    sent.extend(frame(&encode_status_request()));
    client.write_all(&sent).await.unwrap();
    let json = decode_status_response(&read_frame_payload(&mut client).await).unwrap();
    let status = StatusResponse::from_json(&json).unwrap();
    assert!(status.description.unwrap().to_string().contains("Sleeping"));

    // Joining explains that it can't be woken yet (no orchestrator).
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut sent = handshake("smp.example.com", NextState::Login);
    sent.extend(frame(
        &LoginStart {
            username: "Steve".into(),
            uuid: None,
        }
        .encode(777),
    ));
    client.write_all(&sent).await.unwrap();
    let payload = read_frame_payload(&mut client).await;
    let mut r = Reader::new(&payload);
    assert_eq!(r.varint().unwrap(), 0);
    let reason: Value = serde_json::from_str(r.string(32_767).unwrap()).unwrap();
    assert!(
        reason["text"].as_str().unwrap().contains("asleep"),
        "{reason}"
    );

    // Deleting the route takes effect immediately.
    let (_, routes) = call(&h.app, "ListRoutes", json!({}), &h.owner).await;
    let route_id = routes["routes"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        call(&h.app, "DeleteRoute", json!({ "id": route_id }), &h.owner)
            .await
            .0,
        StatusCode::OK
    );
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    client.write_all(&sent).await.unwrap();
    let payload = read_frame_payload(&mut client).await;
    assert!(String::from_utf8_lossy(&payload).contains("Unknown server address"));

    let audit: Vec<String> = h
        .store
        .recent_audit(10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    assert!(audit.contains(&"proxy.route.upsert".to_owned()));
    assert!(audit.contains(&"proxy.route.delete".to_owned()));
    assert!(audit.contains(&"proxy.listener.upsert".to_owned()));
}

#[tokio::test]
async fn route_validation() {
    let h = harness().await;
    let server_id = server(&h.store, "smp", "running").await;

    let (status, body) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "bad host", "serverId": server_id }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_argument");

    let (status, body) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "play.example.com" }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"].as_str().unwrap().contains("destination"));

    let (status, _) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "x.com", "serverId": "missing" }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Slug wildcards don't need a server.
    let (status, body) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "*.mc.example.com" }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["route"]["kind"], "ROUTE_KIND_WILDCARD");

    let (status, body) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "*.MC.example.com" }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "already_exists");

    let (status, _) = call(
        &h.app,
        "UpsertListener",
        json!({ "bindAddr": "nope", "port": 25565 }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &h.app,
        "UpsertListener",
        json!({ "bindAddr": "0.0.0.0", "port": 0 }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn viewers_can_look_but_not_change() {
    let h = harness().await;
    let hash = password::hash_password(PASSWORD).unwrap();
    h.store
        .create_user(NewUser {
            username: "viewer",
            password_hash: &hash,
            role: "viewer",
            must_change_password: false,
        })
        .await
        .unwrap();
    let auth = Auth::new(h.store.clone(), AuthConfig::default());
    let viewer = auth
        .login("viewer", PASSWORD, Default::default())
        .await
        .unwrap()
        .session()
        .unwrap()
        .token;

    assert_eq!(
        call(&h.app, "ListRoutes", json!({}), &viewer).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&h.app, "ListConnections", json!({}), &viewer).await.0,
        StatusCode::OK
    );
    let (status, body) = call(
        &h.app,
        "UpsertRoute",
        json!({ "hostname": "*.x.com" }),
        &viewer,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "permission_denied");
    let (status, _) = call(
        &h.app,
        "UpsertListener",
        json!({ "bindAddr": "0.0.0.0", "port": 25570 }),
        &viewer,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        call(&h.app, "ListDomains", json!({}), &viewer).await.0,
        StatusCode::OK
    );
    let (status, _) = call(
        &h.app,
        "SetDomains",
        json!({ "domains": ["example.com"] }),
        &viewer,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn domains_are_cleaned_up_and_checked() {
    let h = harness().await;
    let (_, empty) = call(&h.app, "ListDomains", json!({}), &h.owner).await;
    assert!(empty.get("domains").is_none());
    let (status, saved) = call(
        &h.app,
        "SetDomains",
        json!({ "domains": [" Example.COM. ", "*.mc.example.com", "example.com"] }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["domains"], json!(["example.com", "mc.example.com"]));
    let (_, listed) = call(&h.app, "ListDomains", json!({}), &h.owner).await;
    assert_eq!(listed["domains"], saved["domains"]);
    for bad in ["localhost", "10.0.0.5", "not a domain", "*", "-x.com"] {
        let (status, body) =
            call(&h.app, "SetDomains", json!({ "domains": [bad] }), &h.owner).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    let (status, _) = call(&h.app, "SetDomains", json!({ "domains": [] }), &h.owner).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn check_dns_resolves_names() {
    let h = harness().await;
    let (status, body) = call(
        &h.app,
        "CheckDns",
        json!({ "hostname": "localhost" }),
        &h.owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["addresses"].as_array().unwrap().is_empty(), "{body}");
    let (status, _) = call(&h.app, "CheckDns", json!({ "hostname": "*" }), &h.owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn event_stream_requires_a_session_and_delivers_events() {
    let h = harness().await;
    let anonymous = h
        .app
        .clone()
        .oneshot(Request::get("/api/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let req = Request::get("/api/events")
        .header(header::COOKIE, format!("nn_session={}", h.owner))
        .body(Body::empty())
        .unwrap();
    let response = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );

    let mut body = response.into_body();
    h.events.publish(Event::ProxyListenerChanged {
        listener_id: "abc".into(),
    });
    let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert!(text.contains("event: proxy_listener_changed"), "{text}");
    assert!(text.contains(r#""listener_id":"abc""#), "{text}");
}

#[tokio::test]
async fn event_stream_ends_when_the_server_shuts_down() {
    let h = harness().await;
    let req = Request::get("/api/events")
        .header(header::COOKIE, format!("nn_session={}", h.owner))
        .body(Body::empty())
        .unwrap();
    let mut body = h.app.clone().oneshot(req).await.unwrap().into_body();
    h.shutdown.send(true).unwrap();
    // Otherwise this would wait forever (until a keep-alive) and block graceful shutdown.
    let end = tokio::time::timeout(Duration::from_secs(2), body.frame())
        .await
        .expect("stream ended");
    assert!(end.is_none(), "stream should be finished, got {end:?}");
}

async fn call_servers(
    app: &Router,
    method: &str,
    body: Value,
    session: &str,
) -> (StatusCode, Value) {
    let req = Request::post(format!("/api/rpc/nethernode.v1.ServerService/{method}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("nn_csrf={CSRF}; nn_session={session}"),
        )
        .header("x-csrf-token", CSRF)
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn server_management_explains_when_docker_is_unavailable() {
    let h = harness().await;
    server(&h.store, "smp", "stopped").await;

    // Reads still work from the database.
    let (status, body) = call_servers(&h.app, "ListServers", json!({}), &h.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["servers"][0]["slug"], "smp");
    assert_eq!(body["servers"][0]["status"], "SERVER_STATUS_STOPPED");

    let (status, body) = call_servers(&h.app, "GetHostInfo", json!({}), &h.owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["dockerAvailable"],
        Value::Null,
        "false is the proto default and omitted"
    );

    // Lifecycle calls say why they can't run.
    let create = json!({ "name": "x", "loader": "vanilla", "mcVersion": "LATEST", "memoryMb": 1024, "eulaAccepted": true });
    let (status, body) = call_servers(&h.app, "CreateServer", create, &h.owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "failed_precondition");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Docker isn't available"),
        "{body}"
    );
}

#[tokio::test]
async fn server_hostnames_include_slug_wildcards() {
    let h = harness().await;
    let server_id = server(&h.store, "smp", "stopped").await;
    for route in [
        json!({ "hostname": "play.example.com", "serverId": server_id }),
        json!({ "hostname": "*.mc.example.com" }),
        json!({ "hostname": "*", "serverId": server_id }),
    ] {
        let (status, body) = call(&h.app, "UpsertRoute", route, &h.owner).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, body) = call_servers(&h.app, "ListServers", json!({}), &h.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["servers"][0]["hostnames"],
        json!(["play.example.com", "smp.mc.example.com"]),
        "exact routes first, then the slug under each wildcard; the default route isn't a hostname"
    );
}

#[tokio::test]
async fn viewers_cannot_manage_servers() {
    let h = harness().await;
    let hash = password::hash_password(PASSWORD).unwrap();
    h.store
        .create_user(NewUser {
            username: "viewer",
            password_hash: &hash,
            role: "viewer",
            must_change_password: false,
        })
        .await
        .unwrap();
    let viewer = Auth::new(h.store.clone(), AuthConfig::default())
        .login("viewer", PASSWORD, Default::default())
        .await
        .unwrap()
        .session()
        .unwrap()
        .token;
    let id = server(&h.store, "smp", "stopped").await;
    assert_eq!(
        call_servers(&h.app, "ListServers", json!({}), &viewer)
            .await
            .0,
        StatusCode::OK
    );
    for method in ["StartServer", "StopServer", "KillServer", "DeleteServer"] {
        let (status, _) = call_servers(&h.app, method, json!({ "id": id }), &viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
    }
}
