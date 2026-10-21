//! ModuleService without Docker: the templates, listing, and what's refused
//! when modules can't run (no Docker, or a server on another machine).

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig};
use nn_events::EventBus;
use nn_node::{LocalNode, Nodes};
use nn_store::{NewModule, NewServer, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";

struct Harness {
    app: Router,
    store: Store,
    owner: String,
    servers_dir: PathBuf,
    _tmp: tempfile::TempDir,
}

async fn harness() -> Harness {
    harness_with(Default::default()).await
}

async fn harness_with(content: Arc<nn_api::content::Content>) -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let servers_dir = tmp.path().canonicalize().unwrap().join("servers");
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        StoreProxyHooks::new(store.clone()),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    let auth = Arc::new(Auth::new(store.clone(), AuthConfig::default()));
    let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let app = nn_api::router(AppState {
        proxy: Arc::new(ProxyRuntime::new(proxy, store.clone(), true)),
        auth: auth.clone(),
        events: events.clone(),
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        content,
        notifier: Default::default(),
        install_count: Default::default(),
        agents: Default::default(),
        nodes: Nodes::new(
            LocalNode::without_docker("not connected", Some(servers_dir.clone()), events.clone()),
            Some(store.clone()),
        ),
    });
    let owner = auth
        .setup("owner", "correct horse battery", Default::default())
        .await
        .unwrap()
        .token;
    Harness {
        app,
        store,
        owner,
        servers_dir,
        _tmp: tmp,
    }
}

impl Harness {
    async fn server(&self, slug: &str, loader: &str) -> String {
        let id = self
            .store
            .insert_server(NewServer {
                name: slug,
                slug,
                status: "stopped",
                mc_version: "1.20.1",
                loader,
                image: "itzg/minecraft-server",
                memory_mb: 2048,
                internal_port: 30_500 + slug.len() as i64,
                sleep_when_empty: false,
            })
            .await
            .unwrap()
            .id;
        std::fs::create_dir_all(self.servers_dir.join(slug).join("mods")).unwrap();
        id
    }

    async fn call(&self, method: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::post(format!("/api/rpc/nethernode.v1.ModuleService/{method}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!("nn_csrf={CSRF}; nn_session={}", self.owner),
            )
            .header("x-csrf-token", CSRF)
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn templates_are_listed_with_their_settings() {
    let h = harness().await;
    let (status, body) = h.call("ListModuleTemplates", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bluemap = body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "bluemap")
        .expect("BlueMap is built in");
    assert_eq!(bluemap["hasWeb"], true);
    assert!(bluemap["image"].as_str().unwrap().contains("@sha256:"));
    let accept = bluemap["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "accept_download")
        .unwrap();
    assert_eq!(accept["kind"], "boolean");
    assert_eq!(accept["required"], true);
}

#[tokio::test]
async fn without_docker_modules_are_listed_but_not_added() {
    let h = harness().await;
    let id = h.server("smp", "vanilla").await;
    // One recorded before Docker went away.
    h.store
        .insert_module(NewModule {
            server_id: &id,
            template: "bluemap",
            name: "bluemap",
            settings_json: r#"{"accept_download":true,"render_threads":1}"#,
            lifecycle: "always",
            memory_mb: 2048,
            web_port: Some(8100),
            web_host: Some("map"),
        })
        .await
        .unwrap();
    let (status, body) = h.call("ListModules", json!({ "serverId": id })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["unavailable"].as_str().unwrap().contains("Docker"),
        "{body}"
    );
    assert_eq!(body["modules"][0]["name"], "bluemap");
    assert_eq!(body["modules"][0]["status"], "absent");

    let (_, body) = h
        .call(
            "AddModule",
            json!({ "serverId": id, "template": "bluemap", "name": "map2", "settingsJson": r#"{"accept_download":true}"# }),
        )
        .await;
    assert_eq!(body["code"], "failed_precondition", "{body}");
    assert_eq!(h.store.list_modules(Some(&id)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_modules_own_address_reaches_its_page() {
    let h = harness().await;
    let id = h.server("mc1", "vanilla").await;
    h.store
        .upsert_route(
            None,
            nn_store::RouteInput {
                server_id: Some(&id),
                hostname: "mc1.example.com",
                listener_id: None,
                priority: 0,
            },
        )
        .await
        .unwrap();
    // A stand-in for the module's web page.
    let page = Router::new().fallback(|uri: http::Uri| async move { format!("map page at {uri}") });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, page).await.unwrap() });
    let module = h
        .store
        .insert_module(NewModule {
            server_id: &id,
            template: "bluemap",
            name: "bluemap",
            settings_json: "{}",
            lifecycle: "manual",
            memory_mb: 2048,
            web_port: Some(i64::from(port)),
            web_host: Some("map"),
        })
        .await
        .unwrap();

    let get = |host: &'static str| {
        let app = h.app.clone();
        async move {
            let req = Request::get("/tiles/0.json?x=1")
                .header(header::HOST, host)
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(req).await.unwrap();
            let status = response.status();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).into_owned())
        }
    };
    // Off until it's opened.
    assert!(!get("map.mc1.example.com").await.1.contains("map page"));

    let (status, body) = h
        .call(
            "SetModuleSite",
            json!({ "id": module.id, "webHost": "map", "webPublic": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["module"]["siteAddresses"],
        json!(["map.mc1.example.com"])
    );
    let (status, page) = get("map.mc1.example.com:443").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page, "map page at /tiles/0.json?x=1");
    // Other hostnames still get the panel.
    assert!(!get("panel.example.com").await.1.contains("map page"));

    let (_, body) = h
        .call(
            "SetModuleSite",
            json!({ "id": module.id, "webHost": "bad host", "webPublic": true }),
        )
        .await;
    assert_eq!(body["code"], "invalid_argument", "{body}");
    h.call(
        "SetModuleSite",
        json!({ "id": module.id, "webHost": "map", "webPublic": false }),
    )
    .await;
    assert!(!get("map.mc1.example.com").await.1.contains("map page"));
}
