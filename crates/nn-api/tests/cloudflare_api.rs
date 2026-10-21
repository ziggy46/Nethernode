//! CloudflareService against a stand-in for Cloudflare's API: connecting
//! makes a tunnel; a preview changes nothing; turning DNS on adds the
//! module's address and a copy of the wildcard above it (so the server's own
//! name keeps answering); turning it off and disconnecting take back only
//! what NetherNode added.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, SecretBox};
use nn_events::EventBus;
use nn_node::{LocalNode, Nodes};
use nn_store::{NewModule, NewServer, RouteInput, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const TOKEN: &str = "cf-test-token";

#[derive(Default)]
struct Fake {
    records: Vec<Value>,
    next: u32,
    ingress: Vec<String>,
    tunnels: Vec<String>,
}

type Shared = Arc<Mutex<Fake>>;

fn ok(result: Value) -> Json<Value> {
    Json(
        json!({ "success": true, "errors": [], "result": result, "result_info": { "total_pages": 1 } }),
    )
}

fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"))
}

async fn fake_cloudflare(fake: Shared) -> String {
    let app = Router::new()
        .route(
            "/accounts",
            get(|headers: HeaderMap| async move {
                if !authorized(&headers) {
                    return Json(json!({ "success": false, "errors": [{ "code": 1000, "message": "Invalid API Token" }] }));
                }
                ok(json!([{ "id": "acc1", "name": "Pickles" }]))
            }),
        )
        .route("/zones", get(|| async { ok(json!([{ "id": "z1", "name": "xtra-pickles.com" }])) }))
        .route(
            "/accounts/acc1/cfd_tunnel",
            axum::routing::post(|State(f): State<Shared>| async move {
                f.lock().unwrap().tunnels.push("tun1".into());
                ok(json!({ "id": "tun1" }))
            }),
        )
        .route(
            "/accounts/acc1/cfd_tunnel/tun1/token",
            get(|| async { ok(json!("run-token")) }),
        )
        .route(
            "/accounts/acc1/cfd_tunnel/tun1/configurations",
            put(|State(f): State<Shared>, Json(body): Json<Value>| async move {
                f.lock().unwrap().ingress = body["config"]["ingress"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|r| r["hostname"].as_str().map(str::to_owned))
                    .collect();
                ok(json!({}))
            }),
        )
        .route(
            "/accounts/acc1/cfd_tunnel/tun1/connections",
            delete(|| async { ok(json!({})) }),
        )
        .route(
            "/accounts/acc1/cfd_tunnel/tun1",
            delete(|State(f): State<Shared>| async move {
                f.lock().unwrap().tunnels.clear();
                ok(json!({}))
            }),
        )
        .route(
            "/zones/z1/dns_records",
            get(|State(f): State<Shared>| async move { ok(Value::Array(f.lock().unwrap().records.clone())) })
                .post(|State(f): State<Shared>, Json(mut body): Json<Value>| async move {
                    let mut f = f.lock().unwrap();
                    f.next += 1;
                    body["id"] = json!(format!("r{}", f.next));
                    f.records.push(body.clone());
                    ok(body)
                }),
        )
        .route(
            "/zones/z1/dns_records/{id}",
            put(|State(f): State<Shared>, Path(id): Path<String>, Json(mut body): Json<Value>| async move {
                let mut f = f.lock().unwrap();
                body["id"] = json!(id);
                for r in f.records.iter_mut().filter(|r| r["id"] == id.as_str()) {
                    *r = body.clone();
                }
                ok(body)
            })
            .delete(|State(f): State<Shared>, Path(id): Path<String>| async move {
                f.lock().unwrap().records.retain(|r| r["id"] != id.as_str());
                ok(json!({ "id": id }))
            }),
        )
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

struct Harness {
    app: Router,
    store: Store,
    owner: String,
    fake: Shared,
    _tmp: tempfile::TempDir,
}

async fn harness() -> Harness {
    let fake = Shared::default();
    fake.lock().unwrap().records = vec![
        json!({ "id": "w", "type": "A", "name": "*.xtra-pickles.com", "content": "203.0.113.5", "proxied": false }),
        json!({ "id": "u", "type": "A", "name": "panel.xtra-pickles.com", "content": "203.0.113.5", "proxied": false }),
    ];
    let base = fake_cloudflare(fake.clone()).await;
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
    let secrets = Arc::new(SecretBox::from_hex_key(&"42".repeat(32)).unwrap());
    let content =
        nn_api::content::Content::new(nn_catalog::Catalog::default(), Some(secrets), None);
    let (_shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let app = nn_api::router(AppState {
        proxy: Arc::new(ProxyRuntime::new(proxy, store.clone(), true)),
        auth: auth.clone(),
        events: events.clone(),
        config: Arc::new(ApiConfig {
            cloudflare_api: base,
            panel_port: 8080,
            ..Default::default()
        }),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        content: Arc::new(content),
        notifier: Default::default(),
        install_count: Default::default(),
        agents: Default::default(),
        nodes: Nodes::new(
            LocalNode::without_docker("not connected", Some(servers_dir), events.clone()),
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
        fake,
        _tmp: tmp,
    }
}

impl Harness {
    async fn call(&self, method: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::post(format!("/api/rpc/nethernode.v1.CloudflareService/{method}"))
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

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .fake
            .lock()
            .unwrap()
            .records
            .iter()
            .map(|r| {
                format!(
                    "{} {} {}",
                    r["type"].as_str().unwrap(),
                    r["name"].as_str().unwrap(),
                    r["content"].as_str().unwrap()
                )
            })
            .collect();
        names.sort();
        names
    }
}

#[tokio::test]
async fn connect_preview_manage_and_disconnect() {
    let h = harness().await;
    let untouched = h.names();

    let (_, body) = h
        .call("ConnectCloudflare", json!({ "token": "wrong" }))
        .await;
    assert_eq!(body["code"], "failed_precondition", "{body}");
    let (status, body) = h.call("ConnectCloudflare", json!({ "token": TOKEN })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, info) = h.call("GetCloudflare", json!({})).await;
    assert_eq!(info["connected"], true);
    assert_eq!(info["accountName"], "Pickles");
    assert_eq!(info["zones"], json!(["xtra-pickles.com"]));
    assert_eq!(
        info["dns"],
        Value::Null,
        "off until turned on (false is omitted)"
    );
    assert_eq!(h.fake.lock().unwrap().tunnels, ["tun1"]);

    // A server whose address comes from the slug wildcard, with BlueMap
    // open at map.mc1.xtra-pickles.com.
    let server = h
        .store
        .insert_server(NewServer {
            name: "MC1",
            slug: "mc1",
            status: "stopped",
            mc_version: "1.21.4",
            loader: "vanilla",
            image: "itzg/minecraft-server",
            memory_mb: 2048,
            internal_port: 30_100,
            sleep_when_empty: false,
        })
        .await
        .unwrap();
    h.store
        .upsert_route(
            None,
            RouteInput {
                server_id: None,
                hostname: "*.xtra-pickles.com",
                listener_id: None,
                priority: 0,
            },
        )
        .await
        .unwrap();
    let module = h
        .store
        .insert_module(NewModule {
            server_id: &server.id,
            template: "bluemap",
            name: "bluemap",
            settings_json: "{}",
            lifecycle: "manual",
            memory_mb: 2048,
            web_port: Some(8100),
            web_host: Some("map"),
        })
        .await
        .unwrap();
    h.store
        .set_module_site(&module.id, Some("map"), true)
        .await
        .unwrap();

    let (status, preview) = h.call("PreviewCloudflare", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let changes: Vec<&str> = preview["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert!(
        changes[0].starts_with("Add A mc1.xtra-pickles.com → 203.0.113.5"),
        "the copy comes first: {changes:?}"
    );
    assert!(changes[1].starts_with("Add CNAME map.mc1.xtra-pickles.com → tun1.cfargotunnel.com"));
    assert_eq!(h.names(), untouched, "a preview changes nothing");

    let (status, body) = h.call("SetCloudflareDns", json!({ "enabled": true })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names = h.names();
    assert!(
        names.contains(&"A mc1.xtra-pickles.com 203.0.113.5".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"CNAME map.mc1.xtra-pickles.com tun1.cfargotunnel.com".to_owned()),
        "{names:?}"
    );
    assert_eq!(h.fake.lock().unwrap().ingress, ["map.mc1.xtra-pickles.com"]);
    let (_, info) = h.call("GetCloudflare", json!({})).await;
    assert_eq!(info["routed"], json!(["map.mc1.xtra-pickles.com"]));

    let (status, body) = h
        .call("SetCloudflareDns", json!({ "enabled": false }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        h.names(),
        untouched,
        "only what NetherNode added is taken back"
    );
    assert!(h.fake.lock().unwrap().ingress.is_empty());

    let (status, body) = h.call("DisconnectCloudflare", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        h.fake.lock().unwrap().tunnels.is_empty(),
        "the tunnel is deleted"
    );
    let (_, info) = h.call("GetCloudflare", json!({})).await;
    assert_eq!(info["connected"], Value::Null);
    assert_eq!(h.names(), untouched);
}
