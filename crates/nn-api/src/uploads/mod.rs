//! Resumable uploads (tus 1.0) into server folders.
//!
//! - `OPTIONS /api/uploads` describes what's supported.
//! - `POST /api/uploads` starts an upload. Its `Upload-Metadata` names the
//!   server, folder, and file; all are checked here, before any bytes move.
//! - `HEAD /api/uploads/{id}` reports how much has arrived.
//! - `PATCH /api/uploads/{id}` appends one chunk (at most the configured
//!   chunk size, well under Cloudflare's per-request limit).
//! - `DELETE /api/uploads/{id}` cancels.
//! - `GET /api/uploads` lists the caller's unfinished uploads.
//!
//! Once every byte is in, the file is verified (when a SHA-256 was given)
//! and moved into place in the background; `upload_finished` reports how
//! that went.

mod finish;
pub(crate) mod http;
pub mod tasks;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nn_uploads::TempStore;

pub(crate) use http::{cancel, create, describe, list, offset, patch};

/// Upload limits.
#[derive(Debug, Clone)]
pub struct UploadSettings {
    /// Largest `PATCH` body accepted.
    pub chunk_limit: u64,
    /// Largest file accepted.
    pub max_file: u64,
    /// Most bytes one user may have in unfinished uploads at once.
    pub quota: u64,
    /// How long an upload may sit without progress before it's discarded.
    pub expiry: Duration,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self {
            chunk_limit: 16 * 1024 * 1024,
            max_file: 50 * 1024 * 1024 * 1024,
            quota: 100 * 1024 * 1024 * 1024,
            expiry: Duration::from_secs(24 * 3600),
        }
    }
}

/// Upload state shared by the handlers and background tasks.
#[derive(Default)]
pub struct Uploads {
    settings: UploadSettings,
    /// Where partial uploads live; `None` when the data folder isn't
    /// available (uploads are refused).
    temp: Option<TempStore>,
    /// One writer per upload at a time.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Uploads {
    /// Uploads stored under `temp`.
    pub fn new(settings: UploadSettings, temp: TempStore) -> Self {
        Self {
            settings,
            temp: Some(temp),
            locks: Mutex::default(),
        }
    }

    /// The limits in force.
    pub fn settings(&self) -> &UploadSettings {
        &self.settings
    }

    pub(crate) fn temp(&self) -> Option<&TempStore> {
        self.temp.as_ref()
    }

    /// Takes upload `id`'s write lock, or `None` if a request holds it.
    pub(crate) fn try_lock(&self, id: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let lock = self
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_owned())
            .or_default()
            .clone();
        lock.try_lock_owned().ok()
    }

    /// Forgets upload `id`'s lock once it can't be written any more.
    pub(crate) fn forget(&self, id: &str) {
        self.locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    fn expiry_ms(&self) -> i64 {
        i64::try_from(self.settings.expiry.as_millis()).unwrap_or(i64::MAX)
    }
}
