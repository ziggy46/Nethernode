//! Player management records: what NetherNode knows about players beyond
//! Minecraft's own lists (play history, notes), the bans it placed (for
//! expiry and bans across all servers), and cached Mojang lookups.

use crate::{PlayerSession, Result, Store, new_id, now_ms};

/// A player as seen through the proxy, with play history in one scope
/// (one server, or all of them).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PlayerSummary {
    /// UUID.
    pub uuid: String,
    /// Latest username.
    pub name: String,
    /// First join in scope, unix ms.
    pub first_seen: i64,
    /// Latest activity in scope (now, if online), unix ms.
    pub last_seen: i64,
    /// Total time online in scope, ms.
    pub playtime_ms: i64,
    /// Address of the latest session in scope.
    pub last_ip: Option<String>,
    /// Admin notes.
    pub notes: Option<String>,
}

/// A ban NetherNode placed.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PlayerBan {
    /// ULID.
    pub id: String,
    /// The server, or `None` for every server.
    pub server_id: Option<String>,
    /// Player UUID.
    pub uuid: Option<String>,
    /// Username.
    pub name: String,
    /// Also banned by this IP.
    pub ip: Option<String>,
    /// Why.
    pub reason: Option<String>,
    /// When it ends (unix ms); `None` is permanent.
    pub expires_at: Option<i64>,
    /// Who placed it.
    pub created_by: Option<String>,
    /// When, unix ms.
    pub created_at: i64,
    /// When it was lifted, if it has been.
    pub lifted_at: Option<i64>,
}

/// Fields for a new ban.
#[derive(Debug, Clone)]
pub struct NewPlayerBan<'a> {
    /// The server, or `None` for every server.
    pub server_id: Option<&'a str>,
    /// Player UUID.
    pub uuid: Option<&'a str>,
    /// Username.
    pub name: &'a str,
    /// Also banned by this IP.
    pub ip: Option<&'a str>,
    /// Why.
    pub reason: Option<&'a str>,
    /// When it ends; `None` is permanent.
    pub expires_at: Option<i64>,
    /// Who placed it.
    pub created_by: Option<&'a str>,
}

/// A cached Mojang lookup. `uuid` `None` means no account has the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedProfile {
    /// UUID, if the account exists.
    pub uuid: Option<String>,
    /// Name with correct capitalization, if the account exists.
    pub name: Option<String>,
    /// When it was looked up, unix ms.
    pub fetched_at: i64,
}

