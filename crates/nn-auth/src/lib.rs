//! Authentication and authorization for the NetherNode panel.
//!
//! [`Auth`] covers first-run setup, password login with rate limiting,
//! session issuance and validation, logout, and owner password recovery.
//! It knows nothing about HTTP; `nn-api` maps it onto cookies and RPCs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nn_store::{NewAuditEntry, NewSession, NewUser, Store, now_ms};
use serde_json::json;

pub mod password;
pub mod rate_limit;
pub mod rbac;
pub mod secrets;
pub mod token;
pub mod totp;

pub use rate_limit::LoginRateLimiter;
pub use rbac::{Permission, Role};
pub use secrets::SecretBox;

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
    /// The change would leave the panel without an owner.
    #[error("there must always be at least one owner")]
    LastOwner,
    /// The username is taken.
    #[error("that username is taken")]
    UsernameTaken,
    /// Wrong or expired two-factor code, or an expired sign-in attempt.
    #[error("{0}")]
    SecondFactor(&'static str),
    /// Two-factor sign-in needs the data key, which couldn't be loaded.
    #[error("two-factor sign-in isn't available because the data key couldn't be loaded")]
    NoDataKey,
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
    /// Must set a new password before doing anything else.
    pub must_change_password: bool,
    /// Signs in with an authenticator code as well as a password.
    pub totp_enabled: bool,
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
            must_change_password: user.must_change_password,
            totp_enabled: user.totp_secret.is_some(),
        })
    }
}

/// The outcome of a correct password.
#[derive(Debug, Clone)]
pub enum SignIn {
    /// Signed in.
    Session(IssuedSession),
    /// The account uses two-factor sign-in: finish with
    /// [`Auth::finish_sign_in`] and an authenticator or recovery code.
    SecondFactor {
        /// Identifies this sign-in attempt; not a session.
        challenge: String,
    },
}

impl SignIn {
    /// The session, if no second factor was needed.
    pub fn session(self) -> Option<IssuedSession> {
        match self {
            SignIn::Session(issued) => Some(issued),
            SignIn::SecondFactor { .. } => None,
        }
    }
}

/// What an authenticator app needs to enroll.
#[derive(Debug, Clone)]
pub struct TotpEnrollment {
    /// Base32 secret, for typing in by hand.
    pub secret: String,
    /// `otpauth://` link.
    pub uri: String,
    /// The link as a QR code (SVG).
    pub qr_svg: String,
}

/// How long a password-verified sign-in waits for its second factor.
const CHALLENGE_TTL_MS: i64 = 5 * 60 * 1000;
/// Wrong codes allowed per sign-in attempt.
const CHALLENGE_ATTEMPTS: u32 = 5;

