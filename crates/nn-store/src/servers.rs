//! Minecraft servers managed by NetherNode.

use crate::{Result, Store, new_id, now_ms};

/// A managed server.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Server {
    /// ULID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// URL- and DNS-safe identifier, unique.
    pub slug: String,
    /// Lifecycle state: installing, stopped, starting, running, stopping, sleeping, crashed.
    pub status: String,
    /// Minecraft version, e.g. `1.21.4`.
    pub mc_version: String,
    /// Mod loader or server software, e.g. `paper`, `forge`, `vanilla`.
    pub loader: String,
    /// Loader version, if pinned.
    pub loader_version: Option<String>,
    /// Container image.
    pub image: String,
    /// Java major version, if pinned.
    pub java_version: Option<i64>,
    /// Memory limit in MiB.
    pub memory_mb: i64,
    /// CPU limit in cores.
    pub cpu_limit: Option<f64>,
    /// Port the server listens on at 127.0.0.1.
    pub internal_port: i64,
    /// Directly published port, if any (bypasses the proxy).
    pub public_port: Option<i64>,
    /// Primary hostname.
    pub hostname: Option<String>,
    /// Start when NetherNode starts.
    pub auto_start: bool,
    /// Stop after this many idle minutes.
    pub auto_stop_idle_minutes: Option<i64>,
    /// Sleep (and wake on connect) when no players are online.
    pub sleep_when_empty: bool,
    /// Encrypted RCON password.
    pub rcon_password_enc: Option<String>,
    /// Extra container environment, as a JSON object.
    pub env_json: String,
    /// Creation time, unix ms.
    pub created_at: i64,
    /// Last update, unix ms.
    pub updated_at: i64,
}

/// Fields needed to create a server.
#[derive(Debug, Clone)]
pub struct NewServer<'a> {
    /// Display name.
    pub name: &'a str,
    /// Unique slug.
    pub slug: &'a str,
    /// Initial status.
    pub status: &'a str,
    /// Minecraft version.
    pub mc_version: &'a str,
    /// Loader.
    pub loader: &'a str,
    /// Container image.
    pub image: &'a str,
    /// Memory limit in MiB.
    pub memory_mb: i64,
    /// Internal port.
    pub internal_port: i64,
    /// Sleep when empty (wake on connect).
    pub sleep_when_empty: bool,
}

impl Store {
    /// All servers, by name.
    pub async fn list_servers(&self) -> Result<Vec<Server>> {
        let rows = sqlx::query_as!(
            Server,
            r#"SELECT id AS "id!", name, slug, status, mc_version, loader, loader_version, image,
                      java_version, memory_mb, cpu_limit, internal_port, public_port, hostname,
                      auto_start AS "auto_start: bool", auto_stop_idle_minutes,
                      sleep_when_empty AS "sleep_when_empty: bool", rcon_password_enc, env_json,
                      created_at, updated_at
               FROM servers ORDER BY name COLLATE NOCASE, id"#,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// One server by ID.
    pub async fn find_server(&self, id: &str) -> Result<Option<Server>> {
        let row = sqlx::query_as!(
            Server,
            r#"SELECT id AS "id!", name, slug, status, mc_version, loader, loader_version, image,
                      java_version, memory_mb, cpu_limit, internal_port, public_port, hostname,
                      auto_start AS "auto_start: bool", auto_stop_idle_minutes,
                      sleep_when_empty AS "sleep_when_empty: bool", rcon_password_enc, env_json,
                      created_at, updated_at
               FROM servers WHERE id = ?1"#,
            id,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Creates a server row. Lifecycle (containers) is the orchestrator's job.
    pub async fn insert_server(&self, server: NewServer<'_>) -> Result<Server> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            r#"INSERT INTO servers (id, name, slug, status, mc_version, loader, image, memory_mb,
                                    internal_port, sleep_when_empty, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)"#,
            id,
            server.name,
            server.slug,
            server.status,
            server.mc_version,
            server.loader,
            server.image,
            server.memory_mb,
            server.internal_port,
            server.sleep_when_empty,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(self.find_server(&id).await?.expect("row just inserted"))
    }

    /// Updates a server's lifecycle status. Returns `false` if it doesn't exist.
    pub async fn set_server_status(&self, id: &str, status: &str) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE servers SET status = ?1, updated_at = ?2 WHERE id = ?3",
            status,
            now,
            id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
pub(crate) fn test_server<'a>(slug: &'a str, port: i64) -> NewServer<'a> {
    NewServer {
        name: slug,
        slug,
        status: "running",
        mc_version: "1.21.4",
        loader: "paper",
        image: "itzg/minecraft-server",
        memory_mb: 2048,
        internal_port: port,
        sleep_when_empty: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_store;

    #[tokio::test]
    async fn create_list_and_update_status() {
        let store = test_store().await;
        let a = store
            .insert_server(test_server("alpha", 30001))
            .await
            .unwrap();
        store
            .insert_server(test_server("beta", 30002))
            .await
            .unwrap();
        assert_eq!(store.list_servers().await.unwrap().len(), 2);
        assert!(store.set_server_status(&a.id, "sleeping").await.unwrap());
        assert_eq!(
            store.find_server(&a.id).await.unwrap().unwrap().status,
            "sleeping"
        );
        assert!(
            store.set_server_status(&a.id, "bogus").await.is_err(),
            "CHECK constraint"
        );
    }

    #[tokio::test]
    async fn slugs_and_internal_ports_are_unique() {
        let store = test_store().await;
        store
            .insert_server(test_server("alpha", 30001))
            .await
            .unwrap();
        assert!(
            store
                .insert_server(test_server("alpha", 30002))
                .await
                .is_err()
        );
        assert!(
            store
                .insert_server(test_server("beta", 30001))
                .await
                .is_err()
        );
    }
}
