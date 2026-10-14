//! Backups of a whole server folder: a zstd-compressed tar, streamed so a
//! large world never sits in memory.
//!
//! Folders the server downloads again on its own (libraries, versions,
//! caches, logs) and the trash are left out. Restoring extracts into a new
//! folder with the same safety rules as zip extraction: names that would
//! land outside it fail the whole restore, links and special files are
//! skipped, and the bytes written are counted against a limit.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};

use crate::archive::{ExtractReport, Limits, Progress};
use crate::error::{FileError, Result};
use crate::owner::Owner;
use crate::path::{RelPath, TRASH_DIR};
use crate::sandbox::ServerFiles;

/// Top-level folders left out of backups: the server downloads or writes
/// them again on its own.
pub const REGENERATED: &[&str] = &[
    "libraries",
    "versions",
    ".cache",
    "cache",
    ".fabric",
    "logs",
    "crash-reports",
    "downloads",
    TRASH_DIR,
];

/// zstd level: fast, and still small for worlds.
const LEVEL: i32 = 3;

/// What a backup holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupReport {
    /// Files archived.
    pub files: u64,
    /// Their size before compression.
    pub bytes: u64,
    /// Symlinks left out.
    pub skipped_links: u64,
}

impl ServerFiles {
    /// Writes a backup of the whole folder to `out`, returning it when done.
    pub fn backup_to<W: Write>(&self, out: W, progress: Progress<'_>) -> Result<(W, BackupReport)> {
        let mut report = BackupReport::default();
        // Walk first: the total drives progress.
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<(PathBuf, u64)> = Vec::new();
        let mut stack = vec![PathBuf::from(".")];
        while let Some(dir) = stack.pop() {
            let entries = self
                .dir()
                .read_dir(&dir)
                .map_err(|e| FileError::at(&dir.to_string_lossy(), e))?;
            for item in entries {
                let item = item?;
                let Ok(name) = item.file_name().into_string() else {
                    continue;
                };
                if dir.as_os_str() == "." && REGENERATED.contains(&name.as_str()) {
                    continue;
                }
                let path = if dir.as_os_str() == "." {
                    PathBuf::from(&name)
                } else {
                    dir.join(&name)
                };
                let kind = item.file_type()?;
                if kind.is_symlink() {
                    report.skipped_links += 1;
                } else if kind.is_dir() {
                    dirs.push(path.clone());
                    stack.push(path);
                } else if kind.is_file() {
                    files.push((path, item.metadata()?.len()));
                }
            }
        }
        let total: u64 = files.iter().map(|(_, size)| size).sum();
        progress(0, total);

        let encoder = zstd::Encoder::new(out, LEVEL)?;
        let mut tar = tar::Builder::new(encoder);
        tar.mode(tar::HeaderMode::Deterministic);
        for dir in &dirs {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            tar.append_data(&mut header, dir, io::empty())?;
        }
        let mut done = 0;
        for (path, _) in &files {
            let at = |e| FileError::at(&path.to_string_lossy(), e);
            let file = self.dir().open(path).map_err(at)?;
            let meta = file.metadata().map_err(at)?;
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            header.set_mode(mode_of(&meta));
            if let Ok(modified) = meta.modified() {
                header.set_mtime(
                    modified
                        .into_std()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs()),
                );
            }
            // The size is read now; a file growing while it's archived is
            // cut at that size (the server's saving is paused meanwhile).
            let mut reader = file.take(meta.len());
            let mut counted = Counted {
                inner: &mut reader,
                on_read: &mut |n| progress(done + n, total),
                read: 0,
            };
            tar.append_data(&mut header, path, &mut counted)
                .map_err(at)?;
            done += meta.len();
            report.files += 1;
            report.bytes += meta.len();
        }
        let encoder = tar.into_inner()?;
        let out = encoder.finish()?;
        progress(total, total);
        Ok((out, report))
    }
}

#[cfg(unix)]
fn mode_of(meta: &cap_std::fs::Metadata) -> u32 {
    use cap_std::fs::PermissionsExt;
    meta.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(meta: &cap_std::fs::Metadata) -> u32 {
    if meta.permissions().readonly() {
        0o444
    } else {
        0o644
    }
}

/// Counts bytes read and reports them.
struct Counted<'a, R> {
    inner: R,
    on_read: &'a mut dyn FnMut(u64),
    read: u64,
}

impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        (self.on_read)(self.read);
        Ok(n)
    }
}

/// Restores a backup into a new folder `parent/name`, which must not exist.
/// `size` is the backup's size, for progress. New files get `owner`. On
/// failure the half-written folder is removed.
pub fn restore_into<R: Read>(
    input: R,
    size: u64,
    parent: &Path,
    name: &str,
    owner: Owner,
    limits: Limits,
    progress: Progress<'_>,
) -> Result<ExtractReport> {
    crate::path::validate_name(name)?;
    let parent_dir =
        Dir::open_ambient_dir(parent, ambient_authority()).map_err(|e| FileError::at("", e))?;
    parent_dir
        .create_dir(name)
        .map_err(|e| FileError::at(name, e))?;
    let result = (|| {
        let dir = parent_dir
            .open_dir(name)
            .map_err(|e| FileError::at(name, e))?;
        let mut counted = Counted {
            inner: input,
            on_read: &mut |n| progress(n.min(size), size),
            read: 0,
        };
        let decoder = zstd::Decoder::new(&mut counted)?;
        extract(&dir, decoder, owner, limits)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(parent.join(name));
    }
    result
}

fn extract<R: Read>(dir: &Dir, input: R, owner: Owner, limits: Limits) -> Result<ExtractReport> {
    let mut report = ExtractReport::default();
    let mut archive = tar::Archive::new(input);
    let mut entries_seen = 0usize;
    for entry in archive.entries()? {
        let mut entry = entry?;
        entries_seen += 1;
        if entries_seen > limits.max_entries {
            return Err(FileError::TooLarge {
                what: "the backup".into(),
                limit: format!("{} entries", limits.max_entries),
            });
        }
        let raw = entry.path()?.to_string_lossy().replace('\\', "/");
        let path = RelPath::parse(raw.trim_end_matches('/')).map_err(|_| {
            FileError::Invalid(format!("the backup holds a file outside its folder: {raw}"))
        })?;
        if path.is_root() {
            continue;
        }
        let at = |e| FileError::at(path.as_str(), e);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                create_dirs(dir, &path, owner)?;
                report.dirs += 1;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = path.parent().filter(|p| !p.is_root()) {
                    create_dirs(dir, &parent, owner)?;
                }
                let mut file = dir
                    .open_with(
                        path.as_path(),
                        OpenOptions::new().write(true).create_new(true),
                    )
                    .map_err(at)?;
                let remaining = limits.max_bytes.saturating_sub(report.bytes);
                let n = io::copy(&mut (&mut entry).take(remaining + 1), &mut file).map_err(at)?;
                if n > remaining {
                    return Err(FileError::TooLarge {
                        what: "the backup".into(),
                        limit: crate::sandbox::human_bytes(limits.max_bytes),
                    });
                }
                #[cfg(unix)]
                {
                    use cap_std::fs::PermissionsExt;
                    let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
                    file.set_permissions(cap_std::fs::Permissions::from_mode(mode | 0o600))
                        .map_err(at)?;
                }
                file.flush().map_err(at)?;
                owner.created(dir, path.as_path()).map_err(at)?;
                report.files += 1;
                report.bytes += n;
            }
            // Links and special files never come out of a backup.
            _ => report.skipped_links += 1,
        }
    }
    Ok(report)
}

