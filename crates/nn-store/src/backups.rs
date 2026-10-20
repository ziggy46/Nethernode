//! Backup records and schedules.

use crate::{Result, Store, new_id, now_ms};

/// A backup archive.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Backup {
    /// ULID.
    pub id: String,
    /// The server, if it still exists.
    pub server_id: Option<String>,
    /// The server's name at the time.
    pub server_name: String,
    /// File name under `<data>/backups`.
    pub file_name: String,
    /// Archive size.
    pub size_bytes: i64,
    /// Files inside.
    pub files: i64,
    /// Their size before compression.
    pub raw_bytes: i64,
    /// `manual`, `scheduled`, or `pre-restore`.
    pub kind: String,
    /// `creating`, `ready`, or `failed`.
    pub status: String,
    /// Why it failed, or a note.
    pub message: Option<String>,
    /// Who made it.
    pub created_by: Option<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms.
    pub finished_at: Option<i64>,
    /// The node holding the archive ([`crate::LOCAL_NODE`] for this machine).
    pub node_id: String,
    /// The server's modpack (JSON) when it was made, if it was a modpack.
    pub modpack_json: Option<String>,
    /// The server's Minecraft version then, for modpacks.
    pub mc_version: Option<String>,
}

/// A scheduled action.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Schedule {
    /// ULID.
    pub id: String,
    /// The server.
    pub server_id: String,
    /// Display name.
    pub name: String,
    /// Cron expression.
    pub cron: String,
    /// IANA time zone the cron expression is in.
    pub timezone: String,
    /// `restart`, `backup`, `command`, `broadcast`, `start`, or `stop`.
    pub action: String,
    /// Action details (JSON).
    pub payload_json: String,
    /// Runs when due.
    pub enabled: bool,
    /// Next run, unix ms.
    pub next_run_at: Option<i64>,
    /// Last run, unix ms.
    pub last_run_at: Option<i64>,
    /// What happened last time.
    pub last_result: Option<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms.
    pub updated_at: i64,
}

/// Fields for a new or changed schedule.
#[derive(Debug, Clone)]
pub struct ScheduleInput<'a> {
    /// The server.
    pub server_id: &'a str,
    /// Display name.
    pub name: &'a str,
    /// Cron expression.
    pub cron: &'a str,
    /// IANA time zone.
    pub timezone: &'a str,
    /// Action.
    pub action: &'a str,
    /// Action details (JSON).
    pub payload_json: &'a str,
    /// Runs when due.
    pub enabled: bool,
    /// Next run, unix ms.
    pub next_run_at: Option<i64>,
}

