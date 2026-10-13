//! Authentication and authorization for the NetherNode panel.
//!
//! [`Auth`] covers first-run setup, password login with rate limiting,
//! session issuance and validation, logout, and owner password recovery.
//! It knows nothing about HTTP; `nn-api` maps it onto cookies and RPCs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nn_store::{NewAuditEntry, NewSession, NewUser, Store, now_ms};
use serde_json::json;

pub mod password;
pub mod rate_limit;
pub mod rbac;
pub mod token;

pub use rate_limit::LoginRateLimiter;
pub use rbac::{Permission, Role};

/// Minimum username length.
pub const MIN_USERNAME_LEN: usize = 3;
/// Maximum username length.
pub const MAX_USERNAME_LEN: usize = 32;

/// Errors from authentication operations.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Wrong username or password. Deliberately doesn't say which.
    #[error("invalid username or password")]
    InvalidCredentials,
    /// Too many failed attempts; retry after the given delay.
    #[error("too many login attempts; try again in {} s", retry_after.as_secs().max(1))]
    RateLimited {
        /// How long to wait.
        retry_after: Duration,
    },
    /// Setup was attempted after an owner already exists.
    #[error("setup has already been completed")]
    SetupAlreadyComplete,
    /// User input failed validation. The message is safe to show to users.
    #[error("{0}")]
    InvalidInput(String),
    /// A referenced user does not exist.
    #[error("user not found")]
    UserNotFound,
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] nn_store::Error),
    /// Unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Result alias for authentication operations.
pub type Result<T, E = AuthError> = std::result::Result<T, E>;

/// Tunables for [`Auth`].
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// How long a session lasts without activity. Sessions renew while in use.
    pub session_ttl: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            session_ttl: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

/// Where a request came from, for rate limiting and session records.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientInfo<'a> {
    /// Client IP address.
    pub ip: Option<&'a str>,
    /// `User-Agent` header.
    pub user_agent: Option<&'a str>,
}

/// A user as seen by the rest of the application (no secrets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUser {
    /// ULID.
    pub id: String,
    /// Login name.
    pub username: String,
    /// RBAC role.
    pub role: Role,
    /// Creation time, unix ms.
    pub created_at: i64,
    /// Last successful login, unix ms.
    pub last_login_at: Option<i64>,
}

impl TryFrom<nn_store::User> for AuthUser {
    type Error = AuthError;

    fn try_from(user: nn_store::User) -> Result<Self> {
        let role = user
            .role
            .parse()
            .map_err(|e| AuthError::Internal(format!("{e}")))?;
        Ok(Self {
            id: user.id,
            username: user.username,
            role,
            created_at: user.created_at,
            last_login_at: user.last_login_at,
        })
    }
}

/// A newly created session. `token` goes into the cookie and is not stored.
#[derive(Debug, Clone)]
pub struct IssuedSession {
    /// The signed-in user.
    pub user: AuthUser,
    /// Raw session token.
    pub token: String,
    /// Expiry, unix ms.
    pub expires_at_ms: i64,
}

/// The result of validating a session token.
#[derive(Debug, Clone)]
pub struct Authenticated {
    /// The session's user.
    pub user: AuthUser,
    /// Session ULID.
    pub session_id: String,
    /// Expiry, unix ms (after any renewal).
    pub expires_at_ms: i64,
    /// Whether the expiry was just extended, so the cookie should be reissued.
    pub renewed: bool,
}

/// Authentication service. Share it behind an `Arc`.
#[derive(Debug)]
pub struct Auth {
    store: Store,
    limiter: LoginRateLimiter,
    config: AuthConfig,
    setup_complete: AtomicBool,
}

impl Auth {
    /// Creates the service.
    pub fn new(store: Store, config: AuthConfig) -> Self {
        Self::with_limiter(store, config, LoginRateLimiter::default())
    }

    /// Creates the service with a custom rate limiter.
    pub fn with_limiter(store: Store, config: AuthConfig, limiter: LoginRateLimiter) -> Self {
        Self {
            store,
            limiter,
            config,
            setup_complete: AtomicBool::new(false),
        }
    }

    /// The underlying store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Session lifetime.
    pub fn session_ttl(&self) -> Duration {
        self.config.session_ttl
    }

