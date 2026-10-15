//! A server's folder on its node.

use std::io::SeekFrom;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use nn_catalog::jar::{self, ModInfo};
use nn_events::EventBus;
use nn_files::{
    Adopted, CopyReport, Entry, Expect, FileContent, FileError, Limits, OnConflict, RelPath,
    ServerFiles, TrashItem,
};
use tokio::io::{AsyncRead, AsyncSeekExt};

use crate::jobs;

/// A server's files, reached through its node. Paths are relative to the
/// server's folder, which nothing can leave (links included).
#[derive(Clone)]
pub struct Folder {
    server_id: String,
    events: EventBus,
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Local(Arc<ServerFiles>),
}

impl Folder {
    pub(crate) fn local(server_id: String, events: EventBus, files: ServerFiles) -> Self {
        Self {
            server_id,
            events,
            kind: Kind::Local(Arc::new(files)),
        }
    }

    /// Runs blocking file work off the async runtime.
    async fn run<T, F>(&self, work: F) -> Result<T, FileError>
    where
        F: FnOnce(&ServerFiles) -> Result<T, FileError> + Send + 'static,
        T: Send + 'static,
    {
        let Kind::Local(files) = &self.kind;
        let files = files.clone();
        match tokio::task::spawn_blocking(move || work(&files)).await {
            Ok(result) => result,
            Err(e) => Err(FileError::Io(std::io::Error::other(format!(
                "file task failed: {e}"
            )))),
        }
    }

    /// The folder's entries.
    pub async fn list(&self, path: &RelPath) -> Result<Vec<Entry>, FileError> {
        let path = path.clone();
        self.run(move |f| f.list(&path)).await
    }

    /// One entry.
    pub async fn stat(&self, path: &RelPath) -> Result<Entry, FileError> {
        let path = path.clone();
        self.run(move |f| f.stat(&path)).await
    }

    /// A whole file, up to `limit` bytes.
    pub async fn read(&self, path: &RelPath, limit: u64) -> Result<FileContent, FileError> {
        let path = path.clone();
        self.run(move |f| f.read(&path, limit)).await
    }

