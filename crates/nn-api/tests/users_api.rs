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
    let key = Arc::new(SecretBox::from_hex_key(&"ab".repeat(32)).unwrap());
    let auth =
        Arc::new(Auth::new(store.clone(), AuthConfig::default()).with_secrets(Some(key.clone())));
    let content = Arc::new(nn_api::content::Content::new(
        nn_catalog::Catalog::new(None, Default::default()),
        Some(key),
        None,
    ));
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
        content,
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

#[tokio::test]
async fn the_audit_log_is_filtered_paged_and_for_admins() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let created = owner
        .ok(
            "UserService/CreateUser",
            json!({ "username": "watcher", "role": "ROLE_VIEWER" }),
        )
        .await;
    let watcher_id = created["user"]["id"].as_str().unwrap().to_owned();
    owner
        .ok("UserService/ResetUserPassword", json!({ "id": watcher_id }))
        .await;

    let all = owner.ok("AuditService/ListAudit", json!({})).await;
    let actions: Vec<&str> = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        ["user.reset_password", "user.create", "auth.setup"],
        "newest first"
    );
    assert_eq!(all["entries"][0]["username"], "owner");
    assert_eq!(all["entries"][0]["targetName"], "watcher");
    assert!(all["actors"].as_array().unwrap().len() == 2);

    let users = owner
        .ok(
            "AuditService/ListAudit",
            json!({ "category": "user", "limit": 1 }),
        )
        .await;
    assert_eq!(users["entries"].as_array().unwrap().len(), 1);
    let next = users["nextBeforeId"].as_str().unwrap();
    let page = owner
        .ok(
            "AuditService/ListAudit",
            json!({ "category": "user", "limit": 1, "beforeId": next }),
        )
        .await;
    assert_eq!(page["entries"][0]["action"], "user.create");
    let (status, _) = owner
        .call("AuditService/ListAudit", json!({ "category": "nope" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Viewers see the activity feed, not the log.
    let mut watcher = Client::new(&app, None);
    let reset = owner
        .ok("UserService/ResetUserPassword", json!({ "id": watcher_id }))
        .await;
    watcher
        .ok(
            "AuthService/Login",
            json!({ "username": "watcher", "password": reset["password"] }),
        )
        .await;
    watcher
        .ok(
            "AccountService/ChangePassword",
            json!({ "currentPassword": reset["password"], "newPassword": "watching closely" }),
        )
        .await;
    let (status, _) = watcher.call("AuditService/ListAudit", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let activity = watcher.ok("AuditService/ListActivity", json!({})).await;
    assert!(
        activity.get("items").is_none(),
        "no server activity yet, and account changes aren't shown: {activity}"
    );
}

#[tokio::test]
async fn discord_notifications_are_configured_by_admins() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let fresh = owner
        .ok("NotificationService/GetNotifications", json!({}))
        .await;
    assert!(fresh.get("webhookHint").is_none());
    assert_eq!(fresh["events"]["crash"], true, "crashes are on by default");

    let (status, body) = owner
        .call(
            "NotificationService/UpdateNotifications",
            json!({ "webhookUrl": "https://evil.example/api/webhooks/1/x", "events": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    owner
        .ok(
            "NotificationService/UpdateNotifications",
            json!({
                "webhookUrl": "https://discord.com/api/webhooks/123/secret-token-abcdef",
                "events": { "startStop": true, "playerJoin": true }
            }),
        )
        .await;
    let saved = owner
        .ok("NotificationService/GetNotifications", json!({}))
        .await;
    assert_eq!(saved["webhookHint"], "…abcdef");
    assert_eq!(saved["events"]["playerJoin"], true);
    assert!(saved["events"].get("crash").is_none(), "turned off");
    // Leaving the URL out keeps it.
    owner
        .ok(
            "NotificationService/UpdateNotifications",
            json!({ "events": { "crash": true } }),
        )
        .await;
    let kept = owner
        .ok("NotificationService/GetNotifications", json!({}))
        .await;
    assert_eq!(kept["webhookHint"], "…abcdef");
    owner
        .ok(
            "NotificationService/UpdateNotifications",
            json!({ "webhookUrl": "", "events": {} }),
        )
        .await;
    let cleared = owner
        .ok("NotificationService/GetNotifications", json!({}))
        .await;
    assert!(cleared.get("webhookHint").is_none());
    let (status, _) = owner
        .call("NotificationService/SendTestNotification", json!({}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no webhook to test");

    let created = owner
        .ok(
            "UserService/CreateUser",
            json!({ "username": "viewer", "role": "ROLE_VIEWER" }),
        )
        .await;
    let mut viewer = Client::new(&app, None);
    viewer
        .ok(
            "AuthService/Login",
            json!({ "username": "viewer", "password": created["password"] }),
        )
        .await;
    viewer
        .ok(
            "AccountService/ChangePassword",
            json!({ "currentPassword": created["password"], "newPassword": "only looking here" }),
        )
        .await;
    let (status, _) = viewer
        .call("NotificationService/GetNotifications", json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_install_count_is_off_until_an_owner_turns_it_on() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let off = owner
        .ok("InstallCountService/GetInstallCount", json!({}))
        .await;
    assert!(off.get("enabled").is_none() && off.get("payload").is_none());
    owner
        .ok(
            "InstallCountService/SetInstallCount",
            json!({ "enabled": true }),
        )
        .await;
    let on = owner
        .ok("InstallCountService/GetInstallCount", json!({}))
        .await;
    assert_eq!(on["enabled"], true);
    let payload: Value = serde_json::from_str(on["payload"].as_str().unwrap()).unwrap();
    assert_eq!(
        payload.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["id", "version"]
    );
    owner
        .ok(
            "InstallCountService/SetInstallCount",
            json!({ "enabled": false }),
        )
        .await;

    // Administrators can't change it; it's the owners' decision.
    let created = owner
        .ok(
            "UserService/CreateUser",
            json!({ "username": "admin2", "role": "ROLE_ADMIN" }),
        )
        .await;
    let mut admin = Client::new(&app, None);
    admin
        .ok(
            "AuthService/Login",
            json!({ "username": "admin2", "password": created["password"] }),
        )
        .await;
    admin
        .ok(
            "AccountService/ChangePassword",
            json!({ "currentPassword": created["password"], "newPassword": "administrating now" }),
        )
        .await;
    let (status, _) = admin
        .call(
            "InstallCountService/SetInstallCount",
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn each_account_keeps_its_own_theme() {
    let (app, owner) = app().await;
    let mut owner = Client::new(&app, Some(owner));
    let prefs = owner.ok("AccountService/GetPreferences", json!({})).await;
    assert_eq!(prefs["preferences"]["theme"], "THEME_SYSTEM");

    let updated = owner
        .ok(
            "AccountService/UpdatePreferences",
            json!({ "preferences": { "theme": "THEME_LIGHT" } }),
        )
        .await;
    assert_eq!(updated["preferences"]["theme"], "THEME_LIGHT");
    let prefs = owner.ok("AccountService/GetPreferences", json!({})).await;
    assert_eq!(prefs["preferences"]["theme"], "THEME_LIGHT");
    let (status, _) = owner
        .call(
            "AccountService/UpdatePreferences",
            json!({ "preferences": {} }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Another account, even a viewer, has its own.
    let created = owner
        .ok(
            "UserService/CreateUser",
            json!({ "username": "watcher", "role": "ROLE_VIEWER" }),
        )
        .await;
    let temporary = created["password"].as_str().unwrap().to_owned();
    let mut watcher = Client::new(&app, None);
    watcher
        .ok(
            "AuthService/Login",
            json!({ "username": "watcher", "password": temporary }),
        )
        .await;
    watcher
        .ok(
            "AccountService/ChangePassword",
            json!({ "currentPassword": temporary, "newPassword": "a brand new password" }),
        )
        .await;
    let prefs = watcher.ok("AccountService/GetPreferences", json!({})).await;
    assert_eq!(prefs["preferences"]["theme"], "THEME_SYSTEM");
    watcher
        .ok(
            "AccountService/UpdatePreferences",
            json!({ "preferences": { "theme": "THEME_DARK" } }),
        )
        .await;
    let prefs = owner.ok("AccountService/GetPreferences", json!({})).await;
    assert_eq!(prefs["preferences"]["theme"], "THEME_LIGHT");

    let (status, _) = Client::new(&app, None)
        .call("AccountService/GetPreferences", json!({}))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