    /// Whether first-run setup is still needed (no users exist).
    ///
    /// Once setup has completed this is answered from memory.
    pub async fn needs_setup(&self) -> Result<bool> {
        if self.setup_complete.load(Ordering::Acquire) {
            return Ok(false);
        }
        let needs = self.store.count_users().await? == 0;
        if !needs {
            self.setup_complete.store(true, Ordering::Release);
        }
        Ok(needs)
    }

    /// Creates the owner account and signs it in. Only works while no users exist.
    pub async fn setup(
        &self,
        username: &str,
        password: &str,
        client: ClientInfo<'_>,
    ) -> Result<IssuedSession> {
        if !self.needs_setup().await? {
            return Err(AuthError::SetupAlreadyComplete);
        }
        let username = validate_username(username)?;
        password::validate_password(password).map_err(AuthError::InvalidInput)?;
        let hash = hash_blocking(password.to_owned()).await?;

        let created = self
            .store
            .create_first_user(NewUser {
                username,
                password_hash: &hash,
                role: Role::Owner.as_str(),
            })
            .await?
            .ok_or(AuthError::SetupAlreadyComplete)?;
        self.setup_complete.store(true, Ordering::Release);

        let user = AuthUser::try_from(created)?;
        self.audit(Some(&user.id), "auth.setup", Some(&user.id), client, None)
            .await;
        self.issue_session(user, client).await
    }

    /// Verifies a username and password and opens a session.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        client: ClientInfo<'_>,
    ) -> Result<IssuedSession> {
        let username = username.trim();
        if let Some(retry_after) = self.limiter.check(client.ip, username) {
            return Err(AuthError::RateLimited { retry_after });
        }
        if username.is_empty() || password.len() > password::MAX_PASSWORD_BYTES {
            self.limiter.record_failure(client.ip, username);
            return Err(AuthError::InvalidCredentials);
        }

        let user = self.store.find_user_by_username(username).await?;
        let password = password.to_owned();
        let stored_hash = user.as_ref().map(|u| u.password_hash.clone());
        let verified = tokio::task::spawn_blocking(move || match stored_hash {
            Some(hash) => password::verify_password(&password, &hash),
            None => {
                password::verify_dummy(&password);
                false
            }
        })
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

        let Some(user) = user.filter(|_| verified) else {
            self.limiter.record_failure(client.ip, username);
            let details = json!({ "username": username });
            self.audit(None, "auth.login_failed", None, client, Some(details))
                .await;
            return Err(AuthError::InvalidCredentials);
        };

        self.limiter.record_success(username);
        let mut user = AuthUser::try_from(user)?;
        let now = now_ms();
        self.store.touch_last_login(&user.id, now).await?;
        user.last_login_at = Some(now);
        self.audit(Some(&user.id), "auth.login", Some(&user.id), client, None)
            .await;
        self.issue_session(user, client).await
    }

    /// Resolves a session token to its user, renewing the session if it's
    /// past the halfway point of its lifetime.
    pub async fn authenticate(&self, token: &str) -> Result<Option<Authenticated>> {
        if token.is_empty() || token.len() > 128 {
            return Ok(None);
        }
        let now = now_ms();
        let Some((session, user)) = self
            .store
            .find_active_session(&token::hash_token(token), now)
            .await?
        else {
            return Ok(None);
        };

        let ttl = self.ttl_ms();
        let mut expires_at_ms = session.expires_at;
        let renewed = expires_at_ms - now < ttl / 2;
        if renewed {
            expires_at_ms = now + ttl;
            self.store
                .extend_session(&session.id, expires_at_ms)
                .await?;
        }
        Ok(Some(Authenticated {
            user: AuthUser::try_from(user)?,
            session_id: session.id,
            expires_at_ms,
            renewed,
        }))
    }

    /// Ends the session identified by `token`.
    pub async fn logout(&self, token: &str, client: ClientInfo<'_>) -> Result<()> {
        let hash = token::hash_token(token);
        let session = self.store.find_active_session(&hash, now_ms()).await?;
        self.store.delete_session_by_token_hash(&hash).await?;
        if let Some((_, user)) = session {
            self.audit(Some(&user.id), "auth.logout", Some(&user.id), client, None)
                .await;
        }
        Ok(())
    }

    /// Sets a new random password for an owner account and signs out all of
    /// its sessions. Used by the `reset-admin` CLI command.
    ///
    /// With no `username`, resets the first owner. Returns the user and the
    /// new plaintext password, which is shown once and never stored.
    pub async fn reset_owner_password(&self, username: Option<&str>) -> Result<(AuthUser, String)> {
        let user = match username {
            Some(name) => self.store.find_user_by_username(name).await?,
            None => self.store.find_first_owner().await?,
        }
        .ok_or(AuthError::UserNotFound)?;
        let user = AuthUser::try_from(user)?;

        let new_password = token::generate_password();
        let hash = hash_blocking(new_password.clone()).await?;
        self.store.set_password_hash(&user.id, &hash).await?;
        self.store.delete_sessions_for_user(&user.id).await?;
        self.limiter.record_success(&user.username);
        let details = json!({ "via": "cli" });
        self.audit(
            None,
            "auth.reset_password",
            Some(&user.id),
            ClientInfo::default(),
            Some(details),
        )
        .await;
        Ok((user, new_password))
    }

    /// Deletes expired sessions. Returns how many were removed.
    pub async fn prune_expired_sessions(&self) -> Result<u64> {
        Ok(self.store.delete_expired_sessions(now_ms()).await?)
    }

    async fn issue_session(&self, user: AuthUser, client: ClientInfo<'_>) -> Result<IssuedSession> {
        let token = token::generate_token();
        let expires_at_ms = now_ms() + self.ttl_ms();
        self.store
            .create_session(NewSession {
                user_id: &user.id,
                token_hash: &token::hash_token(&token),
                user_agent: client.user_agent.map(|ua| truncate(ua, 512)),
                ip: client.ip,
                expires_at: expires_at_ms,
            })
            .await?;
        Ok(IssuedSession {
            user,
            token,
            expires_at_ms,
        })
    }

    /// Writes an audit row. Failures are logged, not returned: an audit write
    /// problem shouldn't turn a successful login into an error.
    async fn audit(
        &self,
        user_id: Option<&str>,
        action: &str,
        target_user: Option<&str>,
        client: ClientInfo<'_>,
        details: Option<serde_json::Value>,
    ) {
        let mut details = details.unwrap_or_else(|| json!({}));
        if let (Some(ip), Some(map)) = (client.ip, details.as_object_mut()) {
            map.insert("ip".into(), ip.into());
        }
        let entry = NewAuditEntry {
            user_id,
            action,
            target_type: target_user.map(|_| "user"),
            target_id: target_user,
            details: Some(details),
        };
        if let Err(e) = self.store.insert_audit(entry).await {
            tracing::warn!(error = %e, action, "failed to write audit log entry");
        }
    }

    fn ttl_ms(&self) -> i64 {
        i64::try_from(self.config.session_ttl.as_millis()).unwrap_or(i64::MAX / 2)
    }
}

