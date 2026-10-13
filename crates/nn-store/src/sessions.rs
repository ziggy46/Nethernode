//! Login sessions.
//!
//! Only the SHA-256 hash of a session token is stored, so a leaked database
//! can't be used to hijack sessions.

use crate::{Result, Store, User, new_id, now_ms};

/// A login session.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Session {
    /// ULID.
    pub id: String,
    /// Owning user.
    pub user_id: String,
    /// Hex-encoded SHA-256 of the session token.
    pub token_hash: String,
    /// `User-Agent` at login.
    pub user_agent: Option<String>,
    /// Client IP at login.
    pub ip: Option<String>,
    /// Creation time, unix ms.
    pub created_at: i64,
    /// Expiry time, unix ms.
    pub expires_at: i64,
}

/// Fields needed to create a session.
#[derive(Debug, Clone)]
pub struct NewSession<'a> {
    /// Owning user.
    pub user_id: &'a str,
    /// Hex-encoded SHA-256 of the session token.
    pub token_hash: &'a str,
    /// `User-Agent` at login.
    pub user_agent: Option<&'a str>,
    /// Client IP at login.
    pub ip: Option<&'a str>,
    /// Expiry time, unix ms.
    pub expires_at: i64,
}

struct SessionUserRow {
    session_id: String,
    user_id: String,
    token_hash: String,
    user_agent: Option<String>,
    ip: Option<String>,
    session_created_at: i64,
    expires_at: i64,
    username: String,
    password_hash: String,
    role: String,
    totp_secret: Option<String>,
    user_created_at: i64,
    last_login_at: Option<i64>,
}

impl Store {
    /// Creates a session.
    pub async fn create_session(&self, session: NewSession<'_>) -> Result<Session> {
        let id = new_id();
        let now = now_ms();
        let created = sqlx::query_as!(
            Session,
            r#"INSERT INTO sessions (id, user_id, token_hash, user_agent, ip, created_at, expires_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               RETURNING id AS "id!", user_id, token_hash, user_agent, ip, created_at, expires_at"#,
            id,
            session.user_id,
            session.token_hash,
            session.user_agent,
            session.ip,
            now,
            session.expires_at,
        )
        .fetch_one(self.pool())
        .await?;
        Ok(created)
    }

    /// Finds an unexpired session by token hash, together with its user.
    pub async fn find_active_session(
        &self,
        token_hash: &str,
        now_ms: i64,
    ) -> Result<Option<(Session, User)>> {
        let row = sqlx::query_as!(
            SessionUserRow,
            r#"SELECT s.id AS "session_id!", s.user_id, s.token_hash, s.user_agent, s.ip,
                      s.created_at AS session_created_at, s.expires_at,
                      u.username, u.password_hash, u.role, u.totp_secret,
                      u.created_at AS user_created_at, u.last_login_at
               FROM sessions s JOIN users u ON u.id = s.user_id
               WHERE s.token_hash = ?1 AND s.expires_at > ?2"#,
            token_hash,
            now_ms,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| {
            let session = Session {
                id: r.session_id,
                user_id: r.user_id.clone(),
                token_hash: r.token_hash,
                user_agent: r.user_agent,
                ip: r.ip,
                created_at: r.session_created_at,
                expires_at: r.expires_at,
            };
            let user = User {
                id: r.user_id,
                username: r.username,
                password_hash: r.password_hash,
                role: r.role,
                totp_secret: r.totp_secret,
                created_at: r.user_created_at,
                last_login_at: r.last_login_at,
            };
            (session, user)
        }))
    }

    /// Extends a session's expiry (sliding expiration).
    pub async fn extend_session(&self, session_id: &str, expires_at: i64) -> Result<()> {
        sqlx::query!(
            "UPDATE sessions SET expires_at = ?1 WHERE id = ?2",
            expires_at,
            session_id,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Deletes the session with this token hash. Returns `false` if none matched.
    pub async fn delete_session_by_token_hash(&self, token_hash: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM sessions WHERE token_hash = ?1", token_hash)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Deletes every session belonging to a user. Returns the number removed.
    pub async fn delete_sessions_for_user(&self, user_id: &str) -> Result<u64> {
        let result = sqlx::query!("DELETE FROM sessions WHERE user_id = ?1", user_id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected())
    }

    /// Deletes sessions that expired at or before `now_ms`. Returns the number removed.
    pub async fn delete_expired_sessions(&self, now_ms: i64) -> Result<u64> {
        let result = sqlx::query!("DELETE FROM sessions WHERE expires_at <= ?1", now_ms)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use crate::{NewUser, test_store};

    use super::*;

    async fn user(store: &Store) -> User {
        store
            .create_user(NewUser { username: "alpha", password_hash: "h", role: "owner" })
            .await
            .unwrap()
    }

    fn new_session<'a>(user_id: &'a str, hash: &'a str, expires_at: i64) -> NewSession<'a> {
        NewSession { user_id, token_hash: hash, user_agent: Some("ua"), ip: None, expires_at }
    }

    #[tokio::test]
    async fn active_sessions_resolve_and_expired_do_not() {
        let store = test_store().await;
        let user = user(&store).await;
        store.create_session(new_session(&user.id, "live", 2_000)).await.unwrap();
        store.create_session(new_session(&user.id, "dead", 500)).await.unwrap();

        let (session, found) = store.find_active_session("live", 1_000).await.unwrap().unwrap();
        assert_eq!(found.id, user.id);
        assert_eq!(session.user_agent.as_deref(), Some("ua"));
        assert!(store.find_active_session("dead", 1_000).await.unwrap().is_none());
        assert!(store.find_active_session("nope", 1_000).await.unwrap().is_none());

        assert_eq!(store.delete_expired_sessions(1_000).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn sessions_can_be_extended_and_deleted() {
        let store = test_store().await;
        let user = user(&store).await;
        let session = store.create_session(new_session(&user.id, "t", 1_000)).await.unwrap();
        store.extend_session(&session.id, 5_000).await.unwrap();
        assert!(store.find_active_session("t", 4_000).await.unwrap().is_some());

        assert!(store.delete_session_by_token_hash("t").await.unwrap());
        assert!(!store.delete_session_by_token_hash("t").await.unwrap());

        store.create_session(new_session(&user.id, "a", 9_000)).await.unwrap();
        store.create_session(new_session(&user.id, "b", 9_000)).await.unwrap();
        assert_eq!(store.delete_sessions_for_user(&user.id).await.unwrap(), 2);
    }
}