impl Store {
    /// Records a backup being made. Returns its ID.
    pub async fn insert_backup(
        &self,
        server_id: &str,
        server_name: &str,
        file_name: &str,
        kind: &str,
        created_by: Option<&str>,
    ) -> Result<String> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO backups (id, server_id, server_name, file_name, kind, status, created_by,
                                  created_at, node_id, modpack_json, mc_version)
             SELECT ?1, ?2, ?3, ?4, ?5, 'creating', ?6, ?7, COALESCE(s.node_id, 'local'),
                    s.modpack_json, CASE WHEN s.modpack_json IS NULL THEN NULL ELSE s.mc_version END
             FROM (SELECT 1) LEFT JOIN servers s ON s.id = ?2",
            id,
            server_id,
            server_name,
            file_name,
            kind,
            created_by,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(id)
    }

    /// Labels a backup (kept when it finishes, unless it fails).
    pub async fn set_backup_message(&self, id: &str, message: &str) -> Result<()> {
        sqlx::query!("UPDATE backups SET message = ?2 WHERE id = ?1", id, message)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Records how a backup turned out. A `None` message keeps any label.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish_backup(
        &self,
        id: &str,
        status: &str,
        size_bytes: i64,
        files: i64,
        raw_bytes: i64,
        message: Option<&str>,
    ) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            "UPDATE backups SET status = ?2, size_bytes = ?3, files = ?4, raw_bytes = ?5,
                                message = COALESCE(?6, message), finished_at = ?7
             WHERE id = ?1",
            id,
            status,
            size_bytes,
            files,
            raw_bytes,
            message,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Backups, newest first; all of them, or one server's.
    pub async fn list_backups(&self, server_id: Option<&str>) -> Result<Vec<Backup>> {
        let rows = sqlx::query_as!(
            Backup,
            r#"SELECT id AS "id!", server_id, server_name, file_name, size_bytes, files, raw_bytes, kind,
                      status, message, created_by, created_at, finished_at, node_id, modpack_json,
                      mc_version
               FROM backups WHERE ?1 IS NULL OR server_id = ?1
               ORDER BY created_at DESC"#,
            server_id,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// One backup.
    pub async fn find_backup(&self, id: &str) -> Result<Option<Backup>> {
        let row = sqlx::query_as!(
            Backup,
            r#"SELECT id AS "id!", server_id, server_name, file_name, size_bytes, files, raw_bytes, kind,
                      status, message, created_by, created_at, finished_at, node_id, modpack_json,
                      mc_version
               FROM backups WHERE id = ?1"#,
            id,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Forgets a backup record. Returns whether it existed.
    pub async fn delete_backup(&self, id: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM backups WHERE id = ?1", id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Forgets the records of backups kept on a node. Returns how many.
    pub async fn delete_backups_on_node(&self, node_id: &str) -> Result<u64> {
        let result = sqlx::query!("DELETE FROM backups WHERE node_id = ?1", node_id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected())
    }

    /// Marks backups left `creating` by a shutdown as failed. Returns their
    /// file names, to clean up.
    pub async fn fail_unfinished_backups(&self) -> Result<Vec<String>> {
        let names =
            sqlx::query_scalar!(r#"SELECT file_name FROM backups WHERE status = 'creating'"#)
                .fetch_all(self.pool())
                .await?;
        let now = now_ms();
        sqlx::query!(
            "UPDATE backups SET status = 'failed', message = 'NetherNode stopped while it was being made.',
                                finished_at = ?1
             WHERE status = 'creating'",
            now
        )
        .execute(self.pool())
        .await?;
        Ok(names)
    }

    /// Creates a schedule. Returns it.
    pub async fn insert_schedule(&self, s: ScheduleInput<'_>) -> Result<Schedule> {
        let id = new_id();
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO schedules (id, server_id, name, cron, timezone, action, payload_json, enabled,
                                    next_run_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            id,
            s.server_id,
            s.name,
            s.cron,
            s.timezone,
            s.action,
            s.payload_json,
            s.enabled,
            s.next_run_at,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(self.find_schedule(&id).await?.expect("row just inserted"))
    }

    /// Replaces a schedule's settings. Returns whether it exists.
    pub async fn update_schedule(&self, id: &str, s: ScheduleInput<'_>) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE schedules SET name = ?2, cron = ?3, timezone = ?4, action = ?5, payload_json = ?6,
                                  enabled = ?7, next_run_at = ?8, updated_at = ?9
             WHERE id = ?1 AND server_id = ?10",
            id,
            s.name,
            s.cron,
            s.timezone,
            s.action,
            s.payload_json,
            s.enabled,
            s.next_run_at,
            now,
            s.server_id,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Deletes a schedule. Returns whether it existed.
    pub async fn delete_schedule(&self, id: &str) -> Result<bool> {
        let result = sqlx::query!("DELETE FROM schedules WHERE id = ?1", id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() == 1)
    }

    /// One schedule.
    pub async fn find_schedule(&self, id: &str) -> Result<Option<Schedule>> {
        let row = sqlx::query_as!(
            Schedule,
            r#"SELECT id AS "id!", server_id, name, cron, timezone, action, payload_json,
                      enabled AS "enabled: bool", next_run_at, last_run_at, last_result, created_at, updated_at
               FROM schedules WHERE id = ?1"#,
            id,
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Schedules; all of them, or one server's.
    pub async fn list_schedules(&self, server_id: Option<&str>) -> Result<Vec<Schedule>> {
        let rows = sqlx::query_as!(
            Schedule,
            r#"SELECT id AS "id!", server_id, name, cron, timezone, action, payload_json,
                      enabled AS "enabled: bool", next_run_at, last_run_at, last_result, created_at, updated_at
               FROM schedules WHERE ?1 IS NULL OR server_id = ?1
               ORDER BY next_run_at IS NULL, next_run_at, name"#,
            server_id,
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Sets when a schedule next runs.
    pub async fn set_schedule_next(&self, id: &str, next_run_at: Option<i64>) -> Result<()> {
        sqlx::query!(
            "UPDATE schedules SET next_run_at = ?2 WHERE id = ?1",
            id,
            next_run_at
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Records a run and when the next one is.
    pub async fn record_schedule_run(
        &self,
        id: &str,
        ran_at: i64,
        result: &str,
        next_run_at: Option<i64>,
    ) -> Result<()> {
        sqlx::query!(
            "UPDATE schedules SET last_run_at = ?2, last_result = ?3, next_run_at = ?4 WHERE id = ?1",
            id,
            ran_at,
            result,
            next_run_at,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
