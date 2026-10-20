//! Server modules (see migration 0016).

use crate::{Result, Store, new_id, now_ms};

/// A module attached to a server.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct Module {
    /// ULID.
    pub id: String,
    /// The server it runs next to.
    pub server_id: String,
    /// The template it's made from.
    pub template: String,
    /// Folder and container name, unique on its server.
    pub name: String,
    /// Settings (JSON object).
    pub settings_json: String,
    /// `with-server`, `always`, or `manual`.
    pub lifecycle: String,
    /// Memory limit in MiB.
    pub memory_mb: i64,
    /// Host port of its web page, if it has one.
    pub web_port: Option<i64>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms.
    pub updated_at: i64,
}

/// Fields for a new module.
#[derive(Debug, Clone)]
pub struct NewModule<'a> {
    /// The server.
    pub server_id: &'a str,
    /// The template.
    pub template: &'a str,
    /// Folder and container name.
    pub name: &'a str,
    /// Settings (JSON object).
    pub settings_json: &'a str,
    /// When it runs.
    pub lifecycle: &'a str,
    /// Memory limit in MiB.
    pub memory_mb: i64,
    /// Host port of its web page.
    pub web_port: Option<i64>,
}

impl Store {
    /// Adds a module. Returns it.
    pub async fn insert_module(&self, m: NewModule<'_>) -> Result<Module> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO server_modules (id, server_id, template, name, settings_json, lifecycle,
                                         memory_mb, web_port, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            id,
            m.server_id,
            m.template,
            m.name,
            m.settings_json,
            m.lifecycle,
            m.memory_mb,
            m.web_port,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(self.find_module(&id).await?.expect("just inserted"))
    }

    /// One module.
    pub async fn find_module(&self, id: &str) -> Result<Option<Module>> {
        let row = sqlx::query_as!(
            Module,
            r#"SELECT id AS "id!", server_id, template, name, settings_json, lifecycle, memory_mb,
                      web_port, created_at, updated_at
               FROM server_modules WHERE id = ?1"#,
            id,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Modules, by name; one server's, or every server's.
    pub async fn list_modules(&self, server_id: Option<&str>) -> Result<Vec<Module>> {
        let rows = sqlx::query_as!(
            Module,
            r#"SELECT id AS "id!", server_id, template, name, settings_json, lifecycle, memory_mb,
                      web_port, created_at, updated_at
               FROM server_modules WHERE ?1 IS NULL OR server_id = ?1
               ORDER BY server_id, name"#,
            server_id,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Changes a module's settings, lifecycle and memory. Returns whether it
    /// exists.
    pub async fn update_module(
        &self,
        id: &str,
        settings_json: &str,
        lifecycle: &str,
        memory_mb: i64,
    ) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE server_modules SET settings_json = ?2, lifecycle = ?3, memory_mb = ?4,
                                       updated_at = ?5
             WHERE id = ?1",
            id,
            settings_json,
            lifecycle,
            memory_mb,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Removes a module's record. Returns whether it existed.
    pub async fn delete_module(&self, id: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM server_modules WHERE id = ?1", id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Host ports modules' web pages hold.
    pub async fn used_module_ports(&self) -> Result<Vec<i64>> {
        let rows = sqlx::query_scalar!(
            r#"SELECT web_port AS "port!: i64" FROM server_modules WHERE web_port IS NOT NULL"#
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }
}
