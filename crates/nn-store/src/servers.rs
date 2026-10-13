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
    /// Minecraft version, e.g. `1.21.4` or `LATEST`.
    pub mc_version: String,
    /// Server software: vanilla, paper, purpur, fabric, forge, neoforge, quilt.
    pub loader: String,
    /// Loader or build version, if pinned.
    pub loader_version: Option<String>,
    /// Container image, including tag.
    pub image: String,
    /// Java major version, if pinned by the admin.
    pub java_version: Option<i64>,
    /// Java heap in MiB.
    pub memory_mb: i64,
    /// CPU limit in cores.
    pub cpu_limit: Option<f64>,
    /// Host port (127.0.0.1) the game listens on.
    pub internal_port: i64,
    /// Directly published port, if any (bypasses the proxy).
    pub public_port: Option<i64>,
    /// Primary hostname.
    pub hostname: Option<String>,
    /// Start when NetherNode starts.
    pub auto_start: bool,
    /// Sleep after this many minutes with no players (when `sleep_when_empty`).
    pub auto_stop_idle_minutes: Option<i64>,
    /// Sleep when empty, and wake when a player connects.
    pub sleep_when_empty: bool,
    /// Encrypted RCON password.
    pub rcon_password_enc: Option<String>,
    /// Extra container environment, as a JSON object.
    pub env_json: String,
    /// Creation time, unix ms.
    pub created_at: i64,
    /// Last update, unix ms.
    pub updated_at: i64,
    /// Host port (127.0.0.1) for RCON.
    pub rcon_port: Option<i64>,
    /// Docker container ID, once created.
    pub container_id: Option<String>,
    /// When the Minecraft EULA was accepted, unix ms.
    pub eula_accepted_at: Option<i64>,
    /// Card artwork: nether, overworld, end, sky.
    pub card_theme: String,
    /// Restart automatically after a crash (at most 3 times in 10 minutes).
    pub restart_on_crash: bool,
    /// Use Aikar's GC-tuned JVM flags.
    pub use_aikar_flags: bool,
    /// Server list message.
    pub motd: Option<String>,
    /// Player limit.
    pub max_players: i64,
    /// Last time the server reached running, unix ms.
    pub last_started_at: Option<i64>,
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
    /// Java heap in MiB.
    pub memory_mb: i64,
    /// Internal (game) port.
    pub internal_port: i64,
    /// Sleep when empty (wake on connect).
    pub sleep_when_empty: bool,
}

/// Everything about a server's configuration that can change after creation.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSettings {
    /// Display name.
    pub name: String,
    /// Minecraft version.
    pub mc_version: String,
    /// Loader.
    pub loader: String,
    /// Loader or build version.
    pub loader_version: Option<String>,
    /// Container image, including tag.
    pub image: String,
    /// Java major version override.
    pub java_version: Option<i64>,
    /// Java heap in MiB.
    pub memory_mb: i64,
    /// CPU limit in cores.
    pub cpu_limit: Option<f64>,
    /// Start with NetherNode.
    pub auto_start: bool,
    /// Sleep when empty.
    pub sleep_when_empty: bool,
    /// Minutes empty before sleeping.
    pub auto_stop_idle_minutes: Option<i64>,
    /// Auto-restart after crashes.
    pub restart_on_crash: bool,
    /// Aikar's JVM flags.
    pub use_aikar_flags: bool,
    /// Server list message.
    pub motd: Option<String>,
    /// Player limit.
    pub max_players: i64,
    /// Card artwork.
    pub card_theme: String,
}

