//! Audit log of mutating admin actions.

use crate::{Result, Store, new_id, now_ms};

/// An audit log entry to record.
#[derive(Debug, Clone)]
pub struct NewAuditEntry<'a> {
    /// Acting user, if known.
    pub user_id: Option<&'a str>,
    /// Dotted action name, e.g. `auth.login`.
    pub action: &'a str,
    /// Kind of object acted on, e.g. `user`.
    pub target_type: Option<&'a str>,
    /// ID of the object acted on.
    pub target_id: Option<&'a str>,
    /// Extra structured details.
    pub details: Option<serde_json::Value>,
}

/// A recorded audit log entry.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuditEntry {
    /// ULID.
    pub id: String,
    /// Acting user, if known.
    pub user_id: Option<String>,
    /// Dotted action name.
    pub action: String,
    /// Kind of object acted on.
    pub target_type: Option<String>,
    /// ID of the object acted on.
    pub target_id: Option<String>,
    /// JSON details.
    pub details_json: Option<String>,
    /// Time recorded, unix ms.
    pub created_at: i64,
}

/// Which audit entries to list.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter<'a> {
    /// Actions starting with any of these (`server.`, `auth.`); all if empty.
    pub prefixes: &'a [&'a str],
    /// Only this user's actions.
    pub user_id: Option<&'a str>,
    /// Only actions on this target, or whose details name it as their
    /// `server_id` (player actions).
    pub target_id: Option<&'a str>,
    /// Entries older than this entry ID, for paging.
    pub before_id: Option<&'a str>,
    /// At most this many.
    pub limit: i64,
}

impl Store {
    /// Audit entries matching `filter`, newest first.
    pub async fn query_audit(&self, filter: AuditFilter<'_>) -> Result<Vec<AuditEntry>> {
        // Prefixes become one JSON array so the query stays static.
        let prefixes = serde_json::to_string(filter.prefixes)?;
        let rows = sqlx::query_as!(
            AuditEntry,
            r#"SELECT id AS "id!", user_id, action, target_type, target_id, details_json, created_at
               FROM audit_log
               WHERE (json_array_length(?1) = 0
                      OR EXISTS (SELECT 1 FROM json_each(?1) p
                                 WHERE substr(audit_log.action, 1, length(p.value)) = p.value))
                 AND (?2 IS NULL OR user_id = ?2)
                 AND (?3 IS NULL OR target_id = ?3 OR json_extract(details_json, '$.server_id') = ?3)
                 AND (?4 IS NULL OR id < ?4)
               ORDER BY id DESC LIMIT ?5"#,
            prefixes,
            filter.user_id,
            filter.target_id,
            filter.before_id,
            filter.limit,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Appends an entry to the audit log.
    pub async fn insert_audit(&self, entry: NewAuditEntry<'_>) -> Result<()> {
        let id = new_id();
        let now = now_ms();
        let details = entry.details.map(|d| d.to_string());
        sqlx::query!(
            r#"INSERT INTO audit_log (id, user_id, action, target_type, target_id, details_json, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
            id,
            entry.user_id,
            entry.action,
            entry.target_type,
            entry.target_id,
            details,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The most recent audit entries, newest first.
    pub async fn recent_audit(&self, limit: i64) -> Result<Vec<AuditEntry>> {
        let rows = sqlx::query_as!(
            AuditEntry,
            r#"SELECT id AS "id!", user_id, action, target_type, target_id, details_json, created_at
               FROM audit_log ORDER BY id DESC LIMIT ?1"#,
            limit,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_store;

    use super::*;

    #[tokio::test]
    async fn audit_entries_are_recorded_newest_first() {
        let store = test_store().await;
        for action in ["a.one", "a.two"] {
            store
                .insert_audit(NewAuditEntry {
                    user_id: None,
                    action,
                    target_type: Some("user"),
                    target_id: Some("x"),
                    details: Some(serde_json::json!({ "k": 1 })),
                })
                .await
                .unwrap();
        }
        let rows = store.recent_audit(10).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].action, "a.two");
        assert_eq!(rows[0].details_json.as_deref(), Some(r#"{"k":1}"#));
    }

    #[tokio::test]
    async fn audit_queries_filter_and_page() {
        let store = test_store().await;
        for (action, target) in [
            ("server.start", "s1"),
            ("server.stop", "s2"),
            ("backup.create", "s1"),
            ("auth.login", "u1"),
            ("server.restart", "s1"),
        ] {
            store
                .insert_audit(NewAuditEntry {
                    user_id: None,
                    action,
                    target_type: Some("server"),
                    target_id: Some(target),
                    details: None,
                })
                .await
                .unwrap();
        }
        let actions =
            |rows: Vec<AuditEntry>| rows.into_iter().map(|e| e.action).collect::<Vec<_>>();
        let all = |limit| AuditFilter {
            limit,
            ..Default::default()
        };
        assert_eq!(store.query_audit(all(10)).await.unwrap().len(), 5);
        let servers = store
            .query_audit(AuditFilter {
                prefixes: &["server.", "backup."],
                target_id: Some("s1"),
                ..all(10)
            })
            .await
            .unwrap();
        assert_eq!(
            actions(servers),
            ["server.restart", "backup.create", "server.start"]
        );
        let first = store.query_audit(all(2)).await.unwrap();
        let next = store
            .query_audit(AuditFilter {
                before_id: Some(&first[1].id),
                ..all(2)
            })
            .await
            .unwrap();
        assert_eq!(actions(next), ["backup.create", "server.stop"]);
    }
}
