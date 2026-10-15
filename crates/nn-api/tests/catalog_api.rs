//! CatalogService rules that don't need the internet: who may call it, and
//! what happens without a CurseForge key or a data key.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, password};
use nn_events::EventBus;
use nn_node::Nodes;
use nn_store::{NewUser, Store};
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";

async fn harness() -> (Router, String, String) {
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
        agents: Default::default(),
        nodes: Nodes::default(),
    });
    let owner = auth
        .setup("owner", PASSWORD, Default::default())
        .await
        .unwrap()
        .token;
    let hash = password::hash_password(PASSWORD).unwrap();
    store
        .create_user(NewUser {
            username: "mod",
            password_hash: &hash,
            role: "moderator",
            must_change_password: false,
        })
        .await
        .unwrap();
    let moderator = auth
        .login("mod", PASSWORD, Default::default())
        .await
        .unwrap()
        .session()
        .unwrap()
        .token;
    (app, owner, moderator)
}

async fn call(app: &Router, session: &str, method: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::post(format!("/api/rpc/nethernode.v1.CatalogService/{method}"))
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
async fn catalog_is_for_admins() {
    let (app, _, moderator) = harness().await;
    let (status, _) = call(
        &app,
        &moderator,
        "ListMcVersions",
        json!({ "loader": "vanilla" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, &moderator, "SetCurseForgeKey", json!({ "key": "x" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn curseforge_without_a_key() {
    let (app, owner, _) = harness().await;
    let (status, body) = call(&app, &owner, "GetCurseForgeStatus", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(body["configured"], true);
    let (status, body) = call(
        &app,
        &owner,
        "SearchProjects",
        json!({ "platform": "PLATFORM_CURSEFORGE", "kind": "PROJECT_KIND_MODPACK", "query": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "failed_precondition");
    assert!(body["message"].as_str().unwrap().contains("API key"));
    let (status, body) = call(&app, &owner, "SearchProjects", json!({ "query": "x" })).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a platform is required: {body}"
    );
    // Removing a key that was never set is fine; nothing to seal it with is
    // reported when saving one (checked with CurseForge first, so this
    // only runs the empty case offline).
    let (status, body) = call(&app, &owner, "SetCurseForgeKey", json!({ "key": "" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