impl Server {
    /// The server's current settings.
    pub fn settings(&self) -> ServerSettings {
        ServerSettings {
            name: self.name.clone(),
            mc_version: self.mc_version.clone(),
            loader: self.loader.clone(),
            loader_version: self.loader_version.clone(),
            image: self.image.clone(),
            java_version: self.java_version,
            memory_mb: self.memory_mb,
            cpu_limit: self.cpu_limit,
            auto_start: self.auto_start,
            sleep_when_empty: self.sleep_when_empty,
            auto_stop_idle_minutes: self.auto_stop_idle_minutes,
            restart_on_crash: self.restart_on_crash,
            use_aikar_flags: self.use_aikar_flags,
            motd: self.motd.clone(),
            max_players: self.max_players,
            card_theme: self.card_theme.clone(),
        }
    }
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
                      created_at, updated_at, rcon_port, container_id, eula_accepted_at, card_theme,
                      restart_on_crash AS "restart_on_crash: bool",
                      use_aikar_flags AS "use_aikar_flags: bool", motd, max_players, last_started_at
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
                      created_at, updated_at, rcon_port, container_id, eula_accepted_at, card_theme,
                      restart_on_crash AS "restart_on_crash: bool",
                      use_aikar_flags AS "use_aikar_flags: bool", motd, max_players, last_started_at
               FROM servers WHERE id = ?1"#,
            id,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Creates a minimal server row (used by tests and fixtures). The
    /// orchestrator uses [`Store::insert_managed_server`].
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