    /// A file, or `None` if it isn't there.
    pub async fn read_if_exists(
        &self,
        path: &RelPath,
        limit: u64,
    ) -> Result<Option<FileContent>, FileError> {
        match self.read(path, limit).await {
            Ok(content) => Ok(Some(content)),
            Err(FileError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A file to stream, of any size.
    pub async fn open_read(&self, path: &RelPath) -> Result<FileReader, FileError> {
        let path = path.clone();
        let (file, size) = self.run(move |f| f.open_read(&path)).await?;
        Ok(FileReader::local(file, size))
    }

    /// Writes a file atomically if it's still what `expect` says. Returns
    /// the new version.
    pub async fn write(
        &self,
        path: &RelPath,
        data: Vec<u8>,
        expect: Expect,
    ) -> Result<String, FileError> {
        let path = path.clone();
        self.run(move |f| f.write(&path, &data, expect)).await
    }

    /// Makes a folder.
    pub async fn mkdir(&self, path: &RelPath) -> Result<(), FileError> {
        let path = path.clone();
        self.run(move |f| f.mkdir(&path)).await
    }

    /// Makes a folder unless it's already there.
    pub async fn ensure_dir(&self, path: &RelPath) -> Result<(), FileError> {
        match self.mkdir(path).await {
            Err(FileError::AlreadyExists(_)) | Ok(()) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Moves or renames.
    pub async fn rename(&self, from: &RelPath, to: &RelPath) -> Result<(), FileError> {
        let (from, to) = (from.clone(), to.clone());
        self.run(move |f| f.rename(&from, &to)).await
    }

    /// Copies a file or folder.
    pub async fn copy(&self, from: &RelPath, to: &RelPath) -> Result<CopyReport, FileError> {
        let (from, to) = (from.clone(), to.clone());
        self.run(move |f| f.copy(&from, &to)).await
    }

    /// Moves files to the trash, after clearing out anything older than
    /// `retention`.
    pub async fn delete(
        &self,
        paths: Vec<RelPath>,
        retention: Duration,
    ) -> Result<Vec<TrashItem>, FileError> {
        self.run(move |f| {
            f.purge_expired(retention)?;
            paths.iter().map(|p| f.delete(p)).collect()
        })
        .await
    }

    /// What's in the trash, after clearing out anything older than
    /// `retention`.
    pub async fn trash(&self, retention: Duration) -> Result<Vec<TrashItem>, FileError> {
        self.run(move |f| {
            f.purge_expired(retention)?;
            f.trash()
        })
        .await
    }

    /// Puts a trashed item back. Returns where it went.
    pub async fn restore(&self, id: &str) -> Result<RelPath, FileError> {
        let id = id.to_owned();
        self.run(move |f| f.restore(&id)).await
    }

    /// Deletes one trashed item for good, or with `None` all of them.
    pub async fn purge(&self, id: Option<&str>) -> Result<(), FileError> {
        let id = id.map(str::to_owned);
        self.run(move |f| match id {
            Some(id) => f.purge(&id),
            None => f.empty_trash(),
        })
        .await
    }

    /// A file's SHA-1, hex encoded.
    pub async fn sha1(&self, path: &RelPath) -> Result<String, FileError> {
        let path = path.clone();
        self.run(move |f| {
            let (file, _) = f.open_read(&path)?;
            Ok(jar::sha1_hex(std::io::BufReader::new(file))?)
        })
        .await
    }

    /// What a mod jar says about itself, if it says anything.
    pub async fn mod_info(&self, path: &RelPath) -> Result<Option<ModInfo>, FileError> {
        let path = path.clone();
        self.run(move |f| {
            let (file, _) = f.open_read(&path)?;
            Ok(jar::read_mod_info(std::io::BufReader::new(file)))
        })
        .await
    }

    /// A mod jar's icon, if it has one.
    pub async fn mod_icon(&self, path: &RelPath) -> Result<Option<Vec<u8>>, FileError> {
        let path = path.clone();
        self.run(move |f| {
            let (file, _) = f.open_read(&path)?;
            let Some(icon) = jar::read_mod_info(std::io::BufReader::new(file)).and_then(|i| i.icon)
            else {
                return Ok(None);
            };
            let (file, _) = f.open_read(&path)?;
            Ok(jar::read_icon(std::io::BufReader::new(file), &icon))
        })
        .await
    }

    /// Extracts `archive` into `dest` as a job. Returns the job ID.
    pub fn extract(&self, archive: RelPath, dest: RelPath, overwrite: bool) -> String {
        let Kind::Local(files) = &self.kind;
        let files = files.clone();
        jobs::spawn_blocking(&self.events, &self.server_id, "extract", move |progress| {
            let report = files.extract(&archive, &dest, overwrite, Limits::default(), progress)?;
            let mut message = format!(
                "Extracted {} files from {archive} into {dest}",
                report.files
            );
            if report.skipped_links > 0 {
                message.push_str(&format!(" ({} links left out)", report.skipped_links));
            }
            Ok(message)
        })
    }

    /// Compresses `sources` into the zip `dest` as a job. Returns the job ID.
    pub fn compress(&self, sources: Vec<RelPath>, dest: RelPath) -> String {
        let Kind::Local(files) = &self.kind;
        let files = files.clone();
        jobs::spawn_blocking(&self.events, &self.server_id, "compress", move |progress| {
            let report = files.compress(&sources, &dest, progress)?;
            let mut message = format!("Compressed {} files into {dest}", report.files);
            if report.skipped_links > 0 {
                message.push_str(&format!(" ({} links left out)", report.skipped_links));
            }
            Ok(message)
        })
    }

    /// Moves `src`, a finished upload on the panel's machine, into folder
    /// `dir` as `name`.
    pub async fn adopt(
        &self,
        src: std::path::PathBuf,
        dir: &RelPath,
        name: &str,
        on_conflict: OnConflict,
    ) -> Result<Adopted, FileError> {
        let (dir, name) = (dir.clone(), name.to_owned());
        self.run(move |f| f.adopt(&src, &dir, &name, on_conflict))
            .await
    }

    pub(crate) fn into_local_files(self) -> Arc<ServerFiles> {
        let Kind::Local(files) = self.kind;
        files
    }
}

/// A file being read: its size, and its bytes from any offset.
pub struct FileReader {
    size: u64,
    kind: ReaderKind,
}

enum ReaderKind {
    Local(tokio::fs::File),
}

impl FileReader {
    /// A file on this machine.
    pub fn local(file: std::fs::File, size: u64) -> Self {
        Self {
            size,
            kind: ReaderKind::Local(tokio::fs::File::from_std(file)),
        }
    }

    /// The file's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The bytes from `start` to the end.
    pub async fn from_offset(self, start: u64) -> std::io::Result<Pin<Box<dyn AsyncRead + Send>>> {
        match self.kind {
            ReaderKind::Local(mut file) => {
                if start > 0 {
                    file.seek(SeekFrom::Start(start)).await?;
                }
                Ok(Box::pin(file))
            }
        }
    }
}
