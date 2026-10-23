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
    /// Its own address: a prefix for the server's address (`map`), or a
    /// whole hostname (with a dot).
    pub web_host: Option<String>,
    /// Anyone can open its web page at that address.
    pub web_public: bool,
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
    /// Its own address (see [`Module::web_host`]).
    pub web_host: Option<&'a str>,
}

impl Store {
    /// Adds a module. Returns it.
    pub async fn insert_module(&self, m: NewModule<'_>) -> Result<Module> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO server_modules (id, server_id, template, name, settings_json, lifecycle,
                                         memory_mb, web_port, web_host, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?10, ?9, ?9)",
            id,
            m.server_id,
            m.template,
            m.name,
            m.settings_json,
            m.lifecycle,
            m.memory_mb,
            m.web_port,
            now,
            m.web_host,
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
                      web_port, web_host, web_public AS "web_public: bool", created_at, updated_at
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
                      web_port, web_host, web_public AS "web_public: bool", created_at, updated_at
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

    /// Sets a module's own address and whether anyone can open it. Returns
    /// whether it exists.
    pub async fn set_module_site(
        &self,
        id: &str,
        web_host: Option<&str>,
        web_public: bool,
    ) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE server_modules SET web_host = ?2, web_public = ?3, updated_at = ?4 WHERE id = ?1",
            id,
            web_host,
            web_public,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Keeps the panel's copy of a module on another machine (its agent
    /// has the record): adds it with the same ID, or brings it up to date,
    /// keeping the panel's own address settings (`web_host`, `web_public`).
    pub async fn upsert_module_mirror(&self, m: &Module) -> Result<()> {
        // Runtime-checked rather than query!, so adding it needed no
        // offline query data.
        sqlx::query(
            "INSERT INTO server_modules (id, server_id, template, name, settings_json, lifecycle,
                                         memory_mb, web_port, web_host, web_public, created_at,
                                         updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11)
             ON CONFLICT (id) DO UPDATE SET
                 template = excluded.template, name = excluded.name,
                 settings_json = excluded.settings_json, lifecycle = excluded.lifecycle,
                 memory_mb = excluded.memory_mb, web_port = excluded.web_port,
                 updated_at = excluded.updated_at",
        )
        .bind(&m.id)
        .bind(&m.server_id)
        .bind(&m.template)
        .bind(&m.name)
        .bind(&m.settings_json)
        .bind(&m.lifecycle)
        .bind(m.memory_mb)
        .bind(m.web_port)
        .bind(&m.web_host)
        .bind(m.created_at)
        .bind(m.updated_at)
        .execute(self.pool())
        .await?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use crate::{NewServer, test_store};

    #[tokio::test]
    async fn mirrors_keep_the_panels_address_settings() {
        let store = test_store().await;
        let server = store
            .insert_server(NewServer {
                name: "mc2",
                slug: "mc2",
                status: "stopped",
                mc_version: "1.21.4",
                loader: "fabric",
                image: "itzg/minecraft-server",
                memory_mb: 2048,
                internal_port: 30_002,
                sleep_when_empty: false,
            })
            .await
            .unwrap();
        // As the agent reports it.
        let mut module = crate::Module {
            id: "01MODULEONANOTHERMACHINE".into(),
            server_id: server.id.clone(),
            template: "bluemap".into(),
            name: "bluemap".into(),
            settings_json: "{}".into(),
            lifecycle: "always".into(),
            memory_mb: 2048,
            web_port: Some(8100),
            web_host: Some("map".into()),
            web_public: false,
            created_at: 1,
            updated_at: 1,
        };
        store.upsert_module_mirror(&module).await.unwrap();
        store
            .set_module_site(&module.id, Some("map"), true)
            .await
            .unwrap();
        module.memory_mb = 4096;
        module.updated_at = 2;
        store.upsert_module_mirror(&module).await.unwrap();
        let now = store.find_module(&module.id).await.unwrap().unwrap();
        assert_eq!(now.memory_mb, 4096);
        assert!(now.web_public, "the panel's own setting stays");
        assert_eq!(store.list_modules(Some(&server.id)).await.unwrap().len(), 1);
    }
}
