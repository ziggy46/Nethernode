//! File operations confined to one server's folder.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, Metadata, OpenOptions};
use sha2::{Digest, Sha256};

use crate::error::{FileError, Result};
use crate::owner::Owner;
use crate::path::{RelPath, TRASH_DIR};

/// Largest file the editor opens; bigger files are downloaded instead.
pub const EDITOR_LIMIT: u64 = 5 * 1024 * 1024;

/// What a folder entry is. Symlinks are reported as such, not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A folder.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Anything else (sockets, devices).
    Other,
}

/// One item in a folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// File or folder name.
    pub name: String,
    /// What it is.
    pub kind: EntryKind,
    /// Size in bytes (0 for folders).
    pub size: u64,
    /// Last modification, in Unix milliseconds.
    pub modified_ms: i64,
}

/// A file read for the editor.
#[derive(Debug, Clone)]
pub struct FileContent {
    /// The bytes.
    pub data: Vec<u8>,
    /// Whether it's UTF-8 text without NUL bytes.
    pub is_text: bool,
    /// Pass back to [`ServerFiles::write`] to detect concurrent changes.
    pub version: String,
    /// Last modification, in Unix milliseconds.
    pub modified_ms: i64,
}

/// What [`ServerFiles::write`] expects to find at the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Nothing; fail if the file exists.
    New,
    /// The file at this version; fail if it changed or is gone.
    Version(String),
    /// Anything; replace it.
    Any,
}

/// Results of a copy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyReport {
    /// Files copied.
    pub files: u64,
    /// Symlinks left out.
    pub skipped_links: u64,
}

/// A server's folder, opened as a sandbox: no path can reach outside it.
pub struct ServerFiles {
    root: Dir,
    owner: Owner,
}

impl ServerFiles {
    /// Opens the server folder at `root` (an absolute host path).
    pub fn open(root: &Path) -> Result<Self> {
        let dir =
            Dir::open_ambient_dir(root, ambient_authority()).map_err(|e| FileError::at("", e))?;
        let owner = Owner::for_root(&dir)?;
        Ok(Self { root: dir, owner })
    }

    /// Overrides the ownership policy (tests).
    pub fn with_owner(mut self, owner: Owner) -> Self {
        self.owner = owner;
        self
    }

    pub(crate) fn dir(&self) -> &Dir {
        &self.root
    }

    pub(crate) fn owner(&self) -> Owner {
        self.owner
    }

