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

impl Store {
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
}
