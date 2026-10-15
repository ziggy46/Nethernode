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
    /// Sealed TOTP secret, when two-factor sign-in is on.
    pub totp_secret: Option<String>,
    /// Sealed TOTP secret awaiting confirmation.
    pub totp_pending: Option<String>,
    /// When two-factor sign-in was turned on, unix ms.
    pub totp_enabled_at: Option<i64>,
    /// The last TOTP time step accepted.
    pub totp_last_step: Option<i64>,
    /// The password was set by someone else and must be changed.
    pub must_change_password: bool,
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
            .field("totp_enabled_at", &self.totp_enabled_at)
            .field("must_change_password", &self.must_change_password)
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
    /// The password is temporary and must be changed at first sign-in.
    pub must_change_password: bool,
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
            r#"INSERT INTO users (id, username, password_hash, role, must_change_password, created_at)
               SELECT ?1, ?2, ?3, ?4, ?5, ?6
               WHERE NOT EXISTS (SELECT 1 FROM users)"#,
            id,
            user.username,
            user.password_hash,
            user.role,
            user.must_change_password,
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
            r#"INSERT INTO users (id, username, password_hash, role, must_change_password, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)
               RETURNING id AS "id!", username, password_hash, role, totp_secret, totp_pending, totp_enabled_at,
                      totp_last_step, must_change_password AS "must_change_password: bool", created_at,
                      last_login_at"#,
            id,
            user.username,
            user.password_hash,
            user.role,
            user.must_change_password,
            now,
        )
        .fetch_one(self.pool())
        .await?;
        Ok(created)
    }

    /// All users, by username.
    pub async fn list_users(&self) -> Result<Vec<User>> {
        let users = sqlx::query_as!(
            User,
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, totp_pending, totp_enabled_at,
                      totp_last_step, must_change_password AS "must_change_password: bool", created_at,
                      last_login_at
               FROM users ORDER BY username COLLATE NOCASE"#,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(users)
    }

    /// Looks up a user by ID.
    pub async fn find_user_by_id(&self, id: &str) -> Result<Option<User>> {
        let user = sqlx::query_as!(
            User,
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, totp_pending, totp_enabled_at,
                      totp_last_step, must_change_password AS "must_change_password: bool", created_at,
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
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, totp_pending, totp_enabled_at,
                      totp_last_step, must_change_password AS "must_change_password: bool", created_at,
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
            r#"SELECT id AS "id!", username, password_hash, role, totp_secret, totp_pending, totp_enabled_at,
                      totp_last_step, must_change_password AS "must_change_password: bool", created_at,
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

    /// Replaces a user's password, and whether it must be changed at next
    /// sign-in. Returns `false` if the user doesn't exist.
    pub async fn set_password(
        &self,
        user_id: &str,
        password_hash: &str,
        must_change: bool,
    ) -> Result<bool> {
        let result = sqlx::query!(
            "UPDATE users SET password_hash = ?1, must_change_password = ?2 WHERE id = ?3",
            password_hash,
            must_change,
            user_id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Changes a user's role, unless that would leave no owner. Returns
    /// `false` if the user doesn't exist or is the last owner.
    pub async fn set_user_role(&self, user_id: &str, role: &str) -> Result<bool> {
        let result = sqlx::query!(
            r#"UPDATE users SET role = ?2
               WHERE id = ?1
                 AND NOT (role = 'owner' AND ?2 <> 'owner'
                          AND (SELECT COUNT(*) FROM users WHERE role = 'owner') <= 1)"#,
            user_id,
            role,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Deletes a user, unless they're the last owner. Their sessions go too.
    /// Returns `false` if the user doesn't exist or is the last owner.
    pub async fn delete_user(&self, user_id: &str) -> Result<bool> {
        let result = sqlx::query!(
            r#"DELETE FROM users
               WHERE id = ?1
                 AND NOT (role = 'owner' AND (SELECT COUNT(*) FROM users WHERE role = 'owner') <= 1)"#,
            user_id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// A user's color theme: `system`, `dark` or `light`. `None` if the user
    /// doesn't exist.
    pub async fn user_theme(&self, user_id: &str) -> Result<Option<String>> {
        let theme = sqlx::query_scalar!("SELECT theme FROM users WHERE id = ?1", user_id)
            .fetch_optional(self.pool())
            .await?;
        Ok(theme)
    }

    /// Sets a user's color theme (`system`, `dark` or `light`). Returns
    /// `false` if the user doesn't exist.
    pub async fn set_user_theme(&self, user_id: &str, theme: &str) -> Result<bool> {
        let result = sqlx::query!("UPDATE users SET theme = ?2 WHERE id = ?1", user_id, theme)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Stores (or clears) a sealed TOTP secret awaiting confirmation.
    pub async fn set_totp_pending(&self, user_id: &str, sealed: Option<&str>) -> Result<()> {
        sqlx::query!(
            "UPDATE users SET totp_pending = ?2 WHERE id = ?1",
            user_id,
            sealed
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Turns two-factor sign-in on with `sealed` and replaces the recovery
    /// codes, in one transaction.
    pub async fn enable_totp(
        &self,
        user_id: &str,
        sealed: &str,
        step: i64,
        recovery_hashes: &[String],
    ) -> Result<()> {
        let now = now_ms();
        let mut tx = self.pool().begin().await?;
        sqlx::query!(
            r#"UPDATE users SET totp_secret = ?2, totp_pending = NULL, totp_enabled_at = ?3,
                                totp_last_step = ?4
               WHERE id = ?1"#,
            user_id,
            sealed,
            now,
            step,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("DELETE FROM recovery_codes WHERE user_id = ?1", user_id)
            .execute(&mut *tx)
            .await?;
        for hash in recovery_hashes {
            let id = new_id();
            sqlx::query!(
                "INSERT INTO recovery_codes (id, user_id, code_hash) VALUES (?1, ?2, ?3)",
                id,
                user_id,
                hash,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Turns two-factor sign-in off and removes the recovery codes.
    pub async fn disable_totp(&self, user_id: &str) -> Result<()> {
        let mut tx = self.pool().begin().await?;
        sqlx::query!(
            r#"UPDATE users SET totp_secret = NULL, totp_pending = NULL, totp_enabled_at = NULL,
                                totp_last_step = NULL
               WHERE id = ?1"#,
            user_id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("DELETE FROM recovery_codes WHERE user_id = ?1", user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Accepts TOTP time step `step` if it's later than any accepted before.
    /// Returns `false` for a replayed (or older) code.
    pub async fn advance_totp_step(&self, user_id: &str, step: i64) -> Result<bool> {
        let result = sqlx::query!(
            r#"UPDATE users SET totp_last_step = ?2
               WHERE id = ?1 AND (totp_last_step IS NULL OR totp_last_step < ?2)"#,
            user_id,
            step,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Uses up a recovery code. Returns `false` if it's wrong or used.
    pub async fn use_recovery_code(&self, user_id: &str, code_hash: &str) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            r#"UPDATE recovery_codes SET used_at = ?3
               WHERE user_id = ?1 AND code_hash = ?2 AND used_at IS NULL"#,
            user_id,
            code_hash,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// How many unused recovery codes a user has.
    pub async fn recovery_codes_left(&self, user_id: &str) -> Result<i64> {
        let count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!: i64" FROM recovery_codes
               WHERE user_id = ?1 AND used_at IS NULL"#,
            user_id
        )
        .fetch_one(self.pool())
        .await?;
        Ok(count)
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
            must_change_password: false,
        }
    }

    #[tokio::test]
    async fn theme_defaults_to_system_and_only_takes_known_values() {
        let store = test_store().await;
        let user = store.create_user(owner("alpha")).await.unwrap();
        assert_eq!(
            store.user_theme(&user.id).await.unwrap().as_deref(),
            Some("system")
        );

        assert!(store.set_user_theme(&user.id, "light").await.unwrap());
        assert_eq!(
            store.user_theme(&user.id).await.unwrap().as_deref(),
            Some("light")
        );
        assert!(store.set_user_theme(&user.id, "sepia").await.is_err());
        assert_eq!(
            store.user_theme(&user.id).await.unwrap().as_deref(),
            Some("light")
        );

        assert!(!store.set_user_theme("missing", "dark").await.unwrap());
        assert_eq!(store.user_theme("missing").await.unwrap(), None);
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
            totp_pending: Some("PENDINGSECRET".into()),
            totp_enabled_at: None,
            totp_last_step: None,
            must_change_password: false,
            created_at: 0,
            last_login_at: None,
        };
        let debug = format!("{user:?}");
        assert!(!debug.contains("argon2id"));
        assert!(!debug.contains("TOTPSECRET"));
        assert!(!debug.contains("PENDINGSECRET"));
    }

    #[tokio::test]
    async fn the_last_owner_stays() {
        let store = test_store().await;
        let first = store.create_user(owner("first")).await.unwrap();
        // The only owner can't be demoted or deleted.
        assert!(!store.set_user_role(&first.id, "admin").await.unwrap());
        assert!(!store.delete_user(&first.id).await.unwrap());
        let second = store.create_user(owner("second")).await.unwrap();
        assert!(store.set_user_role(&first.id, "admin").await.unwrap());
        assert!(!store.set_user_role(&second.id, "viewer").await.unwrap());
        assert!(!store.delete_user(&second.id).await.unwrap());
        assert!(store.delete_user(&first.id).await.unwrap());
        assert!(store.find_user_by_id(&first.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn totp_steps_only_move_forward_and_codes_work_once() {
        let store = test_store().await;
        let user = store.create_user(owner("alpha")).await.unwrap();
        store
            .enable_totp(&user.id, "sealed", 100, &["h1".into(), "h2".into()])
            .await
            .unwrap();
        let found = store.find_user_by_id(&user.id).await.unwrap().unwrap();
        assert_eq!(found.totp_secret.as_deref(), Some("sealed"));
        assert!(found.totp_enabled_at.is_some());
        assert!(!store.advance_totp_step(&user.id, 100).await.unwrap());
        assert!(!store.advance_totp_step(&user.id, 99).await.unwrap());
        assert!(store.advance_totp_step(&user.id, 101).await.unwrap());

        assert_eq!(store.recovery_codes_left(&user.id).await.unwrap(), 2);
        assert!(store.use_recovery_code(&user.id, "h1").await.unwrap());
        assert!(!store.use_recovery_code(&user.id, "h1").await.unwrap());
        assert!(!store.use_recovery_code(&user.id, "nope").await.unwrap());
        assert_eq!(store.recovery_codes_left(&user.id).await.unwrap(), 1);

        store.disable_totp(&user.id).await.unwrap();
        let found = store.find_user_by_id(&user.id).await.unwrap().unwrap();
        assert!(found.totp_secret.is_none() && found.totp_last_step.is_none());
        assert_eq!(store.recovery_codes_left(&user.id).await.unwrap(), 0);
    }
}
