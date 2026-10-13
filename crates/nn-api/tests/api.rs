//! End-to-end tests of the HTTP surface: auth flow, CSRF, and redirects.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, SETUP_REQUIRED_HEADER};
use nn_auth::{Auth, AuthConfig};
use nn_events::EventBus;
use nn_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

const AUTH: &str = "/api/rpc/nethernode.v1.AuthService";
const CSRF: &str = "test-csrf-token";
const PASSWORD: &str = "correct horse battery";

async fn app() -> Router {
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        Arc::new(nn_proxy::NoopHooks),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    nn_api::router(AppState {
        proxy: Arc::new(nn_api::ProxyRuntime::new(proxy, store.clone(), false)),
        auth: Arc::new(Auth::new(store, AuthConfig::default())),
        events,
        config: Arc::new(ApiConfig::default()),
        metrics: None,
    })
}

struct Reply {
    status: StatusCode,
    headers: http::HeaderMap,
    body: Value,
}

impl Reply {
    /// The value of a cookie set by this response.
    fn cookie(&self, name: &str) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| {
                v.split(';')
                    .next()?
                    .strip_prefix(&format!("{name}="))
                    .map(str::to_owned)
            })
    }
}

/// Calls a unary RPC with the Connect JSON protocol.
async fn rpc(app: &Router, method: &str, body: Value, session: Option<&str>, csrf: bool) -> Reply {
    let mut cookie = String::new();
    let mut req =
        Request::post(format!("{AUTH}/{method}")).header(header::CONTENT_TYPE, "application/json");
    if csrf {
        req = req.header("x-csrf-token", CSRF);
        cookie.push_str(&format!("nn_csrf={CSRF}; "));
    }
    if let Some(s) = session {
        cookie.push_str(&format!("nn_session={s}"));
    }
    if !cookie.is_empty() {
        req = req.header(header::COOKIE, cookie);
    }
    send(app, req.body(Body::from(body.to_string())).unwrap()).await
}

async fn get(app: &Router, path: &str, session: Option<&str>) -> Reply {
    let mut req = Request::get(path);
    if let Some(s) = session {
        req = req.header(header::COOKIE, format!("nn_session={s}"));
    }
    send(app, req.body(Body::empty()).unwrap()).await
}

async fn send(app: &Router, req: Request<Body>) -> Reply {
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        headers,
        body,
    }
}