#[derive(Debug)]
struct Challenge {
    user_id: String,
    username: String,
    expires_at: i64,
    attempts: u32,
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
    secrets: Option<Arc<SecretBox>>,
    /// Sign-ins waiting for a second factor, by challenge hash.
    challenges: Mutex<HashMap<String, Challenge>>,
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
            secrets: None,
            challenges: Mutex::new(HashMap::new()),
        }
    }

    /// Uses `secrets` to seal two-factor secrets. Without it, two-factor
    /// sign-in can't be turned on (or used).
    pub fn with_secrets(mut self, secrets: Option<Arc<SecretBox>>) -> Self {
        self.secrets = secrets;
        self
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
                must_change_password: false,
            })
            .await?
            .ok_or(AuthError::SetupAlreadyComplete)?;
        self.setup_complete.store(true, Ordering::Release);

        let user = AuthUser::try_from(created)?;
        self.audit(Some(&user.id), "auth.setup", Some(&user.id), client, None)
            .await;
        self.issue_session(user, client).await
    }

    /// Verifies a username and password. Opens a session, or asks for a
    /// second factor when the account has one.
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        client: ClientInfo<'_>,
    ) -> Result<SignIn> {
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

        let user = AuthUser::try_from(user)?;
        if user.totp_enabled {
            // Password right; the rate limit stays until the code is too.
            let challenge = token::generate_token();
            let mut challenges = self.challenges.lock().expect("challenge lock");
            let now = now_ms();
            challenges.retain(|_, c| c.expires_at > now);
            challenges.insert(
                token::hash_token(&challenge),
                Challenge {
                    user_id: user.id.clone(),
                    username: user.username.clone(),
                    expires_at: now + CHALLENGE_TTL_MS,
                    attempts: 0,
                },
            );
            return Ok(SignIn::SecondFactor { challenge });
        }
        self.limiter.record_success(username);
        self.complete_sign_in(user, client, None)
            .await
            .map(SignIn::Session)
    }

    /// Finishes a two-factor sign-in with an authenticator or recovery code.
    pub async fn finish_sign_in(
        &self,
        challenge: &str,
        code: &str,
        client: ClientInfo<'_>,
    ) -> Result<IssuedSession> {
        const EXPIRED: AuthError =
            AuthError::SecondFactor("This sign-in expired. Enter your password again.");
        let key = token::hash_token(challenge);
        let (user_id, username) = {
            let challenges = self.challenges.lock().expect("challenge lock");
            match challenges.get(&key) {
                Some(c) if c.expires_at > now_ms() => (c.user_id.clone(), c.username.clone()),
                _ => return Err(EXPIRED),
            }
        };
        if let Some(retry_after) = self.limiter.check(client.ip, &username) {
            return Err(AuthError::RateLimited { retry_after });
        }
        let user = self.store.find_user_by_id(&user_id).await?.ok_or(EXPIRED)?;
        let via = self.check_second_factor(&user, code).await?;
        let Some(via) = via else {
            self.limiter.record_failure(client.ip, &username);
            self.count_attempt(&key);
            let details = json!({ "username": username, "second_factor": true });
            self.audit(
                Some(&user_id),
                "auth.login_failed",
                Some(&user_id),
                client,
                Some(details),
            )
            .await;
            return Err(AuthError::SecondFactor("That code isn't right."));
        };
        self.challenges.lock().expect("challenge lock").remove(&key);
        self.limiter.record_success(&username);
        let user = AuthUser::try_from(user)?;
        self.complete_sign_in(user, client, Some(via)).await
    }

    /// Counts a wrong code; too many and the sign-in attempt is over.
    fn count_attempt(&self, key: &str) {
        let mut challenges = self.challenges.lock().expect("challenge lock");
        if let Some(c) = challenges.get_mut(key) {
            c.attempts += 1;
            if c.attempts >= CHALLENGE_ATTEMPTS {
                challenges.remove(key);
            }
        }
    }

    /// Checks an authenticator code (once per time step) or an unused
    /// recovery code. Returns how it matched, or `None`.
    async fn check_second_factor(
        &self,
        user: &nn_store::User,
        code: &str,
    ) -> Result<Option<&'static str>> {
        if totp::is_recovery_code(code) {
            let used = self
                .store
                .use_recovery_code(&user.id, &totp::hash_recovery_code(code))
                .await?;
            return Ok(used.then_some("recovery_code"));
        }
        let Some(sealed) = &user.totp_secret else {
            return Ok(None);
        };
        let secret = self.open(sealed)?;
        let Some(step) = totp::verify(&secret, code, now_ms() / 1000) else {
            return Ok(None);
        };
        // A code works once: an intercepted one can't be replayed.
        let fresh = self.store.advance_totp_step(&user.id, step).await?;
        Ok(fresh.then_some("authenticator"))
    }

    async fn complete_sign_in(
        &self,
        mut user: AuthUser,
        client: ClientInfo<'_>,
        second_factor: Option<&str>,
    ) -> Result<IssuedSession> {
        let now = now_ms();
        self.store.touch_last_login(&user.id, now).await?;
        user.last_login_at = Some(now);
        let details = second_factor.map(|via| json!({ "second_factor": via }));
        self.audit(
            Some(&user.id),
            "auth.login",
            Some(&user.id),
            client,
            details,
        )
        .await;
        self.issue_session(user, client).await
    }

    /// Whether two-factor sign-in can be turned on (the data key loaded).
    pub fn two_factor_available(&self) -> bool {
        self.secrets.is_some()
    }

    fn sealer(&self) -> Result<&SecretBox> {
        self.secrets.as_deref().ok_or(AuthError::NoDataKey)
    }

    fn open(&self, sealed: &str) -> Result<String> {
        self.sealer()?
            .open(sealed)
            .map_err(|e| AuthError::Internal(format!("couldn't open a two-factor secret: {e}")))
    }

    /// Verifies `password` for a signed-in user (before sensitive changes).
    async fn check_password(&self, user_id: &str, password: &str) -> Result<nn_store::User> {
        let user = self
            .store
            .find_user_by_id(user_id)
            .await?
            .ok_or(AuthError::UserNotFound)?;
        if let Some(retry_after) = self.limiter.check(None, &user.username) {
            return Err(AuthError::RateLimited { retry_after });
        }
        let (password, hash) = (password.to_owned(), user.password_hash.clone());
        let ok = tokio::task::spawn_blocking(move || password::verify_password(&password, &hash))
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        if !ok {
            self.limiter.record_failure(None, &user.username);
            return Err(AuthError::InvalidInput(
                "Your current password isn't right.".into(),
            ));
        }
        Ok(user)
    }

    /// Changes a user's own password and signs out their other sessions.
    pub async fn change_password(
        &self,
        user_id: &str,
        session_id: &str,
        current: &str,
        new: &str,
        client: ClientInfo<'_>,
    ) -> Result<()> {
        self.check_password(user_id, current).await?;
        password::validate_password(new).map_err(AuthError::InvalidInput)?;
        if current == new {
            return Err(AuthError::InvalidInput(
                "Choose a password different from the current one.".into(),
            ));
        }
        let hash = hash_blocking(new.to_owned()).await?;
        self.store.set_password(user_id, &hash, false).await?;
        let signed_out = self
            .store
            .delete_other_sessions(user_id, session_id)
            .await?;
        self.audit(
            Some(user_id),
            "auth.change_password",
            Some(user_id),
            client,
            Some(json!({ "other_sessions_ended": signed_out })),
        )
        .await;
        Ok(())
    }

    /// Starts two-factor enrollment: a new secret to scan, held until
    /// [`Auth::confirm_totp`] proves the app has it.
    pub async fn begin_totp(&self, user: &AuthUser) -> Result<TotpEnrollment> {
        let secret = totp::generate_secret();
        let sealed = self.sealer()?.seal(&secret);
        self.store.set_totp_pending(&user.id, Some(&sealed)).await?;
        let uri = totp::otpauth_uri(&secret, &user.username);
        let qr_svg = totp::qr_svg(&uri)
            .ok_or_else(|| AuthError::Internal("couldn't draw the QR code".into()))?;
        Ok(TotpEnrollment {
            secret,
            uri,
            qr_svg,
        })
    }

    /// Turns two-factor sign-in on once `code` matches the pending secret.
    /// Returns recovery codes, shown once.
    pub async fn confirm_totp(
        &self,
        user_id: &str,
        code: &str,
        client: ClientInfo<'_>,
    ) -> Result<Vec<String>> {
        let user = self
            .store
            .find_user_by_id(user_id)
            .await?
            .ok_or(AuthError::UserNotFound)?;
        let sealed = user.totp_pending.as_deref().ok_or(AuthError::SecondFactor(
            "Start setting up two-factor sign-in again.",
        ))?;
        let secret = self.open(sealed)?;
        let step = totp::verify(&secret, code, now_ms() / 1000).ok_or(AuthError::SecondFactor(
            "That code isn't right. Check the time on your phone and try the newest code.",
        ))?;
        let codes = totp::generate_recovery_codes();
        let hashes: Vec<String> = codes.iter().map(|c| totp::hash_recovery_code(c)).collect();
        self.store
            .enable_totp(user_id, sealed, step, &hashes)
            .await?;
        self.audit(
            Some(user_id),
            "auth.totp_enabled",
            Some(user_id),
            client,
            None,
        )
        .await;
        Ok(codes)
    }

    /// Turns a user's own two-factor sign-in off, after checking their password.
    pub async fn disable_totp(
        &self,
        user_id: &str,
        password: &str,
        client: ClientInfo<'_>,
    ) -> Result<()> {
        self.check_password(user_id, password).await?;
        self.store.disable_totp(user_id).await?;
        self.audit(
            Some(user_id),
            "auth.totp_disabled",
            Some(user_id),
            client,
            None,
        )
        .await;
        Ok(())
    }

    /// Unused recovery codes left.
    pub async fn recovery_codes_left(&self, user_id: &str) -> Result<i64> {
        Ok(self.store.recovery_codes_left(user_id).await?)
    }

    /// Creates a user with a one-time password they must change. Returns
    /// the user and that password, which is shown once.
    pub async fn create_user(
        &self,
        actor: &AuthUser,
        username: &str,
        role: Role,
        client: ClientInfo<'_>,
    ) -> Result<(AuthUser, String)> {
        let username = validate_username(username)?;
        if self.store.find_user_by_username(username).await?.is_some() {
            return Err(AuthError::UsernameTaken);
        }
        let password = token::generate_password();
        let hash = hash_blocking(password.clone()).await?;
        let created = self
            .store
            .create_user(NewUser {
                username,
                password_hash: &hash,
                role: role.as_str(),
                must_change_password: true,
            })
            .await
            .map_err(|e| {
                if e.is_unique_violation() {
                    AuthError::UsernameTaken
                } else {
                    AuthError::Store(e)
                }
            })?;
        let user = AuthUser::try_from(created)?;
        let details = json!({ "username": user.username, "role": role.as_str() });
        self.audit(
            Some(&actor.id),
            "user.create",
            Some(&user.id),
            client,
            Some(details),
        )
        .await;
        Ok((user, password))
    }

    /// Changes a user's role, keeping at least one owner.
    pub async fn set_role(
        &self,
        actor: &AuthUser,
        user_id: &str,
        role: Role,
        client: ClientInfo<'_>,
    ) -> Result<AuthUser> {
        let before = self.load_user(user_id).await?;
        if !self.store.set_user_role(user_id, role.as_str()).await? {
            return Err(AuthError::LastOwner);
        }
        let details = json!({ "username": before.username, "from": before.role.as_str(), "to": role.as_str() });
        self.audit(
            Some(&actor.id),
            "user.role",
            Some(user_id),
            client,
            Some(details),
        )
        .await;
        self.load_user(user_id).await
    }

    /// Sets a new one-time password for a user and signs them out
    /// everywhere. Returns the password, shown once.
    pub async fn reset_password(
        &self,
        actor: &AuthUser,
        user_id: &str,
        client: ClientInfo<'_>,
    ) -> Result<String> {
        let user = self.load_user(user_id).await?;
        let password = token::generate_password();
        let hash = hash_blocking(password.clone()).await?;
        self.store.set_password(user_id, &hash, true).await?;
        self.store.delete_sessions_for_user(user_id).await?;
        self.limiter.record_success(&user.username);
        let details = json!({ "username": user.username });
        self.audit(
            Some(&actor.id),
            "user.reset_password",
            Some(user_id),
            client,
            Some(details),
        )
        .await;
        Ok(password)
    }

    /// Turns off another user's two-factor sign-in (a lost phone).
    pub async fn reset_totp(
        &self,
        actor: &AuthUser,
        user_id: &str,
        client: ClientInfo<'_>,
    ) -> Result<()> {
        let user = self.load_user(user_id).await?;
        self.store.disable_totp(user_id).await?;
        let details = json!({ "username": user.username });
        self.audit(
            Some(&actor.id),
            "user.reset_totp",
            Some(user_id),
            client,
            Some(details),
        )
        .await;
        Ok(())
    }

    /// Signs a user out everywhere. Returns how many sessions ended.
    pub async fn revoke_sessions(
        &self,
        actor: &AuthUser,
        user_id: &str,
        client: ClientInfo<'_>,
    ) -> Result<u64> {
        let user = self.load_user(user_id).await?;
        let ended = self.store.delete_sessions_for_user(user_id).await?;
        let details = json!({ "username": user.username, "sessions": ended });
        self.audit(
            Some(&actor.id),
            "user.revoke_sessions",
            Some(user_id),
            client,
            Some(details),
        )
        .await;
        Ok(ended)
    }

    /// Deletes a user, keeping at least one owner. Nobody deletes themselves.
    pub async fn delete_user(
        &self,
        actor: &AuthUser,
        user_id: &str,
        client: ClientInfo<'_>,
    ) -> Result<()> {
        if actor.id == user_id {
            return Err(AuthError::InvalidInput(
                "You can't delete your own account.".into(),
            ));
        }
        let user = self.load_user(user_id).await?;
        if !self.store.delete_user(user_id).await? {
            return Err(AuthError::LastOwner);
        }
        let details = json!({ "username": user.username, "role": user.role.as_str() });
        self.audit(
            Some(&actor.id),
            "user.delete",
            Some(user_id),
            client,
            Some(details),
        )
        .await;
        Ok(())
    }

    async fn load_user(&self, user_id: &str) -> Result<AuthUser> {
        let user = self
            .store
            .find_user_by_id(user_id)
            .await?
            .ok_or(AuthError::UserNotFound)?;
        AuthUser::try_from(user)
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

    /// Sets a new random password for an owner account, turns off its
    /// two-factor sign-in (the phone may be what was lost), and signs out
    /// all of its sessions. Used by the `reset-admin` CLI command, which
    /// needs access to the host. The password must be changed at sign-in.
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
        self.store.set_password(&user.id, &hash, true).await?;
        self.store.disable_totp(&user.id).await?;
        self.store.delete_sessions_for_user(&user.id).await?;
        self.limiter.record_success(&user.username);
        let details = json!({ "via": "cli", "two_factor_was_on": user.totp_enabled });
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

        let issued = auth
            .login("admin", PASSWORD, CLIENT)
            .await
            .unwrap()
            .session()
            .unwrap();
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

    async fn auth_with_key() -> Auth {
        let key = hex::encode([7u8; 32]);
        auth()
            .await
            .with_secrets(Some(Arc::new(SecretBox::from_hex_key(&key).unwrap())))
    }

    fn current_code(secret: &str) -> String {
        let key = totp::decode_secret(secret).unwrap();
        let code = totp::code_at(&key, totp::step_at(now_ms() / 1000), totp::DIGITS);
        format!("{code:06}")
    }

    #[tokio::test]
    async fn two_factor_sign_in() {
        let auth = auth_with_key().await;
        let owner = auth.setup("admin", PASSWORD, CLIENT).await.unwrap().user;
        let enrollment = auth.begin_totp(&owner).await.unwrap();
        assert!(enrollment.qr_svg.contains("<svg"));
        assert!(matches!(
            auth.confirm_totp(&owner.id, "000000", CLIENT).await,
            Err(AuthError::SecondFactor(_))
        ));
        let code = current_code(&enrollment.secret);
        let recovery = auth.confirm_totp(&owner.id, &code, CLIENT).await.unwrap();
        assert_eq!(recovery.len(), totp::RECOVERY_CODES);
        // The stored secret is sealed, not the plain base32.
        let stored = auth
            .store()
            .find_user_by_id(&owner.id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            stored.totp_secret.as_deref(),
            Some(enrollment.secret.as_str())
        );

        // The password alone no longer signs in.
        let SignIn::SecondFactor { challenge } =
            auth.login("admin", PASSWORD, CLIENT).await.unwrap()
        else {
            panic!("expected a second-factor challenge");
        };
        // The enrollment code was already used for this time step.
        assert!(
            auth.finish_sign_in(&challenge, &code, CLIENT)
                .await
                .is_err()
        );
        assert!(
            auth.finish_sign_in(&challenge, "123456", CLIENT)
                .await
                .is_err()
        );
        let issued = auth
            .finish_sign_in(&challenge, &recovery[0], CLIENT)
            .await
            .unwrap();
        assert!(issued.user.totp_enabled);
        // The challenge is spent, and so is the recovery code.
        assert!(
            auth.finish_sign_in(&challenge, &recovery[1], CLIENT)
                .await
                .is_err()
        );
        let SignIn::SecondFactor { challenge } =
            auth.login("admin", PASSWORD, CLIENT).await.unwrap()
        else {
            panic!("expected a second-factor challenge");
        };
        assert!(
            auth.finish_sign_in(&challenge, &recovery[0], CLIENT)
                .await
                .is_err()
        );
        assert_eq!(auth.recovery_codes_left(&owner.id).await.unwrap(), 9);

        auth.disable_totp(&owner.id, PASSWORD, CLIENT)
            .await
            .unwrap();
        assert!(
            auth.login("admin", PASSWORD, CLIENT)
                .await
                .unwrap()
                .session()
                .is_some()
        );
    }

    #[tokio::test]
    async fn sign_in_challenges_allow_few_attempts() {
        let auth = auth_with_key().await;
        let owner = auth.setup("admin", PASSWORD, CLIENT).await.unwrap().user;
        let enrollment = auth.begin_totp(&owner).await.unwrap();
        auth.confirm_totp(&owner.id, &current_code(&enrollment.secret), CLIENT)
            .await
            .unwrap();
        let SignIn::SecondFactor { challenge } =
            auth.login("admin", PASSWORD, CLIENT).await.unwrap()
        else {
            panic!("expected a second-factor challenge");
        };
        for _ in 0..CHALLENGE_ATTEMPTS {
            assert!(
                auth.finish_sign_in(&challenge, "000001", CLIENT)
                    .await
                    .is_err()
            );
        }
        let err = auth
            .finish_sign_in(&challenge, "000002", CLIENT)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("expired") || matches!(err, AuthError::RateLimited { .. })
        );
    }

    #[tokio::test]
    async fn the_command_line_reset_turns_two_factor_off() {
        let auth = auth_with_key().await;
        let owner = auth.setup("admin", PASSWORD, CLIENT).await.unwrap().user;
        let enrollment = auth.begin_totp(&owner).await.unwrap();
        auth.confirm_totp(&owner.id, &current_code(&enrollment.secret), CLIENT)
            .await
            .unwrap();
        let (user, password) = auth.reset_owner_password(None).await.unwrap();
        assert!(user.totp_enabled, "reports what it was before");
        let issued = auth
            .login("admin", &password, CLIENT)
            .await
            .unwrap()
            .session()
            .expect("no second factor after a reset");
        assert!(issued.user.must_change_password && !issued.user.totp_enabled);
    }

    #[tokio::test]
    async fn two_factor_needs_the_data_key() {
        let auth = auth().await;
        let owner = auth.setup("admin", PASSWORD, CLIENT).await.unwrap().user;
        assert!(matches!(
            auth.begin_totp(&owner).await,
            Err(AuthError::NoDataKey)
        ));
    }

    #[tokio::test]
    async fn managed_users_change_their_one_time_password() {
        let auth = auth().await;
        let owner = auth.setup("admin", PASSWORD, CLIENT).await.unwrap().user;
        let (user, temporary) = auth
            .create_user(&owner, "builder", Role::Moderator, CLIENT)
            .await
            .unwrap();
        assert!(user.must_change_password);
        assert!(matches!(
            auth.create_user(&owner, "BUILDER", Role::Viewer, CLIENT)
                .await,
            Err(AuthError::UsernameTaken)
        ));
        let issued = auth
            .login("builder", &temporary, CLIENT)
            .await
            .unwrap()
            .session()
            .unwrap();
        assert!(issued.user.must_change_password);
        let session = auth.authenticate(&issued.token).await.unwrap().unwrap();
        let other = auth
            .login("builder", &temporary, CLIENT)
            .await
            .unwrap()
            .session()
            .unwrap();
        assert!(matches!(
            auth.change_password(
                &user.id,
                &session.session_id,
                "wrong!!!!!",
                PASSWORD,
                CLIENT
            )
            .await,
            Err(AuthError::InvalidInput(_))
        ));
        auth.change_password(&user.id, &session.session_id, &temporary, PASSWORD, CLIENT)
            .await
            .unwrap();
        let after = auth.authenticate(&issued.token).await.unwrap().unwrap();
        assert!(!after.user.must_change_password);
        assert!(
            auth.authenticate(&other.token).await.unwrap().is_none(),
            "other sessions end"
        );

        // An owner's reset forces another change and signs them out.
        let reset = auth.reset_password(&owner, &user.id, CLIENT).await.unwrap();
        assert!(auth.authenticate(&issued.token).await.unwrap().is_none());
        let again = auth
            .login("builder", &reset, CLIENT)
            .await
            .unwrap()
            .session()
            .unwrap();
        assert!(again.user.must_change_password);
    }

    #[tokio::test]
    async fn owners_keep_one_owner() {
        let auth = auth().await;
        let owner = auth.setup("admin", PASSWORD, CLIENT).await.unwrap().user;
        assert!(matches!(
            auth.set_role(&owner, &owner.id, Role::Admin, CLIENT).await,
            Err(AuthError::LastOwner)
        ));
        assert!(matches!(
            auth.delete_user(&owner, &owner.id, CLIENT).await,
            Err(AuthError::InvalidInput(_))
        ));
        let (second, _) = auth
            .create_user(&owner, "second", Role::Owner, CLIENT)
            .await
            .unwrap();
        auth.set_role(&second, &owner.id, Role::Admin, CLIENT)
            .await
            .unwrap();
        assert!(matches!(
            auth.delete_user(&owner, &second.id, CLIENT).await,
            Err(AuthError::LastOwner)
        ));
        let actions: Vec<_> = auth
            .store()
            .recent_audit(10)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect();
        assert!(
            actions.contains(&"user.create".to_owned())
                && actions.contains(&"user.role".to_owned())
        );
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("héllo", 2), "hé");
        assert_eq!(truncate("hi", 10), "hi");
    }
}
