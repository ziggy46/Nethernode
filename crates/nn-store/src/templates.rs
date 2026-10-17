//! Server templates (see migration 0014).

use crate::{Result, Store};

/// A saved template.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Template {
    /// ULID; its archive is `template-<id>.tar.zst`.
    pub id: String,
    /// What it's called.
    pub name: String,
    /// The server it was made from.
    pub source_name: String,
    /// Its server software.
    pub loader: String,
    /// Its Minecraft version.
    pub mc_version: String,
    /// Its Java heap in MiB.
    pub memory_mb: i64,
    /// The source's settings, as a server row in JSON.
    pub server_json: String,
    /// Size of its archive.
    pub archive_bytes: i64,
    /// When it was saved, unix ms.
    pub created_at: i64,
}

impl Template {
    /// Its archive's name among the panel's backups.
    pub fn archive(&self) -> String {
        archive_name(&self.id)
    }
}

/// The archive name of template `id`.
pub fn archive_name(id: &str) -> String {
    format!("template-{id}.tar.zst")
}

impl Store {
    /// Saves a template.
    pub async fn insert_template(&self, t: &Template) -> Result<()> {
        sqlx::query!(
            r#"INSERT INTO server_templates (id, name, source_name, loader, mc_version, memory_mb,
                   server_json, archive_bytes, created_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
            t.id,
            t.name,
            t.source_name,
            t.loader,
            t.mc_version,
            t.memory_mb,
            t.server_json,
            t.archive_bytes,
            t.created_at,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Templates, newest first.
    pub async fn list_templates(&self) -> Result<Vec<Template>> {
        Ok(sqlx::query_as!(
            Template,
            r#"SELECT id AS "id!", name, source_name, loader, mc_version, memory_mb, server_json,
                   archive_bytes, created_at
               FROM server_templates ORDER BY created_at DESC"#
        )
        .fetch_all(self.pool())
        .await?)
    }

    /// One template.
    pub async fn find_template(&self, id: &str) -> Result<Option<Template>> {
        Ok(sqlx::query_as!(
            Template,
            r#"SELECT id AS "id!", name, source_name, loader, mc_version, memory_mb, server_json,
                   archive_bytes, created_at
               FROM server_templates WHERE id = ?1"#,
            id
        )
        .fetch_optional(self.pool())
        .await?)
    }

    /// Deletes a template's record. Returns whether there was one.
    pub async fn delete_template(&self, id: &str) -> Result<bool> {
        let done = sqlx::query!("DELETE FROM server_templates WHERE id = ?1", id)
            .execute(self.pool())
            .await?;
        Ok(done.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_store;

    #[tokio::test]
    async fn templates_round_trip() {
        let store = test_store().await;
        let t = Template {
            id: "01T".into(),
            name: "Modded base".into(),
            source_name: "SMP".into(),
            loader: "fabric".into(),
            mc_version: "1.21.4".into(),
            memory_mb: 4096,
            server_json: "{}".into(),
            archive_bytes: 1234,
            created_at: 1,
        };
        store.insert_template(&t).await.unwrap();
        assert_eq!(
            store.list_templates().await.unwrap(),
            std::slice::from_ref(&t)
        );
        assert_eq!(store.find_template("01T").await.unwrap(), Some(t.clone()));
        assert_eq!(t.archive(), "template-01T.tar.zst");
        assert!(store.delete_template("01T").await.unwrap());
        assert!(!store.delete_template("01T").await.unwrap());
    }
}