    /// Creates a server with full settings, ports, and a sealed RCON password.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_managed_server(
        &self,
        slug: &str,
        status: &str,
        settings: &ServerSettings,
        internal_port: i64,
        rcon_port: i64,
        rcon_password_enc: &str,
        eula_accepted_at: i64,
    ) -> Result<Server> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            r#"INSERT INTO servers (id, name, slug, status, mc_version, loader, loader_version, image,
                                    java_version, memory_mb, cpu_limit, internal_port, rcon_port,
                                    auto_start, sleep_when_empty, auto_stop_idle_minutes,
                                    restart_on_crash, use_aikar_flags, motd, max_players, card_theme,
                                    rcon_password_enc, eula_accepted_at, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                       ?19, ?20, ?21, ?22, ?23, ?24, ?24)"#,
            id,
            settings.name,
            slug,
            status,
            settings.mc_version,
            settings.loader,
            settings.loader_version,
            settings.image,
            settings.java_version,
            settings.memory_mb,
            settings.cpu_limit,
            internal_port,
            rcon_port,
            settings.auto_start,
            settings.sleep_when_empty,
            settings.auto_stop_idle_minutes,
            settings.restart_on_crash,
            settings.use_aikar_flags,
            settings.motd,
            settings.max_players,
            settings.card_theme,
            rcon_password_enc,
            eula_accepted_at,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(self.find_server(&id).await?.expect("row just inserted"))
    }

    /// Replaces a server's settings. Returns `false` if it doesn't exist.
    pub async fn update_server_settings(&self, id: &str, s: &ServerSettings) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            r#"UPDATE servers SET name = ?1, mc_version = ?2, loader = ?3, loader_version = ?4,
                   image = ?5, java_version = ?6, memory_mb = ?7, cpu_limit = ?8, auto_start = ?9,
                   sleep_when_empty = ?10, auto_stop_idle_minutes = ?11, restart_on_crash = ?12,
                   use_aikar_flags = ?13, motd = ?14, max_players = ?15, card_theme = ?16,
                   updated_at = ?17
               WHERE id = ?18"#,
            s.name,
            s.mc_version,
            s.loader,
            s.loader_version,
            s.image,
            s.java_version,
            s.memory_mb,
            s.cpu_limit,
            s.auto_start,
            s.sleep_when_empty,
            s.auto_stop_idle_minutes,
            s.restart_on_crash,
            s.use_aikar_flags,
            s.motd,
            s.max_players,
            s.card_theme,
            now,
            id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Updates a server's lifecycle status. Returns `false` if it doesn't
    /// exist or already had that status.
    pub async fn set_server_status(&self, id: &str, status: &str) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE servers SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status <> ?1",
            status,
            now,
            id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Records (or clears) the server's container ID.
    pub async fn set_container_id(&self, id: &str, container_id: Option<&str>) -> Result<()> {
        sqlx::query!(
            "UPDATE servers SET container_id = ?1 WHERE id = ?2",
            container_id,
            id
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Records when the server last reached running.
    pub async fn mark_server_started(&self, id: &str, at_ms: i64) -> Result<()> {
        sqlx::query!(
            "UPDATE servers SET last_started_at = ?1 WHERE id = ?2",
            at_ms,
            id
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Every host port assigned to a server (game and RCON).
    pub async fn used_ports(&self) -> Result<Vec<i64>> {
        let rows = sqlx::query!(
            r#"SELECT internal_port AS "port!: i64" FROM servers
               UNION SELECT rcon_port AS "port!: i64" FROM servers WHERE rcon_port IS NOT NULL"#
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(|r| r.port).collect())
    }

    /// Whether a slug is already taken.
    pub async fn slug_exists(&self, slug: &str) -> Result<bool> {
        let count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM servers WHERE slug = ?1"#,
            slug
        )
        .fetch_one(self.pool())
        .await?;
        Ok(count > 0)
    }

    /// Deletes a server row (routes and sessions cascade). Returns `false` if missing.
    pub async fn delete_server(&self, id: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM servers WHERE id = ?1", id)
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

    fn settings(name: &str) -> ServerSettings {
        ServerSettings {
            name: name.into(),
            mc_version: "1.21.4".into(),
            loader: "paper".into(),
            loader_version: None,
            image: "itzg/minecraft-server:java21".into(),
            java_version: None,
            memory_mb: 2048,
            cpu_limit: None,
            auto_start: false,
            sleep_when_empty: true,
            auto_stop_idle_minutes: Some(10),
            restart_on_crash: true,
            use_aikar_flags: true,
            motd: Some("hi".into()),
            max_players: 20,
            card_theme: "nether".into(),
        }
    }

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
        assert!(
            !store.set_server_status(&a.id, "sleeping").await.unwrap(),
            "no change"
        );
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
        assert!(store.slug_exists("alpha").await.unwrap());
        assert!(!store.slug_exists("gamma").await.unwrap());
    }

    #[tokio::test]
    async fn managed_servers_round_trip_settings() {
        let store = test_store().await;
        let s = store
            .insert_managed_server(
                "smp",
                "installing",
                &settings("SMP"),
                30000,
                30001,
                "v1:sealed",
                42,
            )
            .await
            .unwrap();
        assert_eq!(s.settings(), settings("SMP"));
        assert_eq!((s.rcon_port, s.eula_accepted_at), (Some(30001), Some(42)));
        let mut used = store.used_ports().await.unwrap();
        used.sort();
        assert_eq!(used, [30000, 30001]);

        let mut changed = settings("Renamed");
        changed.memory_mb = 4096;
        changed.card_theme = "end".into();
        assert!(store.update_server_settings(&s.id, &changed).await.unwrap());
        assert_eq!(
            store.find_server(&s.id).await.unwrap().unwrap().settings(),
            changed
        );

        let mut bad = settings("x");
        bad.card_theme = "lava".into();
        assert!(
            store.update_server_settings(&s.id, &bad).await.is_err(),
            "theme CHECK"
        );

        store.set_container_id(&s.id, Some("abc123")).await.unwrap();
        store.mark_server_started(&s.id, 99).await.unwrap();
        let s = store.find_server(&s.id).await.unwrap().unwrap();
        assert_eq!(
            (s.container_id.as_deref(), s.last_started_at),
            (Some("abc123"), Some(99))
        );
        assert!(store.delete_server(&s.id).await.unwrap());
        assert!(store.find_server(&s.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rcon_ports_are_unique() {
        let store = test_store().await;
        store
            .insert_managed_server("a", "stopped", &settings("A"), 30000, 30001, "x", 1)
            .await
            .unwrap();
        let clash = store
            .insert_managed_server("b", "stopped", &settings("B"), 30002, 30001, "x", 1)
            .await;
        assert!(clash.is_err());
    }
}
