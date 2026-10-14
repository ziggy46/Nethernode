//! Accounts over HTTP: owner-managed users with one-time passwords,
//! two-factor sign-in, sessions, and keeping an owner.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use nn_api::{ApiConfig, AppState, ProxyRuntime, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, SecretBox, totp};
use nn_events::EventBus;
use nn_store::Store;
use serde_json::{Value, json};
use tower::ServiceExt;

const CSRF: &str = "csrf";
const PASSWORD: &str = "correct horse battery";

async fn app() -> (Router, String) {
    let store = Store::open_in_memory().await.unwrap();
    store.migrate().await.unwrap();
    let events = EventBus::default();
    let proxy = nn_proxy::Proxy::new(
        StoreProxyHooks::new(store.clone()),
        events.clone(),
        nn_proxy::Limits::default(),
    );
    let key = SecretBox::from_hex_key(&"ab".repeat(32)).unwrap();
    let auth =
        Arc::new(Auth::new(store.clone(), AuthConfig::default()).with_secrets(Some(Arc::new(key))));
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
        servers: Default::default(),
    });
    let owner = auth
        .setup("owner", PASSWORD, Default::default())
        .await
        .unwrap()
        .token;
    (app, owner)
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

fn code_for(secret: &str) -> String {
    let key = totp::decode_secret(secret).unwrap();
    let step = totp::step_at(nn_store::now_ms() / 1000);
    format!("{:06}", totp::code_at(&key, step, totp::DIGITS))
}