/// Checks username rules: 3–32 characters of ASCII letters, digits, `_`, `.`, or `-`.
/// Returns the trimmed username.
pub fn validate_username(username: &str) -> Result<&str> {
    let username = username.trim();
    let len = username.chars().count();
    if !(MIN_USERNAME_LEN..=MAX_USERNAME_LEN).contains(&len) {
        return Err(AuthError::InvalidInput(format!(
            "Username must be {MIN_USERNAME_LEN}–{MAX_USERNAME_LEN} characters."
        )));
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err(AuthError::InvalidInput(
            "Username may only contain letters, numbers, '_', '.', and '-'.".into(),
        ));
    }
    Ok(username)
}

async fn hash_blocking(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || password::hash_password(&password))
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?
        .map_err(|e| AuthError::Internal(e.to_string()))
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "correct horse battery";
    const CLIENT: ClientInfo<'static> = ClientInfo {
        ip: Some("127.0.0.1"),
        user_agent: Some("test"),
    };

    async fn auth() -> Auth {
        let store = Store::open_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        Auth::new(store, AuthConfig::default())
    }

    #[tokio::test]
    async fn setup_creates_owner_once() {
        let auth = auth().await;
        assert!(auth.needs_setup().await.unwrap());
        let issued = auth.setup("admin", PASSWORD, CLIENT).await.unwrap();
        assert_eq!(issued.user.role, Role::Owner);
        assert!(!auth.needs_setup().await.unwrap());
        assert!(matches!(
            auth.setup("other", PASSWORD, CLIENT).await,
            Err(AuthError::SetupAlreadyComplete)
        ));
        let actions: Vec<_> = auth
            .store()
            .recent_audit(10)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect();
        assert_eq!(actions, ["auth.setup"]);
    }

    #[tokio::test]
    async fn setup_validates_input() {
        let auth = auth().await;
        assert!(matches!(
            auth.setup("a", PASSWORD, CLIENT).await,
            Err(AuthError::InvalidInput(_))
        ));
        assert!(matches!(
            auth.setup("bad name!", PASSWORD, CLIENT).await,
            Err(AuthError::InvalidInput(_))
        ));
        assert!(matches!(
            auth.setup("admin", "short", CLIENT).await,
            Err(AuthError::InvalidInput(_))
        ));
        assert!(auth.needs_setup().await.unwrap());
    }

    #[tokio::test]
    async fn login_authenticate_logout() {
        let auth = auth().await;
        auth.setup("Admin", PASSWORD, CLIENT).await.unwrap();

        let issued = auth.login("admin", PASSWORD, CLIENT).await.unwrap();
        assert_eq!(issued.user.username, "Admin");
        assert!(issued.user.last_login_at.is_some());

        let authed = auth.authenticate(&issued.token).await.unwrap().unwrap();
        assert_eq!(authed.user.id, issued.user.id);
        assert!(!authed.renewed);

        auth.logout(&issued.token, CLIENT).await.unwrap();
        assert!(auth.authenticate(&issued.token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn wrong_password_and_unknown_user_are_indistinguishable() {
        let auth = auth().await;
        auth.setup("admin", PASSWORD, CLIENT).await.unwrap();
        let wrong = auth
            .login("admin", "wrong password!", CLIENT)
            .await
            .unwrap_err();
        let unknown = auth.login("nobody", PASSWORD, CLIENT).await.unwrap_err();
        assert_eq!(wrong.to_string(), unknown.to_string());
    }

    #[tokio::test]
    async fn repeated_failures_are_rate_limited() {
        let auth = auth().await;
        auth.setup("admin", PASSWORD, CLIENT).await.unwrap();
        for _ in 0..rate_limit::USERNAME_POLICY.free_attempts + 1 {
            let _ = auth.login("admin", "wrong password!", CLIENT).await;
        }
        assert!(matches!(
            auth.login("admin", PASSWORD, CLIENT).await,
            Err(AuthError::RateLimited { .. })
        ));
    }

    #[tokio::test]
    async fn sessions_renew_past_halfway() {
        let store = Store::open_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let auth = Auth::new(
            store,
            AuthConfig {
                session_ttl: Duration::from_secs(60),
            },
        );
        let issued = auth.setup("admin", PASSWORD, CLIENT).await.unwrap();
        // Pull the expiry close so the session is past its halfway point.
        let authed = auth.authenticate(&issued.token).await.unwrap().unwrap();
        auth.store()
            .extend_session(&authed.session_id, now_ms() + 10_000)
            .await
            .unwrap();
        let renewed = auth.authenticate(&issued.token).await.unwrap().unwrap();
        assert!(renewed.renewed);
        assert!(renewed.expires_at_ms > now_ms() + 50_000);
    }

    #[tokio::test]
    async fn garbage_tokens_do_not_authenticate() {
        let auth = auth().await;
        assert!(auth.authenticate("").await.unwrap().is_none());
        assert!(auth.authenticate(&"x".repeat(500)).await.unwrap().is_none());
        assert!(auth.authenticate("not-a-token").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reset_owner_password_revokes_sessions() {
        let auth = auth().await;
        let issued = auth.setup("admin", PASSWORD, CLIENT).await.unwrap();
        let (user, new_password) = auth.reset_owner_password(None).await.unwrap();
        assert_eq!(user.username, "admin");
        assert!(auth.authenticate(&issued.token).await.unwrap().is_none());
        assert!(auth.login("admin", PASSWORD, CLIENT).await.is_err());
        assert!(auth.login("admin", &new_password, CLIENT).await.is_ok());
        assert!(matches!(
            auth.reset_owner_password(Some("ghost")).await,
            Err(AuthError::UserNotFound)
        ));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("héllo", 2), "hé");
        assert_eq!(truncate("hi", 10), "hi");
    }
}
