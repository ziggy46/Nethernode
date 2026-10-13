//! Panel user accounts.

use std::fmt;

use crate::{Result, Store, new_id, now_ms};

/// A panel user.
#[derive(Clone, sqlx::FromRow)]
pub struct User {
    /// ULID.
    pub id: String,
    /// Unique, case-insensitive login name.
    pub username: String,
    /// Argon2id PHC string.
    pub password_hash: String,
    /// RBAC role name (`owner`, `admin`, `moderator`, `viewer`).
    pub role: String,
    /// Base32 TOTP secret, when 2FA is enrolled.
    pub totp_secret: Option<String>,
    /// Creation time, unix ms.
    pub created_at: i64,
    /// Last successful login, unix ms.
    pub last_login_at: Option<i64>,
}

// Hand-written so password hashes and TOTP secrets never reach logs.
impl fmt::Debug for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("role", &self.role)
            .field("created_at", &self.created_at)
            .field("last_login_at", &self.last_login_at)
            .finish_non_exhaustive()
    }
}

/// Fields needed to create a user.
#[derive(Clone)]
pub struct NewUser<'a> {
    /// Login name.
    pub username: &'a str,
    /// Argon2id PHC string.
    pub password_hash: &'a str,
    /// RBAC role name.
    pub role: &'a str,
}

impl Store {
    /// Number of user accounts. Zero means first-run setup has not happened.
    pub async fn count_users(&self) -> Result<i64> {
        let count = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "count!: i64" FROM users"#)
            .fetch_one(self.pool())
            .await?;
        Ok(count)
    }

    /// Creates `user` only if no users exist yet.
    ///
    /// The existence check and insert are one statement, so two concurrent
    /// setup requests cannot both create an account. Returns `None` if a user
    /// already existed.
    pub async fn create_first_user(&self, user: NewUser<'_>) -> Result<Option<User>> {
        let id = new_id();
        let now = now_ms();
        let result = sqlx::query!(
            r#"INSERT INTO users (id, username, password_hash, role, created_at)
               SELECT ?1, ?2, ?3, ?4, ?5
               WHERE NOT EXISTS (SELECT 1 FROM users)"#,
            id,
            user.username,
            user.password_hash,
            user.role,
            now,
        )
        .execute(self.pool())
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_by_id(&id).await
    }

    /// Creates a user. Fails with a unique-constraint error if the username is taken.
    pub async fn create_user(&self, user: NewUser<'_>) -> Result<User> {
        let id = new_id();
        let now = now_ms();
        let created = sqlx::query_as!(
            User,
            r#"INSERT INTO users (id, username, password_hash, role, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5)
               RETURNING id AS "id!", username, password_hash, role, totp_secret, created_at,
                         last_login_at"#,
            id,
            user.username,
            user.password_hash,
            user.role,
            now,
        )
        .fetch_one(self.pool())
        .await?;
        Ok(created)
    }

    /// Looks up a user by ID.
    pub async fn find_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, created_at,
                      last_login_at
               FROM users WHERE id = ?1"#,
            id,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// Looks up a user by username, ignoring case.
    pub async fn find_user_by_username(&self, username: &str) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, created_at,
                      last_login_at
               FROM users WHERE username = ?1"#,
            username,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// The oldest account with the `owner` role, if any.
    pub async fn find_first_owner(&self) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, created_at,
                      last_login_at
               FROM users WHERE role = 'owner'
               ORDER BY created_at, id LIMIT 1"#,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(user)
    }

    /// Replaces a user's password hash. Returns `false` if the user doesn't exist.
    pub async fn set_password_hash(&self, user_id: &str, password_hash: &str) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE users SET password_hash = ?1 WHERE id = ?2",
            password_hash,
            user_id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Records a successful login.
    pub async fn touch_last_login(&self, user_id: &str, at_ms: i64) -> Result<()> {
        sqlx::query!(
            "UPDATE users SET last_login_at = ?1 WHERE id = ?2",
            at_ms,
            user_id,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_store;

    use super::*;

    fn owner(name: &str) -> NewUser<'_> {
        NewUser {
            username: name,
            password_hash: "hash",
            role: "owner",
        }
    }

    #[tokio::test]
    async fn first_user_is_created_only_once() {
        let store = test_store().await;
        assert_eq!(store.count_users().await.unwrap(), 0);

        let first = store.create_first_user(owner("alpha")).await.unwrap();
        assert_eq!(first.unwrap().username, "alpha");
        assert!(
            store
                .create_first_user(owner("beta"))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(store.count_users().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn username_lookup_is_case_insensitive_and_unique() {
        let store = test_store().await;
        store.create_user(owner("Alpha")).await.unwrap();
        let found = store.find_user_by_username("alpha").await.unwrap().unwrap();
        assert_eq!(found.username, "Alpha");
        assert!(store.create_user(owner("ALPHA")).await.is_err());
    }

    #[tokio::test]
    async fn password_and_last_login_updates() {
        let store = test_store().await;
        let user = store.create_user(owner("alpha")).await.unwrap();
        assert!(store.set_password_hash(&user.id, "new").await.unwrap());
        store.touch_last_login(&user.id, 42).await.unwrap();
        let user = store.find_user_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(user.password_hash, "new");
        assert_eq!(user.last_login_at, Some(42));
        assert!(!store.set_password_hash("missing", "x").await.unwrap());
    }

    #[test]
    fn debug_hides_secrets() {
        let user = User {
            id: "1".into(),
            username: "alpha".into(),
            password_hash: "$argon2id$secret".into(),
            role: "owner".into(),
            totp_secret: Some("TOTPSECRET".into()),
            created_at: 0,
            last_login_at: None,
        };
        let debug = format!("{user:?}");
        assert!(!debug.contains("argon2id"));
        assert!(!debug.contains("TOTPSECRET"));
    }
}