#[tokio::test]
async fn new_users_must_choose_a_password_first() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let created = owner
        .ok(
            "UserService/CreateUser",
            json!({ "username": "builder", "role": "ROLE_MODERATOR" }),
        )
        .await;
    let temporary = created["password"].as_str().unwrap().to_owned();
    assert_eq!(created["user"]["mustChangePassword"], true);
    let (status, _) = owner
        .call(
            "UserService/CreateUser",
            json!({ "username": "Builder", "role": "ROLE_VIEWER" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let mut builder = Client::new(&app, None);
    let login = builder
        .ok(
            "AuthService/Login",
            json!({ "username": "builder", "password": temporary }),
        )
        .await;
    assert_eq!(login["user"]["mustChangePassword"], true);
    // Until the password changes, only Me, Logout, and ChangePassword work.
    let me = builder.ok("AuthService/Me", json!({})).await;
    assert_eq!(me["user"]["username"], "builder");
    let (status, body) = builder.call("ServerService/ListServers", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body["message"].as_str().unwrap().contains("new password"));
    let (status, _) = builder
        .call(
            "AccountService/ChangePassword",
            json!({ "currentPassword": "wrong one!", "newPassword": "a brand new password" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let changed = builder
        .ok(
            "AccountService/ChangePassword",
            json!({ "currentPassword": temporary, "newPassword": "a brand new password" }),
        )
        .await;
    assert!(changed["user"].get("mustChangePassword").is_none());
    builder.ok("ServerService/ListServers", json!({})).await;

    // Moderators don't manage users.
    let (status, _) = builder.call("UserService/ListUsers", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let users = owner.ok("UserService/ListUsers", json!({})).await;
    let row = users["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["user"]["username"] == "builder")
        .unwrap()
        .clone();
    assert_eq!(row["activeSessions"], "1");
    let id = row["user"]["id"].as_str().unwrap();

    // Revoking signs them out; a reset forces another change.
    owner
        .ok("UserService/RevokeUserSessions", json!({ "id": id }))
        .await;
    let (status, _) = builder.call("ServerService/ListServers", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let reset = owner
        .ok("UserService/ResetUserPassword", json!({ "id": id }))
        .await;
    let login = builder
        .ok(
            "AuthService/Login",
            json!({ "username": "builder", "password": reset["password"] }),
        )
        .await;
    assert_eq!(login["user"]["mustChangePassword"], true);

    owner
        .ok("UserService/DeleteUser", json!({ "id": id }))
        .await;
    let (status, _) = builder.call("AuthService/Me", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn two_factor_sign_in_over_http() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let security = owner.ok("AccountService/GetSecurity", json!({})).await;
    assert_eq!(security["totpAvailable"], true);
    assert_eq!(security["sessions"][0]["current"], true);

    let setup = owner.ok("AccountService/BeginTotpSetup", json!({})).await;
    let secret = setup["secret"].as_str().unwrap().to_owned();
    assert!(setup["qrSvg"].as_str().unwrap().contains("<svg"));
    let (status, _) = owner
        .call(
            "AccountService/ConfirmTotpSetup",
            json!({ "code": "000000" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let confirmed = owner
        .ok(
            "AccountService/ConfirmTotpSetup",
            json!({ "code": code_for(&secret) }),
        )
        .await;
    let recovery: Vec<String> = serde_json::from_value(confirmed["recoveryCodes"].clone()).unwrap();
    assert_eq!(recovery.len(), 10);

    // A new browser: the password alone gets a challenge, not a session.
    let mut laptop = Client::new(&app, None);
    let login = laptop
        .ok(
            "AuthService/Login",
            json!({ "username": "owner", "password": PASSWORD }),
        )
        .await;
    assert!(laptop.session.is_none());
    assert!(login.get("user").is_none());
    let challenge = login["secondFactorChallenge"].as_str().unwrap().to_owned();
    // The enrollment code can't be used again.
    let (status, _) = laptop
        .call(
            "AuthService/FinishSignIn",
            json!({ "challenge": challenge, "code": code_for(&secret) }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let done = laptop
        .ok(
            "AuthService/FinishSignIn",
            json!({ "challenge": challenge, "code": recovery[0] }),
        )
        .await;
    assert_eq!(done["user"]["totpEnabled"], true);
    assert!(laptop.session.is_some());
    laptop.ok("ServerService/ListServers", json!({})).await;

    // The first browser can end the second one's session.
    let security = owner.ok("AccountService/GetSecurity", json!({})).await;
    assert_eq!(security["recoveryCodesLeft"], "9");
    let other = security["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s.get("current").is_none())
        .unwrap()["id"]
        .clone();
    owner
        .ok("AccountService/RevokeSession", json!({ "id": other }))
        .await;
    let (status, _) = laptop.call("ServerService/ListServers", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = owner
        .call(
            "AccountService/DisableTotp",
            json!({ "password": "nope nope nope" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    owner
        .ok(
            "AccountService/DisableTotp",
            json!({ "password": PASSWORD }),
        )
        .await;
    let login = laptop
        .ok(
            "AuthService/Login",
            json!({ "username": "owner", "password": PASSWORD }),
        )
        .await;
    assert!(login["user"].is_object() && laptop.session.is_some());
}

#[tokio::test]
async fn there_is_always_an_owner() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let me = owner.ok("AuthService/Me", json!({})).await;
    let id = me["user"]["id"].as_str().unwrap().to_owned();
    let (status, body) = owner
        .call(
            "UserService/SetUserRole",
            json!({ "id": id, "role": "ROLE_ADMIN" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["message"].as_str().unwrap().contains("owner"));
    let (status, _) = owner
        .call("UserService/DeleteUser", json!({ "id": id }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // With a second owner, either can step down.
    let created = owner
        .ok(
            "UserService/CreateUser",
            json!({ "username": "second", "role": "ROLE_OWNER" }),
        )
        .await;
    let second = created["user"]["id"].as_str().unwrap().to_owned();
    owner
        .ok(
            "UserService/SetUserRole",
            json!({ "id": second, "role": "ROLE_VIEWER" }),
        )
        .await;
    let (status, _) = owner
        .call(
            "UserService/SetUserRole",
            json!({ "id": id, "role": "ROLE_ADMIN" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "the only owner again");
}
