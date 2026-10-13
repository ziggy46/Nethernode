//! Resumable uploads in flight, and their outcome.

use crate::{Result, Store, now_ms};

/// An upload's lifecycle.
pub mod status {
    /// Accepting chunks.
    pub const UPLOADING: &str = "uploading";
    /// All bytes are in; being verified and moved into place.
    pub const FINISHING: &str = "finishing";
    /// In place.
    pub const DONE: &str = "done";
    /// Couldn't be verified or placed.
    pub const FAILED: &str = "failed";
    /// Cancelled by the uploader.
    pub const CANCELLED: &str = "cancelled";
    /// Abandoned and cleaned up.
    pub const EXPIRED: &str = "expired";
}

/// One upload.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Upload {
    /// ULID; also the partial file's name.
    pub id: String,
    /// Who is uploading.
    pub user_id: String,
    /// Destination server.
    pub server_id: String,
    /// Destination folder, relative to the server root.
    pub dest_path: String,
    /// File name.
    pub filename: String,
    /// Final size in bytes.
    pub total_size: i64,
    /// Bytes received and synced.
    pub received: i64,
    /// Expected SHA-256 (hex).
    pub sha256: Option<String>,
    /// `rename`, `overwrite`, or `skip`.
    pub on_conflict: String,
    /// Extract a .zip once placed.
    pub extract: bool,
    /// See [`status`].
    pub status: String,
    /// Outcome, for display.
    pub message: Option<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms.
    pub updated_at: i64,
    /// Unix ms.
    pub expires_at: i64,
}

/// Fields for a new upload.
#[derive(Debug, Clone)]
pub struct NewUpload<'a> {
    /// ULID.
    pub id: &'a str,
    /// Who is uploading.
    pub user_id: &'a str,
    /// Destination server.
    pub server_id: &'a str,
    /// Destination folder.
    pub dest_path: &'a str,
    /// File name.
    pub filename: &'a str,
    /// Final size in bytes.
    pub total_size: i64,
    /// Expected SHA-256 (hex).
    pub sha256: Option<&'a str>,
    /// `rename`, `overwrite`, or `skip`.
    pub on_conflict: &'a str,
    /// Extract a .zip once placed.
    pub extract: bool,
    /// Unix ms.
    pub expires_at: i64,
}

impl Store {
    /// Records a new upload, status `uploading`.
    pub async fn insert_upload(&self, new: NewUpload<'_>) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO uploads (id, user_id, server_id, dest_path, filename, total_size, sha256,
                                  on_conflict, extract, status, created_at, updated_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'uploading', ?10, ?10, ?11)",
            new.id,
            new.user_id,
            new.server_id,
            new.dest_path,
            new.filename,
            new.total_size,
            new.sha256,
            new.on_conflict,
            new.extract,
            now,
            new.expires_at,
        )
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// One upload.
    pub async fn find_upload(&self, id: &str) -> Result<Option<Upload>> {
        let row = sqlx::query_as!(
            Upload,
            r#"SELECT id AS "id!", user_id, server_id, dest_path, filename, total_size, received,
                      sha256, on_conflict, extract AS "extract: bool", status, message,
                      created_at, updated_at, expires_at
               FROM uploads WHERE id = ?1"#,
            id
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    /// Records progress on an upload that's still `uploading`, moving its
    /// expiry along. Returns whether it was still uploading.
    pub async fn set_upload_received(
        &self,
        id: &str,
        received: i64,
        expires_at: i64,
    ) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE uploads SET received = ?2, updated_at = ?3, expires_at = ?4
             WHERE id = ?1 AND status = 'uploading'",
            id,
            received,
            now,
            expires_at,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Moves an upload from status `from` to `to`. Returns whether it was in
    /// `from` (so only one caller wins a transition).
    pub async fn transition_upload(
        &self,
        id: &str,
        from: &str,
        to: &str,
        message: Option<&str>,
    ) -> Result<bool> {
        let now = now_ms();
        let result = sqlx::query!(
            "UPDATE uploads SET status = ?3, message = COALESCE(?4, message), updated_at = ?5
             WHERE id = ?1 AND status = ?2",
            id,
            from,
            to,
            message,
            now,
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Bytes this user has reserved in unfinished uploads.
    pub async fn upload_bytes_in_flight(&self, user_id: &str) -> Result<i64> {
        let total = sqlx::query_scalar!(
            r#"SELECT COALESCE(SUM(total_size), 0) AS "total!: i64" FROM uploads
               WHERE user_id = ?1 AND status IN ('uploading', 'finishing')"#,
            user_id
        )
        .fetch_one(self.pool())
        .await?;
        Ok(total)
    }

    /// Unfinished uploads past their expiry.
    pub async fn expired_uploads(&self) -> Result<Vec<Upload>> {
        let now = now_ms();
        let rows = sqlx::query_as!(
            Upload,
            r#"SELECT id AS "id!", user_id, server_id, dest_path, filename, total_size, received,
                      sha256, on_conflict, extract AS "extract: bool", status, message,
                      created_at, updated_at, expires_at
               FROM uploads WHERE status = 'uploading' AND expires_at <= ?1"#,
            now
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// IDs of uploads whose partial file must be kept.
    pub async fn unfinished_upload_ids(&self) -> Result<Vec<String>> {
        let ids = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM uploads WHERE status IN ('uploading', 'finishing')"#
        )
        .fetch_all(self.pool())
        .await?;
        Ok(ids)
    }

    /// Uploads left `finishing` by a shutdown mid-placement.
    pub async fn finishing_uploads(&self) -> Result<Vec<Upload>> {
        let rows = sqlx::query_as!(
            Upload,
            r#"SELECT id AS "id!", user_id, server_id, dest_path, filename, total_size, received,
                      sha256, on_conflict, extract AS "extract: bool", status, message,
                      created_at, updated_at, expires_at
               FROM uploads WHERE status = 'finishing'"#
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// A user's unfinished uploads, newest first.
    pub async fn unfinished_uploads_for(&self, user_id: &str) -> Result<Vec<Upload>> {
        let rows = sqlx::query_as!(
            Upload,
            r#"SELECT id AS "id!", user_id, server_id, dest_path, filename, total_size, received,
                      sha256, on_conflict, extract AS "extract: bool", status, message,
                      created_at, updated_at, expires_at
               FROM uploads WHERE user_id = ?1 AND status IN ('uploading', 'finishing')
               ORDER BY created_at DESC"#,
            user_id
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    /// Deletes records of uploads that ended before `before` (unix ms).
    pub async fn prune_uploads(&self, before: i64) -> Result<u64> {
        let result = sqlx::query!(
            "DELETE FROM uploads
             WHERE status IN ('done', 'failed', 'cancelled', 'expired') AND updated_at < ?1",
            before
        )
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected())
    }
}