fn create_dirs(dir: &Dir, path: &RelPath, owner: Owner) -> Result<()> {
    let mut current = RelPath::root();
    for part in path.as_str().split('/').filter(|p| !p.is_empty()) {
        current = current.join(part)?;
        match dir.create_dir(current.as_path()) {
            Ok(()) => owner
                .created(dir, current.as_path())
                .map_err(|e| FileError::at(current.as_str(), e))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let meta = dir
                    .symlink_metadata(current.as_path())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn server(tmp: &Path) -> ServerFiles {
        let root = tmp.join("smp");
        std::fs::create_dir_all(root.join("world/region")).unwrap();
        std::fs::create_dir_all(root.join("libraries/big")).unwrap();
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("world/level.dat"), b"level").unwrap();
        std::fs::write(root.join("world/region/r.0.0.mca"), vec![7u8; 300_000]).unwrap();
        std::fs::write(root.join("server.properties"), b"motd=hi\n").unwrap();
        std::fs::write(root.join("mods/a.jar"), b"jar").unwrap();
        std::fs::write(root.join("libraries/big/lib.jar"), b"skip me").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", root.join("world/escape")).unwrap();
        ServerFiles::open(&root)
            .unwrap()
            .with_owner(Owner::default())
    }

    #[test]
    fn round_trip_without_regenerated_folders_or_links() {
        let tmp = tempfile::tempdir().unwrap();
        let files = server(tmp.path());
        let mut seen = 0;
        let (bytes, report) = files
            .backup_to(Vec::new(), &mut |done, _| seen = done)
            .unwrap();
        assert_eq!(report.files, 4);
        assert_eq!(seen, report.bytes);
        assert!(bytes.len() < 10_000, "compressed: {} bytes", bytes.len());
        #[cfg(unix)]
        assert_eq!(report.skipped_links, 1);

        let restored = restore_into(
            bytes.as_slice(),
            bytes.len() as u64,
            tmp.path(),
            "smp.restoring",
            Owner::default(),
            Limits::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(restored.files, 4);
        let out = tmp.path().join("smp.restoring");
        assert_eq!(
            std::fs::read(out.join("world/level.dat")).unwrap(),
            b"level"
        );
        assert_eq!(
            std::fs::read(out.join("world/region/r.0.0.mca"))
                .unwrap()
                .len(),
            300_000
        );
        assert_eq!(std::fs::read(out.join("mods/a.jar")).unwrap(), b"jar");
        assert!(!out.join("libraries").exists());
        assert!(!out.join("world/escape").exists());
    }

    fn tar_with(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut out = zstd::Encoder::new(Vec::new(), 1).unwrap();
        {
            let mut tar = tar::Builder::new(&mut out);
            for (name, kind, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*kind);
                header.set_size(data.len() as u64);
                header.set_mode(0o4755);
                // Bypass the builder's own path checks to write hostile names.
                let bytes = name.as_bytes();
                header.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
                if *kind == tar::EntryType::Symlink {
                    header.set_link_name("/etc").unwrap();
                }
                header.set_cksum();
                tar.append(&header, *data).unwrap();
            }
            tar.finish().unwrap();
        }
        out.finish().unwrap()
    }

    #[test]
    fn hostile_backups_are_refused_and_cleaned_up() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["../evil", "/etc/evil", "world/../../evil"] {
            let bytes = tar_with(&[(name, tar::EntryType::Regular, b"x")]);
            let result = restore_into(
                bytes.as_slice(),
                0,
                tmp.path(),
                "out",
                Owner::default(),
                Limits::default(),
                &mut |_, _| {},
            );
            assert!(result.is_err(), "{name} must be refused");
            assert!(!tmp.path().join("out").exists(), "nothing left behind");
            assert!(!tmp.path().join("evil").exists());
        }
        let bytes = tar_with(&[
            ("link", tar::EntryType::Symlink, b""),
            ("ok.txt", tar::EntryType::Regular, b"fine"),
        ]);
        let report = restore_into(
            bytes.as_slice(),
            0,
            tmp.path(),
            "out",
            Owner::default(),
            Limits::default(),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!((report.files, report.skipped_links), (1, 1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tmp.path().join("out/ok.txt"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o7000, 0, "no setuid from a backup");
        }
        assert!(
            restore_into(
                bytes.as_slice(),
                0,
                tmp.path(),
                "out",
                Owner::default(),
                Limits::default(),
                &mut |_, _| {}
            )
            .is_err(),
            "never restores over an existing folder"
        );
    }

    #[test]
    fn size_limits_hold() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = tar_with(&[("big", tar::EntryType::Regular, &[0u8; 2048])]);
        let limits = Limits {
            max_entries: 10,
            max_bytes: 1024,
        };
        assert!(matches!(
            restore_into(
                bytes.as_slice(),
                0,
                tmp.path(),
                "out",
                Owner::default(),
                limits,
                &mut |_, _| {}
            ),
            Err(FileError::TooLarge { .. })
        ));
        assert!(!tmp.path().join("out").exists());
    }
}
