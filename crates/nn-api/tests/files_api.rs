//! FileService, server.properties, downloads, and the console
//! endpoint's access checks, against a real server folder on disk.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, ServerManager, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, password};
use nn_events::EventBus;
use nn_store::{NewServer, NewUser, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";

struct Harness {
    app: Router,
    store: Store,
    auth: Arc<Auth>,
    owner: String,
    root: PathBuf,
    server_id: String,
    _tmp: tempfile::TempDir,
}

async fn harness() -> Harness {
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
        events,
        config: Arc::new(ApiConfig::default()),
        metrics: None,
        shutdown: shutdown_rx,
        players: Default::default(),
        uploads: Default::default(),
        servers: ServerManager {
            servers_dir: Some(servers_dir.clone()),
            ..Default::default()
        },
    });
    let owner = auth
        .setup("owner", PASSWORD, Default::default())
        .await
        .unwrap()
        .token;
    let server_id = store
        .insert_server(NewServer {
            name: "SMP",
            slug: "smp",
            status: "stopped",
            mc_version: "1.21.4",
            loader: "paper",
            image: "itzg/minecraft-server",
            memory_mb: 2048,
            internal_port: 30_500,
            sleep_when_empty: false,
        })
        .await
        .unwrap()
        .id;
    let root = servers_dir.join("smp");
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/a.yml"), "a: 1\n").unwrap();
    Harness {
        app,
        store,
        auth,
        owner,
        root,
        server_id,
        _tmp: tmp,
    }
}