async fn setup(app: &Router) -> String {
    let reply = rpc(
        app,
        "Setup",
        json!({ "username": "admin", "password": PASSWORD }),
        None,
        true,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    reply.cookie("nn_session").expect("session cookie")
}

#[tokio::test]
async fn unauthenticated_rpc_is_401_and_flags_pending_setup() {
    let app = app().await;
    let reply = rpc(&app, "Me", json!({}), None, true).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(reply.body["code"], "unauthenticated");
    assert_eq!(reply.headers.get(SETUP_REQUIRED_HEADER).unwrap(), "1");

    setup(&app).await;
    let reply = rpc(&app, "Me", json!({}), None, true).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(reply.headers.get(SETUP_REQUIRED_HEADER).is_none());
}

#[tokio::test]
async fn mutations_without_csrf_are_403() {
    let app = app().await;
    let body = json!({ "username": "admin", "password": PASSWORD });
    let reply = rpc(&app, "Setup", body.clone(), None, false).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.body["code"], "permission_denied");
    // A fresh CSRF cookie is handed out so the client can retry.
    assert!(reply.cookie("nn_csrf").is_some());

    // Header present but not matching the cookie.
    let req = Request::post(format!("{AUTH}/Setup"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, "nn_csrf=one")
        .header("x-csrf-token", "two")
        .body(Body::from(body.to_string()))
        .unwrap();
    assert_eq!(send(&app, req).await.status, StatusCode::FORBIDDEN);

    // Authenticated mutations need it too.
    let session = setup(&app).await;
    let reply = rpc(&app, "Logout", json!({}), Some(&session), false).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn setup_login_me_logout_flow() {
    let app = app().await;
    let session = setup(&app).await;

    let me = rpc(&app, "Me", json!({}), Some(&session), true).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["user"]["username"], "admin");
    assert_eq!(me.body["user"]["role"], "ROLE_OWNER");

    // Setup can't run twice.
    let again = rpc(
        &app,
        "Setup",
        json!({ "username": "x2x", "password": PASSWORD }),
        None,
        true,
    )
    .await;
    assert_eq!(again.body["code"], "failed_precondition");

    let bad = rpc(
        &app,
        "Login",
        json!({ "username": "admin", "password": "nope nope nope" }),
        None,
        true,
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);

    let login = rpc(
        &app,
        "Login",
        json!({ "username": "admin", "password": PASSWORD }),
        None,
        true,
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);
    let set_cookie: Vec<_> = login.headers.get_all(header::SET_COOKIE).iter().collect();
    let session_cookie = set_cookie
        .iter()
        .find(|c| c.to_str().unwrap().starts_with("nn_session="))
        .unwrap()
        .to_str()
        .unwrap();
    assert!(session_cookie.contains("HttpOnly"));
    assert!(session_cookie.contains("SameSite=Lax"));
    assert!(
        login.cookie("nn_csrf").is_some(),
        "CSRF token rotates on login"
    );
    let session2 = login.cookie("nn_session").unwrap();

    let out = rpc(&app, "Logout", json!({}), Some(&session2), true).await;
    assert_eq!(out.status, StatusCode::OK);
    assert_eq!(out.cookie("nn_session").as_deref(), Some(""));
    let me = rpc(&app, "Me", json!({}), Some(&session2), true).await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
    // The setup session is independent and still valid.
    assert_eq!(
        rpc(&app, "Me", json!({}), Some(&session), true)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn cookies_are_secure_behind_https_proxy() {
    let app = app().await;
    let req = Request::post(format!("{AUTH}/Setup"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, format!("nn_csrf={CSRF}"))
        .header("x-csrf-token", CSRF)
        .header("x-forwarded-proto", "https")
        .body(Body::from(
            json!({ "username": "admin", "password": PASSWORD }).to_string(),
        ))
        .unwrap();
    let reply = send(&app, req).await;
    assert!(
        reply
            .headers
            .get_all(header::SET_COOKIE)
            .iter()
            .all(|c| c.to_str().unwrap().contains("; Secure"))
    );
}

#[tokio::test]
async fn pages_redirect_by_setup_and_session_state() {
    let app = app().await;
    let location = |r: &Reply| {
        r.headers
            .get(header::LOCATION)
            .map(|v| v.to_str().unwrap().to_owned())
    };

    // Before setup: everything goes to /setup.
    for path in ["/", "/login", "/servers"] {
        let reply = get(&app, path, None).await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(location(&reply).as_deref(), Some("/setup"), "{path}");
    }
    // /setup itself is served (503 here because tests run without a UI build,
    // or 200 when web/dist exists).
    let reply = get(&app, "/setup", None).await;
    assert!(location(&reply).is_none());

    let session = setup(&app).await;
    assert_eq!(
        location(&get(&app, "/setup", None).await).as_deref(),
        Some("/login")
    );
    assert_eq!(
        location(&get(&app, "/", None).await).as_deref(),
        Some("/login")
    );
    assert!(location(&get(&app, "/login", None).await).is_none());
    assert_eq!(
        location(&get(&app, "/login", Some(&session)).await).as_deref(),
        Some("/")
    );
    assert!(location(&get(&app, "/", Some(&session)).await).is_none());
}

#[tokio::test]
async fn unknown_api_paths_are_404_not_spa() {
    let app = app().await;
    setup(&app).await;
    assert_eq!(
        get(&app, "/api/nope", None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/assets/missing.js", None).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn healthz_and_metrics_access() {
    let app = app().await;
    let health = get(&app, "/healthz", None).await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(health.body["status"], "ok");

    assert_eq!(
        get(&app, "/metrics", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let session = setup(&app).await;
    // No recorder is installed in tests, but the owner gets past auth.
    assert_eq!(
        get(&app, "/metrics", Some(&session)).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn responses_hand_out_csrf_cookie_and_security_headers() {
    let app = app().await;
    let reply = get(&app, "/healthz", None).await;
    assert!(reply.cookie("nn_csrf").is_some());
    assert_eq!(
        reply.headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(reply.headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");

    let req = Request::get("/healthz")
        .header(header::COOKIE, "nn_csrf=existing")
        .body(Body::empty())
        .unwrap();
    assert!(send(&app, req).await.cookie("nn_csrf").is_none());
}

#[tokio::test]
async fn non_get_page_requests_are_rejected() {
    let app = app().await;
    let req = Request::builder()
        .method(Method::DELETE)
        .uri("/whatever")
        .header(header::COOKIE, format!("nn_csrf={CSRF}"))
        .header("x-csrf-token", CSRF)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, req).await.status, StatusCode::METHOD_NOT_ALLOWED);
}
