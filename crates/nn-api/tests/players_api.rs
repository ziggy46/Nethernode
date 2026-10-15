//! PlayerService without Docker: lists read from a server's files, the
//! permission rules, IP masking, and input validation. Actions against real
//! servers are exercised on a Docker host.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, ServerManager, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, password};
use nn_events::EventBus;
use nn_store::{NewPlayerSession, NewServer, NewUser, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";
const NOTCH: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
const JEB: &str = "853c80ef-3c37-49fd-aa49-938b674adae6";

struct Harness {
    app: Router,
    store: Store,
    auth: Arc<Auth>,
    owner: String,
    server_id: String,
    _tmp: tempfile::TempDir,
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let servers_dir = tmp.path().canonicalize().unwrap().join("servers");
    let root: PathBuf = servers_dir.join("smp");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("ops.json"),
        format!(r#"[{{"uuid":"{JEB}","name":"jeb_","level":3,"bypassesPlayerLimit":false}}]"#),
    )
    .unwrap();
    std::fs::write(
        root.join("whitelist.json"),
        format!(r#"[{{"uuid":"{JEB}","name":"jeb_"}}]"#),
    )
    .unwrap();
    std::fs::write(
        root.join("banned-players.json"),
        format!(
            r#"[{{"uuid":"{NOTCH}","name":"Notch","created":"2026-10-13 22:11:15 +0000","source":"Rcon","expires":"2030-01-02 03:04:05 +0000","reason":"timed"}}]"#
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("server.properties"),
        "white-list=true\nonline-mode=true\n",
    )
    .unwrap();

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
        content: Default::default(),
        notifier: Default::default(),
        install_count: Default::default(),
        servers: ServerManager {
            servers_dir: Some(servers_dir),
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
    let session = store
        .open_player_session(NewPlayerSession {
            server_id: &server_id,
            uuid: Some(JEB),
            username: "jeb_",
            ip: Some("203.0.113.7"),
        })
        .await
        .unwrap();
    store.close_player_session(&session).await.unwrap();
    Harness {
        app,
        store,
        auth,
        owner,
        server_id,
        _tmp: tmp,
    }
}

impl Harness {
    async fn call(&self, method: &str, body: Value, session: &str) -> (StatusCode, Value) {
        let req = Request::post(format!("/api/rpc/nethernode.v1.PlayerService/{method}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::COOKIE,
                format!("nn_csrf={CSRF}; nn_session={session}"),
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

    async fn user(&self, name: &str, role: &str) -> String {
        let hash = password::hash_password(PASSWORD).unwrap();
        self.store
            .create_user(NewUser {
                username: name,
                password_hash: &hash,
                role,
                must_change_password: false,
            })
            .await
            .unwrap();
        self.auth
            .login(name, PASSWORD, Default::default())
            .await
            .unwrap()
            .session()
            .unwrap()
            .token
    }
}

#[tokio::test]
async fn lists_come_from_the_server_files() {
    let h = harness().await;
    let (status, body) = h
        .call("ListPlayers", json!({ "serverId": h.server_id }), &h.owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["allowlistEnabled"], true);
    let players = body["players"].as_array().unwrap();
    let jeb = players.iter().find(|p| p["name"] == "jeb_").unwrap();
    assert_eq!(jeb["opLevel"], 3);
    assert_eq!(jeb["allowlisted"], true);
    assert_eq!(jeb["lastIp"], "203.0.113.7", "admins see full addresses");
    let notch = players.iter().find(|p| p["name"] == "Notch").unwrap();
    assert_eq!(notch["ban"]["reason"], "timed");
    assert!(
        notch["ban"]["expiresMs"].is_string(),
        "int64 as a JSON string"
    );
    assert_eq!(
        body["capabilities"]["backend"], "unavailable",
        "without Docker, NetherNode can't tell whether the server is running"
    );

    let (status, body) = h.call("ListPlayers", json!({}), &h.owner).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["players"].as_array().unwrap().len(),
        1,
        "only players seen through the proxy"
    );
}

#[tokio::test]
async fn roles_and_masking() {
    let h = harness().await;
    let viewer = h.user("viewer", "viewer").await;
    let moderator = h.user("mod", "moderator").await;

    let (status, body) = h
        .call("ListPlayers", json!({ "serverId": h.server_id }), &viewer)
        .await;
    assert_eq!(status, StatusCode::OK);
    let jeb = body["players"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "jeb_")
        .unwrap()
        .clone();
    assert_eq!(jeb["lastIp"], "203.0.113.•••", "masked for non-admins");

    let kick = json!({ "serverId": h.server_id, "player": { "name": "jeb_" } });
    assert_eq!(
        h.call("KickPlayer", kick, &viewer).await.0,
        StatusCode::FORBIDDEN
    );
    let global = json!({ "player": { "name": "jeb_" }, "reason": "x" });
    assert_eq!(
        h.call("BanPlayer", global, &moderator).await.0,
        StatusCode::FORBIDDEN,
        "bans on every server need an administrator"
    );
}

#[tokio::test]
async fn names_are_validated_before_anything_runs() {
    let h = harness().await;
    for name in ["@a", "jeb_ @a", "ThisNameIsWayTooLong", ""] {
        let body = json!({ "serverId": h.server_id, "player": { "name": name }, "reason": "x" });
        let (status, reply) = h.call("BanPlayer", body, &h.owner).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {reply}");
    }
    let body =
        json!({ "serverId": h.server_id, "player": { "name": "jeb_" }, "durationSeconds": 5 });
    assert_eq!(
        h.call("BanPlayer", body, &h.owner).await.0,
        StatusCode::BAD_REQUEST,
        "too short"
    );
}

#[tokio::test]
async fn notes() {
    let h = harness().await;
    let (status, _) = h
        .call(
            "SetPlayerNotes",
            json!({ "uuid": JEB, "notes": "builds the spawn" }),
            &h.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = h.call("GetPlayer", json!({ "uuid": JEB }), &h.owner).await;
    assert_eq!(body["notes"], "builds the spawn");
    assert_eq!(body["sessions"][0]["serverName"], "SMP");
    let (status, _) = h
        .call(
            "SetPlayerNotes",
            json!({ "uuid": NOTCH, "notes": "x" }),
            &h.owner,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "never seen through the proxy"
    );
}

#[tokio::test]
async fn bans_are_enforced_by_the_proxy_hooks() {
    let h = harness().await;
    let hooks = StoreProxyHooks::new(h.store.clone());
    h.store
        .insert_player_ban(nn_store::NewPlayerBan {
            server_id: None,
            uuid: Some(JEB),
            name: "jeb_",
            ip: Some("203.0.113.7"),
            reason: Some("griefing"),
            expires_at: None,
            created_by: None,
        })
        .await
        .unwrap();
    hooks.bans().reload(&h.store).await;
    let login = |ip: &str, name: &str, uuid: Option<&str>| nn_proxy::Admission {
        server_id: h.server_id.clone(),
        ip: ip.parse().unwrap(),
        username: Some(name.to_owned()),
        uuid: uuid.map(str::to_owned),
    };
    let refusal = hooks
        .bans()
        .refusal(&login("203.0.113.7", "someone_else", None))
        .unwrap();
    assert!(
        refusal.contains("banned") && refusal.contains("griefing"),
        "{refusal}"
    );
    // By name (any case), as offline-mode servers may not have the entry.
    assert!(
        hooks
            .bans()
            .refusal(&login("203.0.113.8", "JEB_", None))
            .is_some()
    );
    // By UUID, with or without dashes.
    let undashed = JEB.replace('-', "");
    assert!(
        hooks
            .bans()
            .refusal(&login("203.0.113.8", "renamed", Some(&undashed)))
            .is_some()
    );
    assert!(
        hooks
            .bans()
            .refusal(&login("203.0.113.8", "someone_else", None))
            .is_none()
    );
}
