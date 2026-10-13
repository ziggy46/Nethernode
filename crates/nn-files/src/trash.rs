//! Deleting moves items into the server's `.trash` folder so they can be
//! restored. Each deletion gets an ID: the item lives at
//! `.trash/<id>/<name>` and its record at `.trash/<id>.json`. The trash is
//! hidden from listings and unreachable through [`RelPath`]; old items are
//! purged by [`ServerFiles::purge_expired`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{FileError, Result};
use crate::path::{RelPath, TRASH_DIR};
use crate::sandbox::{EntryKind, ServerFiles, split};

/// How long deleted items are kept by default.
pub const TRASH_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// A deleted item.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrashItem {
    /// ID for restoring or purging.
    pub id: String,
    /// Where it was.
    pub original: String,
    /// What it was.
    pub kind: EntryKind,
    /// Size in bytes, for files.
    pub size: Option<u64>,
    /// When it was deleted, in Unix milliseconds.
    pub deleted_ms: i64,
}

impl ServerFiles {
    /// Moves an item to the trash and returns its trash ID.
    pub fn delete(&self, path: &RelPath) -> Result<TrashItem> {
        let (_, name) = split(path)?;
        let meta = self
            .dir()
            .symlink_metadata(path.as_path())
            .map_err(|e| FileError::at(path.as_str(), e))?;
        self.ensure_trash()?;

        let id = ulid::Ulid::generate().to_string();
        let slot = trash_path(&id);
        let at = |e| FileError::at(path.as_str(), e);
        self.dir().create_dir(&slot).map_err(at)?;
        self.owner().created(self.dir(), &slot).map_err(at)?;
        if let Err(e) = self
            .dir()
            .rename(path.as_path(), self.dir(), slot.join(name))
        {
            let _ = self.dir().remove_dir(&slot);
            return Err(at(e));
        }
        let kind = if meta.is_symlink() {
            EntryKind::Symlink
        } else if meta.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        let item = TrashItem {
            id: id.clone(),
            original: path.as_str().to_owned(),
            kind,
            size: (kind == EntryKind::File).then(|| meta.len()),
            deleted_ms: now_ms(),
        };
        let record = serde_json::to_vec(&item).map_err(io::Error::other)?;
        self.dir().write(record_path(&id), record).map_err(at)?;
        self.owner()
            .created(self.dir(), &record_path(&id))
            .map_err(at)?;
        Ok(item)
    }

    /// Deleted items, newest first.
    pub fn trash(&self) -> Result<Vec<TrashItem>> {
        let entries = match self.dir().read_dir(TRASH_DIR) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(FileError::at(TRASH_DIR, e)),
        };
        let mut items = Vec::new();
        for entry in entries {
            let name = entry?.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
                continue;
            };
            if let Ok(item) = self.record(id) {
                items.push(item);
            }
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.deleted_ms));
        Ok(items)
    }

    /// Puts an item back where it was, recreating missing parent folders.
    /// Fails if something new now occupies that path.
    pub fn restore(&self, id: &str) -> Result<RelPath> {
        let item = self.record(id)?;
        let original = RelPath::parse(&item.original)?;
        let (parent, name) = split(&original)?;
        match self.dir().symlink_metadata(original.as_path()) {
            Ok(_) => return Err(FileError::AlreadyExists(original.to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(FileError::at(original.as_str(), e)),
        }
        self.ensure_dirs(&parent)?;
        self.dir()
            .rename(trash_path(id).join(name), self.dir(), original.as_path())
            .map_err(|e| FileError::at(original.as_str(), e))?;
        self.purge(id)?;
        Ok(original)
    }

    /// Permanently deletes one trashed item.
    pub fn purge(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        match self.dir().remove_dir_all(trash_path(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(FileError::at(TRASH_DIR, e)),
        }
        match self.dir().remove_file(record_path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(FileError::at(TRASH_DIR, e)),
        }
    }

    /// Permanently deletes everything in the trash.
    pub fn empty_trash(&self) -> Result<()> {
        match self.dir().remove_dir_all(TRASH_DIR) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(FileError::at(TRASH_DIR, e)),
        }
    }

    /// Purges items deleted more than `age` ago. Returns how many.
    pub fn purge_expired(&self, age: Duration) -> Result<usize> {
        let cutoff = now_ms() - i64::try_from(age.as_millis()).unwrap_or(i64::MAX);
        let mut purged = 0;
        for item in self.trash()? {
            if item.deleted_ms < cutoff {
                self.purge(&item.id)?;
                purged += 1;
            }
        }
        Ok(purged)
    }

    fn record(&self, id: &str) -> Result<TrashItem> {
        validate_id(id)?;
        let bytes = self.dir().read(record_path(id)).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                FileError::NotFound("that item in the trash".into())
            } else {
                FileError::at(TRASH_DIR, e)
            }
        })?;
        let item: TrashItem = serde_json::from_slice(&bytes)
            .map_err(|_| FileError::Invalid("the trash record is damaged".into()))?;
        if item.id != id {
            return Err(FileError::Invalid("the trash record is damaged".into()));
        }
        Ok(item)
    }

    fn ensure_trash(&self) -> Result<()> {
        match self.dir().create_dir(TRASH_DIR) {
            Ok(()) => self
                .owner()
                .created(self.dir(), Path::new(TRASH_DIR))
                .map_err(|e| FileError::at(TRASH_DIR, e)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(FileError::at(TRASH_DIR, e)),
        }
    }
}

fn validate_id(id: &str) -> Result<()> {
    // ULIDs only, so an ID can never be a path.
    if id.len() == 26 && ulid::Ulid::from_string(id).is_ok() {
        Ok(())
    } else {
        Err(FileError::InvalidPath("not a trash item ID"))
    }
}

fn trash_path(id: &str) -> PathBuf {
    Path::new(TRASH_DIR).join(id)
}

fn record_path(id: &str) -> PathBuf {
    Path::new(TRASH_DIR).join(format!("{id}.json"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
