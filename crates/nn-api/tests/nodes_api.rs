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
async fn app() -> (Router, String, String, Store) {
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let kept = store.clone();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        StoreProxyHooks::new(store.clone()),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    let key = Arc::new(SecretBox::from_hex_key(&"ab".repeat(32)).unwrap());
    let auth =
        Arc::new(Auth::new(store.clone(), AuthConfig::default()).with_secrets(Some(key.clone())));
    let nodes = Nodes::new(
        nn_node::LocalNode::without_docker("not connected", None, events.clone()),
        Some(store.clone()),
    );
    let agents =
        Arc::new(Agents::load(store.clone(), events.clone(), Some(&key), nodes.clone()).await);
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
        nodes,
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
    (app, owner, url, kept)
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
        // Never the real thing in tests.
        updater: Some(Arc::new(|version| {
            Box::pin(async move { Err(format!("tests don't update (asked for {version})")) })
        })),
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
    let (app, owner, url, _) = app().await;
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

    // A server for that machine is its agent's to create: here it has no
    // Docker, and says so.
    let (status, body) = admin
        .call(
            "ServerService/CreateServer",
            json!({ "name": "Far away", "loader": "vanilla", "mcVersion": "1.21.4",
                    "memoryMb": 1024, "eulaAccepted": true, "nodeId": id }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Docker isn't available"),
        "{body}"
    );
    let (status, _) = admin
        .call(
            "ServerService/CreateServer",
            json!({ "name": "Nowhere", "loader": "vanilla", "mcVersion": "1.21.4",
                    "memoryMb": 1024, "eulaAccepted": true, "nodeId": "no-such-node" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
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

#[tokio::test]
async fn outdated_agents_moves_and_removing_a_machine_with_servers() {
    let (app, owner, url, store) = app().await;
    let mut admin = Client::new(&app, Some(owner));
    let created = admin
        .ok("NodeService/CreateNode", json!({ "name": "Garage" }))
        .await;
    let id = created["node"]["id"].as_str().unwrap().to_owned();
    let token = created["join"]["token"].as_str().unwrap().to_owned();

    // An agent on another version reports in, but needs updating.
    let dir = tempfile::tempdir().unwrap();
    let mut args = agent(dir.path(), Some(&url), Some(&token));
    args.version = "0.0.1".into();
    let old = tokio::spawn(nn_node::agent::run(args, Nodes::default().local().clone()));
    let node = wait_for(&mut admin, &id, |n| n["status"] == "NODE_STATUS_ONLINE").await;
    assert_eq!(node["updateNeeded"], true);
    assert_eq!(node["updateAvailable"], true);

    // Older agents are asked to update as they connect, to the panel's
    // version (this test agent answers without updating).
    let version = env!("CARGO_PKG_VERSION");
    let asked = format!("asked for {version}");
    let node = wait_for(&mut admin, &id, |n| {
        n["updating"] != true
            && n["updateError"]
                .as_str()
                .is_some_and(|e| e.contains(&asked))
    })
    .await;
    assert!(
        node["updateError"]
            .as_str()
            .unwrap()
            .contains("tests don't update")
    );
    let list = admin.ok("NodeService/ListNodes", json!({})).await;
    assert_eq!(list["autoUpdate"], true);
    assert_eq!(list["panelVersion"], version);
    // And by hand, with automatic updates off.
    admin
        .ok("NodeService/SetAutoUpdate", json!({ "enabled": false }))
        .await;
    let list = admin.ok("NodeService/ListNodes", json!({})).await;
    assert_eq!(list.get("autoUpdate"), None, "off is the default value");
    admin
        .ok("NodeService/UpdateNode", json!({ "id": id }))
        .await;
    wait_for(&mut admin, &id, |n| {
        n["updating"] != true
            && n["updateError"]
                .as_str()
                .is_some_and(|e| e.contains(&asked))
    })
    .await;
    let (status, _) = admin
        .call("NodeService/UpdateNode", json!({ "id": "local" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A server here, and one on that machine.
    let here = store
        .insert_server(nn_store::NewServer {
            name: "Here",
            slug: "here",
            status: "stopped",
            mc_version: "1.21.4",
            loader: "paper",
            image: "itzg/minecraft-server",
            memory_mb: 2048,
            internal_port: 30000,
            sleep_when_empty: false,
        })
        .await
        .unwrap();
    let there = nn_store::Server {
        id: "there-1".into(),
        name: "There".into(),
        slug: "there".into(),
        internal_port: 30010,
        ..here.clone()
    };
    store.upsert_server_mirror(&there, &id).await.unwrap();

    // Its server says why it can't be managed.
    let got = admin
        .ok("ServerService/GetServer", json!({ "id": "there-1" }))
        .await;
    assert_eq!(got["server"]["nodeOffline"], true);
    assert_eq!(got["server"]["nodeUpdateNeeded"], true);
    assert_eq!(got["server"]["nodeName"], "Garage");
    let (_, body) = admin
        .call("ServerService/StartServer", json!({ "id": "there-1" }))
        .await;
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("different NetherNode version"),
        "{body}"
    );

    // Moves: not to where it is, nowhere, or a machine that needs updating.
    let (status, body) = admin
        .call(
            "ServerService/MoveServer",
            json!({ "id": here.id, "nodeId": "local" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = admin
        .call(
            "ServerService/MoveServer",
            json!({ "id": here.id, "nodeId": "no-such-node" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = admin
        .call(
            "ServerService/MoveServer",
            json!({ "id": here.id, "nodeId": id }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Update it first"),
        "{body}"
    );

    // Removing a machine that has servers needs saying so; then the panel
    // forgets them, and only them.
    let (status, body) = admin
        .call("NodeService/DeleteNode", json!({ "id": id }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("still has servers"),
        "{body}"
    );
    admin
        .ok(
            "NodeService/DeleteNode",
            json!({ "id": id, "forgetServers": true }),
        )
        .await;
    let (status, _) = admin
        .call("ServerService/GetServer", json!({ "id": "there-1" }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin
        .ok("ServerService/GetServer", json!({ "id": here.id }))
        .await;
    old.abort();
}
