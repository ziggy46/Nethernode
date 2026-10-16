//! Players seen through the proxy and their play sessions.

use crate::{Result, Store, new_id, now_ms};

/// A play session: one connection from login to disconnect.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PlayerSession {
    /// ULID.
    pub id: String,
    /// Server joined.
    pub server_id: String,
    /// Player UUID, when the client sent one.
    pub uuid: Option<String>,
    /// Username at login.
    pub username: String,
    /// Client IP.
    pub ip: Option<String>,
    /// Join time, unix ms.
    pub joined_at: i64,
    /// Leave time, unix ms (`None` while online).
    pub left_at: Option<i64>,
}

/// Fields recorded when a player joins.
#[derive(Debug, Clone)]
pub struct NewPlayerSession<'a> {
    /// Server joined.
    pub server_id: &'a str,
    /// Player UUID, if known.
    pub uuid: Option<&'a str>,
    /// Username.
    pub username: &'a str,
    /// Client IP.
    pub ip: Option<&'a str>,
}

impl Store {
    /// Records a join: opens a session and upserts the player. Returns the session ID.
    pub async fn open_player_session(&self, session: NewPlayerSession<'_>) -> Result<String> {
        let id = new_id();
        let now = now_ms();
        let mut tx = self.pool().begin().await?;
        sqlx::query!(
            r#"INSERT INTO player_sessions (id, server_id, uuid, username, ip, joined_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
            id,
            session.server_id,
            session.uuid,
            session.username,
            session.ip,
            now,
        )
        .execute(&mut *tx)
        .await?;
        if let Some(uuid) = session.uuid {
            sqlx::query!(
                r#"INSERT INTO players (uuid, last_username, first_seen, last_seen)
                   VALUES (?1, ?2, ?3, ?3)
                   ON CONFLICT (uuid) DO UPDATE
                   SET last_username = excluded.last_username, last_seen = excluded.last_seen"#,
                uuid,
                session.username,
                now,
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Records a disconnect. Returns `false` if the session was already closed or missing.
    pub async fn close_player_session(&self, id: &str) -> Result<bool> {
        let now = now_ms();
        let mut tx = self.pool().begin().await?;
        let result = sqlx::query!(
            "UPDATE player_sessions SET left_at = ?1 WHERE id = ?2 AND left_at IS NULL",
            now,
            id,
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            r#"UPDATE players SET last_seen = ?1
               WHERE uuid = (SELECT uuid FROM player_sessions WHERE id = ?2)"#,
            now,
            id,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    /// Closes sessions left open by an unclean shutdown. Returns how many.
    pub async fn close_stale_player_sessions(&self) -> Result<u64> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE player_sessions SET left_at = ?1 WHERE left_at IS NULL",
            now
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected())
    }

    /// Open sessions, newest first, optionally for one server.
    pub async fn online_player_sessions(
        &self,
        server_id: Option<&str>,
    ) -> Result<Vec<PlayerSession>> {
        let rows = sqlx::query_as!(
            PlayerSession,
            r#"SELECT id AS "id!", server_id, uuid, username, ip, joined_at, left_at
               FROM player_sessions
               WHERE left_at IS NULL AND (?1 IS NULL OR server_id = ?1)
               ORDER BY joined_at DESC, id DESC"#,
            server_id,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// The most players on a server at once since `since` (unix ms), from
    /// the proxy's join and leave records.
    pub async fn player_peak_since(&self, server_id: &str, since: i64) -> Result<u32> {
        let rows = sqlx::query!(
            r#"SELECT joined_at AS "joined_at!: i64", left_at
               FROM player_sessions
               WHERE server_id = ?1 AND (left_at IS NULL OR left_at >= ?2)"#,
            server_id,
            since,
        )
        .fetch_all(self.pool())
        .await?;
        // +1 at each join, -1 at each leave; leaves sort before joins at the
        // same moment.
        let mut changes: Vec<(i64, i32)> = Vec::with_capacity(rows.len() * 2);
        for row in rows {
            changes.push((row.joined_at.max(since), 1));
            if let Some(left) = row.left_at {
                changes.push((left, -1));
            }
        }
        changes.sort_unstable();
        let (mut now, mut peak) = (0i32, 0i32);
        for (_, change) in changes {
            now += change;
            peak = peak.max(now);
        }
        Ok(u32::try_from(peak).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servers::test_server;
    use crate::test_store;

    #[tokio::test]
    async fn the_peak_counts_players_online_at_once() {
        let store = test_store().await;
        let server = store
            .insert_server(test_server("smp", 30001))
            .await
            .unwrap();
        let join = |name: &'static str| NewPlayerSession {
            server_id: &server.id,
            uuid: Some(name),
            username: name,
            ip: Some("127.0.0.1"),
        };
        assert_eq!(store.player_peak_since(&server.id, 0).await.unwrap(), 0);
        let a = store.open_player_session(join("a")).await.unwrap();
        let b = store.open_player_session(join("b")).await.unwrap();
        assert_eq!(store.player_peak_since(&server.id, 0).await.unwrap(), 2);
        store.close_player_session(&a).await.unwrap();
        store.close_player_session(&b).await.unwrap();
        store.open_player_session(join("c")).await.unwrap();
        assert_eq!(store.player_peak_since(&server.id, 0).await.unwrap(), 2);
        // Sessions that ended before the window don't count.
        let later = crate::now_ms() + 60_000;
        assert_eq!(store.player_peak_since(&server.id, later).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn sessions_open_and_close() {
        let store = test_store().await;
        let server = store
            .insert_server(test_server("smp", 30001))
            .await
            .unwrap();
        let join = |username, uuid| NewPlayerSession {
            server_id: &server.id,
            uuid,
            username,
            ip: Some("1.2.3.4"),
        };

        let a = store
            .open_player_session(join("Alex", Some("uuid-a")))
            .await
            .unwrap();
        store
            .open_player_session(join("OldClient", None))
            .await
            .unwrap();
        assert_eq!(
            store
                .online_player_sessions(Some(&server.id))
                .await
                .unwrap()
                .len(),
            2
        );

        assert!(store.close_player_session(&a).await.unwrap());
        assert!(
            !store.close_player_session(&a).await.unwrap(),
            "already closed"
        );
        assert_eq!(store.online_player_sessions(None).await.unwrap().len(), 1);

        // Rejoining under a new name updates the player record.
        store
            .open_player_session(join("Alexandra", Some("uuid-a")))
            .await
            .unwrap();
        let name: String =
            sqlx::query_scalar("SELECT last_username FROM players WHERE uuid = 'uuid-a'")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(name, "Alexandra");

        assert_eq!(store.close_stale_player_sessions().await.unwrap(), 2);
        assert!(store.online_player_sessions(None).await.unwrap().is_empty());
    }
}