impl Store {
    /// Players with play history, most recently seen first. With a server,
    /// only that server's sessions count.
    pub async fn list_player_summaries(
        &self,
        server_id: Option<&str>,
    ) -> Result<Vec<PlayerSummary>> {
        let now = now_ms();
        let rows = sqlx::query_as!(
            PlayerSummary,
            r#"SELECT p.uuid AS "uuid!", p.last_username AS "name!",
                      MIN(s.joined_at) AS "first_seen!: i64",
                      MAX(COALESCE(s.left_at, ?2)) AS "last_seen!: i64",
                      SUM(COALESCE(s.left_at, ?2) - s.joined_at) AS "playtime_ms!: i64",
                      (SELECT s2.ip FROM player_sessions s2
                        WHERE s2.uuid = p.uuid AND (?1 IS NULL OR s2.server_id = ?1)
                        ORDER BY s2.joined_at DESC LIMIT 1) AS last_ip,
                      p.notes
               FROM players p
               JOIN player_sessions s ON s.uuid = p.uuid AND (?1 IS NULL OR s.server_id = ?1)
               GROUP BY p.uuid
               ORDER BY 4 DESC"#,
            server_id,
            now,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// One player's record, with play history across all servers.
    pub async fn player_summary(&self, uuid: &str) -> Result<Option<PlayerSummary>> {
        Ok(self
            .list_player_summaries(None)
            .await?
            .into_iter()
            .find(|p| p.uuid == uuid))
    }

    /// Records that a player exists (from a list entry or lookup) without
    /// a session, so notes can be attached.
    pub async fn ensure_player(&self, uuid: &str, name: &str) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            r#"INSERT INTO players (uuid, last_username, first_seen, last_seen)
               VALUES (?1, ?2, ?3, ?3) ON CONFLICT (uuid) DO NOTHING"#,
            uuid,
            name,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Admin notes for a player (`None` clears them).
    pub async fn set_player_notes(&self, uuid: &str, notes: Option<&str>) -> Result<bool> {
        let result = sqlx::query!("UPDATE players SET notes = ?1 WHERE uuid = ?2", notes, uuid)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Notes for a player, if any.
    pub async fn player_notes(&self, uuid: &str) -> Result<Option<String>> {
        let notes = sqlx::query_scalar!("SELECT notes FROM players WHERE uuid = ?1", uuid)
            .fetch_optional(self.pool())
            .await?;
        Ok(notes.flatten())
    }

    /// A player's sessions, newest first.
    pub async fn player_session_history(
        &self,
        uuid: &str,
        server_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<PlayerSession>> {
        let rows = sqlx::query_as!(
            PlayerSession,
            r#"SELECT id AS "id!", server_id, uuid, username, ip, joined_at, left_at
               FROM player_sessions
               WHERE uuid = ?1 AND (?2 IS NULL OR server_id = ?2)
               ORDER BY joined_at DESC LIMIT ?3"#,
            uuid,
            server_id,
            limit,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Records a ban NetherNode placed.
    pub async fn insert_player_ban(&self, ban: NewPlayerBan<'_>) -> Result<PlayerBan> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            r#"INSERT INTO player_bans (id, server_id, uuid, name, ip, reason, expires_at, created_by, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            id,
            ban.server_id,
            ban.uuid,
            ban.name,
            ban.ip,
            ban.reason,
            ban.expires_at,
            ban.created_by,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(PlayerBan {
            id,
            server_id: ban.server_id.map(str::to_owned),
            uuid: ban.uuid.map(str::to_owned),
            name: ban.name.to_owned(),
            ip: ban.ip.map(str::to_owned),
            reason: ban.reason.map(str::to_owned),
            expires_at: ban.expires_at,
            created_by: ban.created_by.map(str::to_owned),
            created_at: now,
            lifted_at: None,
        })
    }

    /// Active bans that apply to a server: its own and global ones. With
    /// `None`, every active ban.
    pub async fn active_player_bans(&self, server_id: Option<&str>) -> Result<Vec<PlayerBan>> {
        let now = now_ms();
        let rows = sqlx::query_as!(
            PlayerBan,
            r#"SELECT id AS "id!", server_id, uuid, name AS "name!", ip, reason, expires_at, created_by,
                      created_at AS "created_at!", lifted_at
               FROM player_bans
               WHERE lifted_at IS NULL AND (expires_at IS NULL OR expires_at > ?2)
                 AND (?1 IS NULL OR server_id IS NULL OR server_id = ?1)
               ORDER BY created_at DESC"#,
            server_id,
            now,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Bans past their expiry that haven't been lifted yet.
    pub async fn expired_player_bans(&self) -> Result<Vec<PlayerBan>> {
        let now = now_ms();
        let rows = sqlx::query_as!(
            PlayerBan,
            r#"SELECT id AS "id!", server_id, uuid, name AS "name!", ip, reason, expires_at, created_by,
                      created_at AS "created_at!", lifted_at
               FROM player_bans
               WHERE lifted_at IS NULL AND expires_at IS NOT NULL AND expires_at <= ?1
               ORDER BY expires_at"#,
            now,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Marks a ban lifted. Returns `false` if it already was.
    pub async fn lift_player_ban(&self, id: &str) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE player_bans SET lifted_at = ?1 WHERE id = ?2 AND lifted_at IS NULL",
            now,
            id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Lifts every active ban on a player in a scope (a server's own bans,
    /// or with `None` the global ones). Returns how many.
    pub async fn lift_player_bans_for(&self, uuid: &str, server_id: Option<&str>) -> Result<u64> {
        let now = now_ms();
        let result = sqlx::query!(
            r#"UPDATE player_bans SET lifted_at = ?1
               WHERE uuid = ?2 AND lifted_at IS NULL
                 AND ((?3 IS NULL AND server_id IS NULL) OR server_id = ?3)"#,
            now,
            uuid,
            server_id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected())
    }

    /// A cached Mojang lookup for a username (any case).
    pub async fn cached_profile(&self, name: &str) -> Result<Option<CachedProfile>> {
        let key = name.to_lowercase();
        let row = sqlx::query!(
            "SELECT uuid, name, fetched_at FROM profile_cache WHERE name_key = ?1",
            key
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| CachedProfile {
            uuid: r.uuid,
            name: r.name,
            fetched_at: r.fetched_at,
        }))
    }

    /// Caches a lookup result (`uuid` `None`: no such account).
    pub async fn cache_profile(
        &self,
        name: &str,
        uuid: Option<&str>,
        proper: Option<&str>,
    ) -> Result<()> {
        let key = name.to_lowercase();
        let now = now_ms();
        sqlx::query!(
            r#"INSERT INTO profile_cache (name_key, uuid, name, fetched_at) VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT (name_key) DO UPDATE SET uuid = excluded.uuid, name = excluded.name,
                                                   fetched_at = excluded.fetched_at"#,
            key,
            uuid,
            proper,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::players::NewPlayerSession;
    use crate::servers::test_server;
    use crate::test_store;

    #[tokio::test]
    async fn summaries_bans_and_cache() {
        let store = test_store().await;
        let a = store.insert_server(test_server("a", 30001)).await.unwrap();
        let b = store.insert_server(test_server("b", 30002)).await.unwrap();
        for server in [&a.id, &b.id] {
            let id = store
                .open_player_session(NewPlayerSession {
                    server_id: server,
                    uuid: Some("u1"),
                    username: "Alex",
                    ip: Some("203.0.113.5"),
                })
                .await
                .unwrap();
            store.close_player_session(&id).await.unwrap();
        }
        assert_eq!(store.list_player_summaries(None).await.unwrap().len(), 1);
        assert_eq!(
            store
                .list_player_summaries(Some(&a.id))
                .await
                .unwrap()
                .len(),
            1
        );
        let c = store.insert_server(test_server("c", 30003)).await.unwrap();
        assert!(
            store
                .list_player_summaries(Some(&c.id))
                .await
                .unwrap()
                .is_empty()
        );
        let summary = store.player_summary("u1").await.unwrap().unwrap();
        assert_eq!(summary.last_ip.as_deref(), Some("203.0.113.5"));
        assert!(store.set_player_notes("u1", Some("helpful")).await.unwrap());
        assert_eq!(
            store.player_notes("u1").await.unwrap().as_deref(),
            Some("helpful")
        );
        assert_eq!(
            store
                .player_session_history("u1", None, 10)
                .await
                .unwrap()
                .len(),
            2
        );

        let ban = |server_id, expires_at| NewPlayerBan {
            server_id,
            uuid: Some("u1"),
            name: "Alex",
            ip: None,
            reason: Some("x"),
            expires_at,
            created_by: None,
        };
        let global = store.insert_player_ban(ban(None, None)).await.unwrap();
        store
            .insert_player_ban(ban(Some(&a.id), Some(1)))
            .await
            .unwrap();
        assert_eq!(
            store.active_player_bans(Some(&b.id)).await.unwrap(),
            vec![global.clone()],
            "global applies everywhere"
        );
        assert_eq!(store.expired_player_bans().await.unwrap().len(), 1);
        assert_eq!(
            store.lift_player_bans_for("u1", None).await.unwrap(),
            1,
            "only the global one"
        );
        assert!(store.active_player_bans(None).await.unwrap().is_empty());
        assert!(!store.lift_player_ban(&global.id).await.unwrap());

        store
            .cache_profile(
                "Notch",
                Some("069a79f4-44e9-4726-a5be-fca90e38aaf5"),
                Some("Notch"),
            )
            .await
            .unwrap();
        store
            .cache_profile("nobody_here", None, None)
            .await
            .unwrap();
        assert_eq!(
            store
                .cached_profile("NOTCH")
                .await
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("Notch")
        );
        assert_eq!(
            store
                .cached_profile("Nobody_Here")
                .await
                .unwrap()
                .unwrap()
                .uuid,
            None
        );
        assert!(store.cached_profile("unknown").await.unwrap().is_none());
    }
}