impl Harness {
    async fn rpc(
        &self,
        service: &str,
        method: &str,
        body: Value,
        session: &str,
    ) -> (StatusCode, Value) {
        let req = Request::post(format!("/api/rpc/nethernode.v1.{service}/{method}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!("nn_csrf={CSRF}; nn_session={session}"),
            )
            .header("x-csrf-token", CSRF)
            .body(Body::from(body.to_string()))
            .unwrap();
        send(&self.app, req).await
    }

    async fn files(&self, method: &str, mut body: Value) -> (StatusCode, Value) {
        body["serverId"] = json!(self.server_id);
        self.rpc("FileService", method, body, &self.owner).await
    }

    async fn viewer(&self) -> String {
        let hash = password::hash_password(PASSWORD).unwrap();
        self.store
            .create_user(NewUser {
                username: "viewer",
                password_hash: &hash,
                role: "viewer",
            })
            .await
            .unwrap();
        self.auth
            .login("viewer", PASSWORD, Default::default())
            .await
            .unwrap()
            .token
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn browse_edit_and_trash() {
    let h = harness().await;
    let (status, body) = h.files("ListFiles", json!({ "path": "" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"][0]["name"], "config");
    assert_eq!(body["entries"][0]["kind"], "FILE_KIND_DIRECTORY");

    let (status, read) = h.files("ReadFile", json!({ "path": "config/a.yml" })).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(
        B64.decode(read["content"].as_str().unwrap()).unwrap(),
        b"a: 1\n"
    );
    assert_eq!(read["isText"], true);

    // The server changes the file after the editor opened it.
    std::fs::write(h.root.join("config/a.yml"), "a: 2\n").unwrap();
    let save = json!({ "path": "config/a.yml", "content": B64.encode("a: 3\n"), "expectedVersion": read["version"] });
    let (status, body) = h.files("WriteFile", save).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "aborted");

    let (_, fresh) = h.files("ReadFile", json!({ "path": "config/a.yml" })).await;
    let save = json!({ "path": "config/a.yml", "content": B64.encode("a: 3\n"), "expectedVersion": fresh["version"] });
    assert_eq!(h.files("WriteFile", save).await.0, StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(h.root.join("config/a.yml")).unwrap(),
        "a: 3\n"
    );

    let (status, body) = h.files("DeleteFiles", json!({ "paths": ["config"] })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let trash_id = body["items"][0]["id"].as_str().unwrap().to_owned();
    assert!(!h.root.join("config").exists());
    let (_, trash) = h.files("ListTrash", json!({})).await;
    assert_eq!(trash["items"][0]["originalPath"], "config");
    assert_eq!(trash["retentionDays"], 30);
    let (status, body) = h.files("RestoreFile", json!({ "trashId": trash_id })).await;
    assert_eq!(
        (status, body["path"].as_str()),
        (StatusCode::OK, Some("config"))
    );

    let audit: Vec<String> = h
        .store
        .recent_audit(50)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect();
    for action in ["file.write", "file.delete", "file.restore"] {
        assert!(
            audit.contains(&action.to_owned()),
            "{action} audited: {audit:?}"
        );
    }
}

#[tokio::test]
async fn traversal_is_rejected_over_rpc() {
    let h = harness().await;
    for path in ["../../etc/passwd", "/etc/passwd", ".trash"] {
        let (status, body) = h.files("ReadFile", json!({ "path": path })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
    }
    let (status, _) = h
        .files("RestoreFile", json!({ "trashId": "../../../etc" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn viewers_cannot_touch_files() {
    let h = harness().await;
    let viewer = h.viewer().await;
    for method in ["ListFiles", "ReadFile", "WriteFile", "DeleteFiles"] {
        let body = json!({ "serverId": h.server_id, "path": "config/a.yml", "paths": ["config"] });
        let (status, _) = h.rpc("FileService", method, body, &viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}");
    }
    let req = Request::get(format!(
        "/api/files/download?server={}&path=config/a.yml",
        h.server_id
    ))
    .header(header::COOKIE, format!("nn_session={viewer}"))
    .body(Body::empty())
    .unwrap();
    assert_eq!(send(&h.app, req).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn downloads_support_ranges() {
    let h = harness().await;
    std::fs::write(h.root.join("big.bin"), (0u8..=255).collect::<Vec<_>>()).unwrap();
    let url = format!("/api/files/download?server={}&path=big.bin", h.server_id);
    let get = |range: Option<&str>| {
        let mut req = Request::get(&url).header(header::COOKIE, format!("nn_session={}", h.owner));
        if let Some(r) = range {
            req = req.header(header::RANGE, r);
        }
        req.body(Body::empty()).unwrap()
    };

    let response = h.app.clone().oneshot(get(None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "256");
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .len(),
        256
    );

    let response = h
        .app
        .clone()
        .oneshot(get(Some("bytes=10-19")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 10-19/256");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], &(10u8..20).collect::<Vec<_>>()[..]);

    let response = h
        .app
        .clone()
        .oneshot(get(Some("bytes=300-")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    let escape = format!("/api/files/download?server={}&path=../../x", h.server_id);
    let req = Request::get(escape)
        .header(header::COOKIE, format!("nn_session={}", h.owner))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&h.app, req).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn server_properties_form() {
    let h = harness().await;
    let (status, body) = h
        .rpc(
            "ServerService",
            "GetServerProperties",
            json!({ "id": h.server_id }),
            &h.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["exists"], Value::Null, "false is omitted: no file yet");

    std::fs::write(
        h.root.join("server.properties"),
        "#Minecraft server properties\ndifficulty=easy\nmotd=\\u00A7aHi\nrcon.password=hunter2\nserver-port=25565\nsome-mod-key=x\n",
    )
    .unwrap();
    let (_, body) = h
        .rpc(
            "ServerService",
            "GetServerProperties",
            json!({ "id": h.server_id }),
            &h.owner,
        )
        .await;
    let props = body["properties"].as_array().unwrap();
    let find = |key: &str| props.iter().find(|p| p["key"] == key).unwrap().clone();
    assert_eq!(find("difficulty")["type"], "PROPERTY_TYPE_CHOICE");
    assert_eq!(find("motd")["value"], "§aHi");
    assert!(
        find("motd")["managedReason"]
            .as_str()
            .unwrap()
            .contains("Server settings")
    );
    assert_eq!(
        find("rcon.password")["value"],
        Value::Null,
        "secret never sent"
    );
    assert_eq!(find("some-mod-key")["group"], "Other");
    let version = body["version"].clone();

    let update = |values: Value, version: Value| json!({ "id": h.server_id, "values": values, "version": version });
    let (status, _) = h
        .rpc(
            "ServerService",
            "UpdateServerProperties",
            update(json!({ "server-port": "1" }), version.clone()),
            &h.owner,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "managed keys are rejected");
    let (status, _) = h
        .rpc(
            "ServerService",
            "UpdateServerProperties",
            update(json!({ "difficulty": "nightmare" }), version.clone()),
            &h.owner,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "invalid choice");
    let (status, body) = h
        .rpc(
            "ServerService",
            "UpdateServerProperties",
            update(json!({ "difficulty": "hard" }), version.clone()),
            &h.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let text = std::fs::read_to_string(h.root.join("server.properties")).unwrap();
    assert!(text.contains("difficulty=hard\n"));
    assert!(
        text.contains("motd=\\u00A7aHi\n"),
        "untouched lines kept as written"
    );
    let (status, _) = h
        .rpc(
            "ServerService",
            "UpdateServerProperties",
            update(json!({ "difficulty": "easy" }), version),
            &h.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale version");
}

#[tokio::test]
async fn console_requires_a_session_role_and_same_origin() {
    let h = harness().await;
    let viewer = h.viewer().await;
    let ws = |session: Option<&str>, origin: &str| {
        let mut req = Request::get(format!("/ws/servers/{}/console", h.server_id))
            .header(header::HOST, "panel.local:8080")
            .header(header::ORIGIN, origin)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
        if let Some(s) = session {
            req = req.header(header::COOKIE, format!("nn_session={s}"));
        }
        req.body(Body::empty()).unwrap()
    };
    let status = |req: Request<Body>| async { h.app.clone().oneshot(req).await.unwrap().status() };
    assert_eq!(
        status(ws(None, "http://panel.local:8080")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(ws(Some(&viewer), "http://panel.local:8080")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(ws(Some(&h.owner), "http://evil.local:8080")).await,
        StatusCode::FORBIDDEN
    );
    // Allowed past the checks; this harness has no Docker.
    assert_eq!(
        status(ws(Some(&h.owner), "http://panel.local:8080")).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