    /// Lists a folder: folders first, then files, by name. The trash folder
    /// is hidden.
    pub fn list(&self, path: &RelPath) -> Result<Vec<Entry>> {
        let at = |e| FileError::at(path.as_str(), e);
        let meta = self.root.metadata(path.as_path()).map_err(at)?;
        if !meta.is_dir() {
            return Err(FileError::NotDirectory(path.to_string()));
        }
        let mut entries = Vec::new();
        for item in self.root.read_dir(path.as_path()).map_err(at)? {
            let item = item.map_err(at)?;
            let Ok(name) = item.file_name().into_string() else {
                tracing::debug!(folder = %path, "skipping a name that isn't UTF-8");
                continue;
            };
            if path.is_root() && name == TRASH_DIR {
                continue;
            }
            let Ok(meta) = item.metadata() else { continue };
            entries.push(entry(name, &meta));
        }
        entries.sort_by(|a, b| {
            (a.kind != EntryKind::Directory)
                .cmp(&(b.kind != EntryKind::Directory))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// Details of one entry (not following a final symlink).
    pub fn stat(&self, path: &RelPath) -> Result<Entry> {
        let meta = self
            .root
            .symlink_metadata(path.as_path())
            .map_err(|e| FileError::at(path.as_str(), e))?;
        let name = path.file_name().unwrap_or("").to_owned();
        Ok(entry(name, &meta))
    }

    /// Reads a file of at most `limit` bytes.
    pub fn read(&self, path: &RelPath, limit: u64) -> Result<FileContent> {
        let (mut file, meta) = self.open_file(path)?;
        if meta.len() > limit {
            return Err(FileError::TooLarge {
                what: path.to_string(),
                limit: human_bytes(limit),
            });
        }
        let mut data = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
        Read::by_ref(&mut file)
            .take(limit + 1)
            .read_to_end(&mut data)
            .map_err(|e| FileError::at(path.as_str(), e))?;
        if data.len() as u64 > limit {
            return Err(FileError::TooLarge {
                what: path.to_string(),
                limit: human_bytes(limit),
            });
        }
        let is_text = !data.contains(&0) && std::str::from_utf8(&data).is_ok();
        Ok(FileContent {
            version: version_of(&data),
            is_text,
            modified_ms: modified_ms(&meta),
            data,
        })
    }

    /// Opens a file for streaming (downloads). Returns it with its size.
    pub fn open_read(&self, path: &RelPath) -> Result<(std::fs::File, u64)> {
        let (file, meta) = self.open_file(path)?;
        Ok((file.into_std(), meta.len()))
    }

    fn open_file(&self, path: &RelPath) -> Result<(File, Metadata)> {
        if path.is_root() {
            return Err(FileError::IsDirectory(path.to_string()));
        }
        let at = |e| FileError::at(path.as_str(), e);
        let file = self.root.open(path.as_path()).map_err(at)?;
        let meta = file.metadata().map_err(at)?;
        if meta.is_dir() {
            return Err(FileError::IsDirectory(path.to_string()));
        }
        if !meta.is_file() {
            return Err(FileError::Invalid(format!("{path} isn't a regular file")));
        }
        Ok((file, meta))
    }

    /// Writes a whole file, replacing it atomically: the data goes to a
    /// temporary file that is then renamed over the target, so a crash never
    /// leaves half a file. A replaced file keeps its permissions and owner.
    /// Returns the new version.
    pub fn write(&self, path: &RelPath, data: &[u8], expect: Expect) -> Result<String> {
        let (parent, name) = split(path)?;
        let at = |e| FileError::at(path.as_str(), e);
        let pdir = self.open_parent(&parent)?;
        let existing = match pdir.symlink_metadata(name) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(at(e)),
        };
        if let Some(meta) = &existing {
            if meta.is_symlink() {
                return Err(FileError::Invalid(format!(
                    "{path} is a link; edit the file it points to"
                )));
            }
            if meta.is_dir() {
                return Err(FileError::IsDirectory(path.to_string()));
            }
        }
        let old = existing
            .as_ref()
            .map(|_| pdir.open(name))
            .transpose()
            .map_err(at)?;
        match (&expect, &old) {
            (Expect::New, Some(_)) => return Err(FileError::AlreadyExists(path.to_string())),
            (Expect::Version(_), None) => return Err(FileError::Conflict),
            (Expect::Version(want), Some(file)) if &hash_file(file).map_err(at)? != want => {
                return Err(FileError::Conflict);
            }
            _ => {}
        }

        let tmp = temp_name(name);
        let mut file = pdir
            .open_with(&tmp, OpenOptions::new().write(true).create_new(true))
            .map_err(at)?;
        let result = (|| -> io::Result<()> {
            file.write_all(data)?;
            if let Some(old) = &old {
                file.set_permissions(old.metadata()?.permissions())?;
            }
            self.owner.replacing(&file, old.as_ref())?;
            file.sync_all()?;
            pdir.rename(&tmp, &pdir, name)
        })();
        if let Err(e) = result {
            let _ = pdir.remove_file(&tmp);
            return Err(at(e));
        }
        Ok(version_of(data))
    }

    /// Creates a folder. Its parent must exist.
    pub fn mkdir(&self, path: &RelPath) -> Result<()> {
        if path.is_root() {
            return Err(FileError::AlreadyExists(path.to_string()));
        }
        let at = |e| FileError::at(path.as_str(), e);
        self.root.create_dir(path.as_path()).map_err(at)?;
        self.owner.created(&self.root, path.as_path()).map_err(at)
    }

    /// Creates `path` and any missing parents.
    pub(crate) fn ensure_dirs(&self, path: &RelPath) -> Result<()> {
        let mut current = RelPath::root();
        for part in path.as_str().split('/').filter(|p| !p.is_empty()) {
            current = current.join(part)?;
            match self.root.create_dir(current.as_path()) {
                Ok(()) => self
                    .owner
                    .created(&self.root, current.as_path())
                    .map_err(|e| FileError::at(current.as_str(), e))?,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let meta = self
                        .root
                        .metadata(current.as_path())
                        .map_err(|e| FileError::at(current.as_str(), e))?;
                    if !meta.is_dir() {
                        return Err(FileError::NotDirectory(current.to_string()));
                    }
                }
                Err(e) => return Err(FileError::at(current.as_str(), e)),
            }
        }
        Ok(())
    }

    /// Moves or renames. Never overwrites.
    pub fn rename(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        self.check_destination(from, to)?;
        self.root
            .rename(from.as_path(), &self.root, to.as_path())
            .map_err(|e| FileError::at(from.as_str(), e))
    }

    /// Copies a file or folder (recursively). Never overwrites; symlinks
    /// are left out.
    pub fn copy(&self, from: &RelPath, to: &RelPath) -> Result<CopyReport> {
        self.check_destination(from, to)?;
        let meta = self
            .root
            .symlink_metadata(from.as_path())
            .map_err(|e| FileError::at(from.as_str(), e))?;
        let mut report = CopyReport::default();
        if meta.is_symlink() {
            return Err(FileError::Invalid(format!(
                "{from} is a link and can't be copied"
            )));
        }
        if meta.is_file() {
            self.copy_file(from.as_path(), to.as_path())?;
            report.files = 1;
            return Ok(report);
        }
        // Folders: walk with an explicit stack so deep trees can't overflow.
        let mut stack = vec![(from.as_path().to_path_buf(), to.as_path().to_path_buf())];
        while let Some((src, dst)) = stack.pop() {
            self.root
                .create_dir(&dst)
                .map_err(|e| FileError::at(&dst.to_string_lossy(), e))?;
            self.owner
                .created(&self.root, &dst)
                .map_err(|e| FileError::at(&dst.to_string_lossy(), e))?;
            let entries = self
                .root
                .read_dir(&src)
                .map_err(|e| FileError::at(&src.to_string_lossy(), e))?;
            for item in entries {
                let item = item?;
                let name = item.file_name();
                let kind = item.file_type()?;
                let (s, d) = (src.join(&name), dst.join(&name));
                if kind.is_symlink() {
                    report.skipped_links += 1;
                } else if kind.is_dir() {
                    stack.push((s, d));
                } else if kind.is_file() {
                    self.copy_file(&s, &d)?;
                    report.files += 1;
                }
            }
        }
        Ok(report)
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        let at = |p: &Path, e| FileError::at(&p.to_string_lossy(), e);
        let mut src = self.root.open(from).map_err(|e| at(from, e))?;
        let mut dst = self
            .root
            .open_with(to, OpenOptions::new().write(true).create_new(true))
            .map_err(|e| at(to, e))?;
        io::copy(&mut src, &mut dst).map_err(|e| at(to, e))?;
        dst.set_permissions(src.metadata()?.permissions())
            .map_err(|e| at(to, e))?;
        self.owner.created(&self.root, to).map_err(|e| at(to, e))
    }

    fn check_destination(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        if from.is_root() || to.is_root() {
            return Err(FileError::Invalid(
                "the server folder itself can't be moved".into(),
            ));
        }
        if to.starts_with(from) {
            return Err(FileError::Invalid(format!(
                "can't put {from} inside itself"
            )));
        }
        match self.root.symlink_metadata(to.as_path()) {
            Ok(_) => return Err(FileError::AlreadyExists(to.to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(FileError::at(to.as_str(), e)),
        }
        let parent = to.parent().unwrap_or_default();
        self.open_parent(&parent).map(drop)
    }

    fn open_parent(&self, parent: &RelPath) -> Result<Dir> {
        self.root.open_dir(parent.as_path()).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                FileError::NotFound(format!("folder {parent}"))
            } else {
                FileError::at(parent.as_str(), e)
            }
        })
    }
}

