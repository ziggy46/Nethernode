//! Nodes over HTTP, with a real agent joining a real panel over a socket.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::agents::Agents;
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, SecretBox};
use nn_events::EventBus;
use nn_node::Nodes;
use nn_node::agent::{AgentArgs, AgentError};
use nn_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";

/// The panel, also listening on a real port for agents.
async fn app() -> (Router, String, String) {
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        StoreProxyHooks::new(store.clone()),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    let key = Arc::new(SecretBox::from_hex_key(&"ab".repeat(32)).unwrap());
    let auth =
        Arc::new(Auth::new(store.clone(), AuthConfig::default()).with_secrets(Some(key.clone())));
    let agents = Arc::new(Agents::load(store.clone(), events.clone(), Some(&key)).await);
    let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let app = nn_api::router(AppState {
        proxy: Arc::new(ProxyRuntime::new(proxy, store.clone(), true)),
        auth: auth.clone(),
        events,
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        content: Default::default(),
        notifier: Default::default(),
        install_count: Default::default(),
        agents,
        nodes: Default::default(),
    });
    let owner = auth
        .setup("owner", PASSWORD, Default::default())
        .await
        .unwrap()
        .token;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let served = app.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            served.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (app, owner, url)
}

/// One browser: a session cookie (once signed in) and the CSRF cookie.
struct Client {
    app: Router,
    session: Option<String>,
}

impl Client {
    fn new(app: &Router, session: Option<String>) -> Self {
        Self {
            app: app.clone(),
            session,
        }
    }

    async fn call(&mut self, method: &str, body: Value) -> (StatusCode, Value) {
        let (service, method) = method.split_once('/').unwrap();
        let mut cookie = format!("nn_csrf={CSRF}");
        if let Some(s) = &self.session {
            cookie.push_str(&format!("; nn_session={s}"));
        }
        let req = Request::post(format!("/api/rpc/nethernode.v1.{service}/{method}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", CSRF)
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        for value in response.headers().get_all(header::SET_COOKIE) {
            let value = value.to_str().unwrap();
            if let Some(rest) = value.strip_prefix("nn_session=") {
                let token = rest.split(';').next().unwrap();
                self.session = (!token.is_empty()).then(|| token.to_owned());
            }
        }
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn ok(&mut self, method: &str, body: Value) -> Value {
        let (status, body) = self.call(method, body).await;
        assert_eq!(status, StatusCode::OK, "{method}: {body}");
        body
    }
}

fn agent(dir: &std::path::Path, url: Option<&str>, code: Option<&str>) -> AgentArgs {
    AgentArgs {
        dir: dir.to_owned(),
        panel_url: url.map(str::to_owned),
        code: code.map(str::to_owned),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The node's entry, once `check` holds for it (or panics after 10 s).
async fn wait_for(admin: &mut Client, id: &str, check: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        let list = admin.ok("NodeService/ListNodes", json!({})).await;
        let node = list["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == id)
            .cloned()
            .unwrap_or(Value::Null);
        if check(&node) {
            return node;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("node {id} never got there");
}

#[tokio::test]
async fn an_agent_joins_reconnects_and_is_removed() {
    let (app, owner, url) = app().await;
    let mut admin = Client::new(&app, Some(owner));

    let list = admin.ok("NodeService/ListNodes", json!({})).await;
    assert_eq!(list["nodes"][0]["local"], true);
    assert_eq!(list["nodes"][0]["status"], "NODE_STATUS_ONLINE");
    let (status, _) = admin
        .call("NodeService/DeleteNode", json!({ "id": "local" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let created = admin
        .ok(
            "NodeService/CreateNode",
            json!({ "name": "  Basement PC " }),
        )
        .await;
    let id = created["node"]["id"].as_str().unwrap().to_owned();
    let token = created["join"]["token"].as_str().unwrap().to_owned();
    assert_eq!(created["node"]["name"], "Basement PC");
    assert_eq!(created["node"]["status"], "NODE_STATUS_WAITING");
    assert!(
        created["join"]["image"]
            .as_str()
            .unwrap()
            .starts_with("ghcr.io/")
    );
    let (status, _) = admin
        .call("NodeService/CreateNode", json!({ "name": "basement pc" }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let waiting = wait_for(&mut admin, &id, |n| n["joinPending"] == true).await;
    assert_eq!(waiting["status"], "NODE_STATUS_WAITING");

    // A code for another panel is refused before anything is sent.
    let elsewhere = tempfile::tempdir().unwrap();
    let foreign = nn_link::JoinToken::generate([9; 32]).encode();
    let result = nn_node::agent::run(
        agent(elsewhere.path(), Some(&url), Some(&foreign)),
        Nodes::default().local().clone(),
    )
    .await;
    assert!(matches!(result, Err(AgentError::Join(_))), "{result:?}");

    // The real code joins, and the agent remembers the panel.
    let dir = tempfile::tempdir().unwrap();
    let running = tokio::spawn(nn_node::agent::run(
        agent(dir.path(), Some(&url), Some(&token)),
        Nodes::default().local().clone(),
    ));
    let online = wait_for(&mut admin, &id, |n| {
        n["status"] == "NODE_STATUS_ONLINE" && n.get("host").is_some()
    })
    .await;
    assert_eq!(online["version"], env!("CARGO_PKG_VERSION"));
    // That machine's agent has no Docker in this test, and says so.
    assert!(online["host"]["dockerError"].as_str().is_some());
    assert!(dir.path().join("agent.json").exists());
    let (status, _) = admin
        .call("NodeService/RegenerateJoinCode", json!({ "id": id }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "joined nodes need no code");

    // The code is spent: a second machine can't use it.
    let thief = tempfile::tempdir().unwrap();
    let result = nn_node::agent::run(
        agent(thief.path(), Some(&url), Some(&token)),
        Nodes::default().local().clone(),
    )
    .await;
    assert!(matches!(result, Err(AgentError::Join(_))), "{result:?}");

    // Gone, then back without a code.
    running.abort();
    wait_for(&mut admin, &id, |n| n["status"] == "NODE_STATUS_OFFLINE").await;
    // Docker's restart policy replays the join flags: the saved state
    // wins, and the spent code isn't tried again.
    let again = tokio::spawn(nn_node::agent::run(
        agent(dir.path(), Some(&url), Some(&token)),
        Nodes::default().local().clone(),
    ));
    wait_for(&mut admin, &id, |n| n["status"] == "NODE_STATUS_ONLINE").await;

    // Pointed at another panel, a joined agent refuses rather than forgets.
    let result = nn_node::agent::run(
        agent(dir.path(), Some("http://other.example"), None),
        Nodes::default().local().clone(),
    )
    .await;
    assert!(matches!(result, Err(AgentError::Usage(_))), "{result:?}");

    let renamed = admin
        .ok(
            "NodeService/RenameNode",
            json!({ "id": id, "name": "Attic" }),
        )
        .await;
    assert_eq!(renamed["node"]["name"], "Attic");

    // Removed: disconnected, and its key no longer gets in.
    admin
        .ok("NodeService/DeleteNode", json!({ "id": id }))
        .await;
    let list = admin.ok("NodeService/ListNodes", json!({})).await;
    assert_eq!(list["nodes"].as_array().unwrap().len(), 1);
    let state = nn_node::agent::AgentState::load(dir.path())
        .unwrap()
        .unwrap();
    let hello = nn_link::messages::Hello {
        protocol: nn_link::PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        join: None,
    };
    let result = nn_node::agent::connect(&url, &state.key, &state.panel_key, &hello).await;
    assert!(
        matches!(result, Err(nn_link::LinkError::Rejected(_))),
        "{:?}",
        result.err()
    );
    again.abort();
}