/// Splits a non-root path into its parent and name.
pub(crate) fn split(path: &RelPath) -> Result<(RelPath, &str)> {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => Ok((parent, name)),
        _ => Err(FileError::Invalid("that's the server folder itself".into())),
    }
}

/// A hidden temporary name beside `name`, unique per call.
pub(crate) fn temp_name(name: &str) -> PathBuf {
    let stem: String = name.chars().take(100).collect();
    PathBuf::from(format!(
        ".{stem}.nn-{}.tmp",
        ulid::Ulid::generate().to_string().to_lowercase()
    ))
}

fn entry(name: String, meta: &Metadata) -> Entry {
    let kind = if meta.is_symlink() {
        EntryKind::Symlink
    } else if meta.is_dir() {
        EntryKind::Directory
    } else if meta.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Entry {
        name,
        kind,
        size: if kind == EntryKind::File {
            meta.len()
        } else {
            0
        },
        modified_ms: modified_ms(meta),
    }
}

fn modified_ms(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.into_std().duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Content version: a short hash, so edits can detect concurrent changes.
pub(crate) fn version_of(data: &[u8]) -> String {
    hex::encode(&Sha256::digest(data)[..12])
}

fn hash_file(file: &File) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut reader = file.try_clone()?;
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(&hasher.finalize()[..12]))
}

/// `5 MB`-style sizes for messages.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}
