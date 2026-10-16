//! Zip extraction and compression inside a server's folder.
//!
//! Extraction validates every entry name before writing anything: names that
//! would land outside the destination (`..`, absolute paths, the trash) fail
//! the whole archive (zip-slip), symlink entries are skipped, and the bytes
//! actually written are counted against a limit rather than trusting the
//! sizes the archive declares (zip bombs).

use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;

use cap_std::fs::OpenOptions;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{FileError, Result};
use crate::path::{RelPath, TRASH_DIR};
use crate::sandbox::{ServerFiles, human_bytes, split, temp_name};

/// Bounds on what an archive may expand to.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Most entries an archive may hold.
    pub max_entries: usize,
    /// Most bytes extraction may write.
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 200_000,
            max_bytes: 32 * 1024 * 1024 * 1024,
        }
    }
}

/// Results of an extraction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractReport {
    /// Files written.
    pub files: u64,
    /// Folders created (or already present).
    pub dirs: u64,
    /// Symlink entries left out.
    pub skipped_links: u64,
    /// Bytes written.
    pub bytes: u64,
}

/// Results of a compression.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompressReport {
    /// Files added.
    pub files: u64,
    /// Uncompressed bytes added.
    pub bytes: u64,
    /// Symlinks left out.
    pub skipped_links: u64,
}

/// Progress callback: `(bytes done, bytes total)`.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

struct Planned {
    index: usize,
    path: RelPath,
    is_dir: bool,
}

impl ServerFiles {
    /// Extracts the zip at `archive` into the folder `dest` (created if
    /// missing). Existing files are replaced only when `overwrite` is set.
    pub fn extract(
        &self,
        archive: &RelPath,
        dest: &RelPath,
        overwrite: bool,
        limits: Limits,
        progress: Progress<'_>,
    ) -> Result<ExtractReport> {
        let (file, _) = self.open_read(archive)?;
        let mut zip = ZipArchive::new(BufReader::new(file))
            .map_err(|e| FileError::Invalid(format!("{archive} isn't a readable zip: {e}")))?;
        if zip.len() > limits.max_entries {
            return Err(FileError::TooLarge {
                what: format!("{archive} ({} entries)", zip.len()),
                limit: format!("{} entries", limits.max_entries),
            });
        }

        // Pass 1: validate every name and plan the work. Nothing is written
        // unless the whole archive is safe.
        let mut report = ExtractReport::default();
        let mut plan = Vec::with_capacity(zip.len());
        let mut declared: u64 = 0;
        for index in 0..zip.len() {
            let entry = zip.by_index_raw(index).map_err(bad_zip(archive))?;
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & S_IFMT == S_IFLNK)
            {
                report.skipped_links += 1;
                continue;
            }
            let raw = entry.name().map_err(bad_zip(archive))?.replace('\\', "/");
            let path = entry_path(dest, &raw).map_err(|_| {
                FileError::Invalid(format!(
                    "{archive} contains an unsafe path ({raw:?}); nothing was extracted"
                ))
            })?;
            if path == *dest {
                continue;
            }
            declared = declared.saturating_add(entry.size());
            plan.push(Planned {
                index,
                is_dir: entry.is_dir(),
                path,
            });
        }
        if declared > limits.max_bytes {
            return Err(too_big(archive, limits));
        }
        if !overwrite {
            for item in plan.iter().filter(|p| !p.is_dir) {
                if self.dir().symlink_metadata(item.path.as_path()).is_ok() {
                    return Err(FileError::Invalid(format!(
                        "{} already exists, so nothing was extracted. Extract somewhere else or choose to replace existing files.",
                        item.path
                    )));
                }
            }
        }

        // Pass 2: write.
        self.ensure_dirs(dest)?;
        let mut done: u64 = 0;
        progress(0, declared);
        for item in plan {
            if item.is_dir {
                self.ensure_dirs(&item.path)?;
                report.dirs += 1;
                continue;
            }
            let (parent, _) = split(&item.path)?;
            self.ensure_dirs(&parent)?;
            let at = |e| FileError::at(item.path.as_str(), e);
            if let Ok(meta) = self.dir().symlink_metadata(item.path.as_path())
                && (meta.is_dir() || meta.is_symlink())
            {
                return Err(FileError::Invalid(format!(
                    "{} is a folder or link and can't be replaced by a file",
                    item.path
                )));
            }
            let mut options = OpenOptions::new();
            options.write(true).create(true).truncate(true);
            let mut out = self
                .dir()
                .open_with(item.path.as_path(), &options)
                .map_err(at)?;
            self.owner()
                .created(self.dir(), item.path.as_path())
                .map_err(at)?;
            let mut entry = zip.by_index(item.index).map_err(bad_zip(archive))?;
            let budget = limits.max_bytes - report.bytes;
            let written = copy_counting(&mut (&mut entry).take(budget + 1), &mut out, &mut |n| {
                progress(done + n, declared);
            })
            .map_err(|e| {
                if e.kind() == io::ErrorKind::InvalidData {
                    FileError::Invalid(format!("{archive} is damaged: {e}"))
                } else {
                    at(e)
                }
            })?;
            if written > budget {
                return Err(too_big(archive, limits));
            }
            report.bytes += written;
            report.files += 1;
            done = done.saturating_add(written);
        }
        progress(done, done.max(declared));
        Ok(report)
    }

    /// Zips `sources` (all in the same folder) into a new archive at `dest`.
    /// Symlinks are left out; the archive is written to a temporary file and
    /// renamed into place when complete.
    pub fn compress(
        &self,
        sources: &[RelPath],
        dest: &RelPath,
        progress: Progress<'_>,
    ) -> Result<CompressReport> {
        let Some(first) = sources.first() else {
            return Err(FileError::Invalid("choose something to compress".into()));
        };
        let base = first.parent().unwrap_or_default();
        if sources
            .iter()
            .any(|s| s.is_root() || s.parent().as_ref() != Some(&base))
        {
            return Err(FileError::Invalid(
                "everything compressed together must be in the same folder".into(),
            ));
        }
        let (dest_parent, dest_name) = split(dest)?;
        if !dest_name.to_ascii_lowercase().ends_with(".zip") {
            return Err(FileError::Invalid(
                "the archive name must end in .zip".into(),
            ));
        }
        if self.dir().symlink_metadata(dest.as_path()).is_ok() {
            return Err(FileError::AlreadyExists(dest.to_string()));
        }
        let tmp_name = temp_name(dest_name);
        let tmp = dest_parent.as_path().join(&tmp_name);

        // Walk first: the file list and total size drive progress.
        let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        let mut report = CompressReport::default();
        for source in sources {
            let name = source.file_name().unwrap_or_default().to_owned();
            let meta = self
                .dir()
                .symlink_metadata(source.as_path())
                .map_err(|e| FileError::at(source.as_str(), e))?;
            if meta.is_symlink() {
                report.skipped_links += 1;
            } else if meta.is_file() {
                files.push((source.as_path().to_path_buf(), name, meta.len()));
            } else if meta.is_dir() {
                let mut stack = vec![(source.as_path().to_path_buf(), name)];
                while let Some((dir, prefix)) = stack.pop() {
                    dirs.push(format!("{prefix}/"));
                    let entries = self
                        .dir()
                        .read_dir(&dir)
                        .map_err(|e| FileError::at(&dir.to_string_lossy(), e))?;
                    for item in entries {
                        let item = item?;
                        let Ok(child) = item.file_name().into_string() else {
                            continue;
                        };
                        let path = dir.join(&child);
                        if path == tmp || (dir.as_os_str() == "." && child == TRASH_DIR) {
                            continue;
                        }
                        let kind = item.file_type()?;
                        if kind.is_symlink() {
                            report.skipped_links += 1;
                        } else if kind.is_dir() {
                            stack.push((path, format!("{prefix}/{child}")));
                        } else if kind.is_file() {
                            let size = item.metadata()?.len();
                            files.push((path, format!("{prefix}/{child}"), size));
                        }
                    }
                }
            }
        }
        let total: u64 = files.iter().map(|(_, _, size)| size).sum();

        let at = |e| FileError::at(dest.as_str(), e);
        let out = self
            .dir()
            .open_with(
                &tmp,
                OpenOptions::new().write(true).read(true).create_new(true),
            )
            .map_err(at)?;
        let result = (|| -> Result<()> {
            let mut zip = ZipWriter::new(out);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for dir in &dirs {
                zip.add_directory(dir.as_str(), options)
                    .map_err(zip_error)?;
            }
            let mut done = 0;
            progress(0, total);
            for (path, name, size) in &files {
                zip.start_file(
                    name.as_str(),
                    options.large_file(*size >= u64::from(u32::MAX)),
                )
                .map_err(zip_error)?;
                let mut src = self
                    .dir()
                    .open(path)
                    .map_err(|e| FileError::at(&path.to_string_lossy(), e))?;
                let n = copy_counting(&mut src, &mut zip, &mut |n| progress(done + n, total))?;
                done += n;
                report.files += 1;
                report.bytes += n;
            }
            let out = zip.finish().map_err(zip_error)?;
            self.owner().replacing(&out, None)?;
            out.sync_all()?;
            Ok(())
        })();
        let result = result.and_then(|()| {
            self.dir()
                .rename(&tmp, self.dir(), dest.as_path())
                .map_err(at)
        });
        if let Err(e) = result {
            let _ = self.dir().remove_file(&tmp);
            return Err(e);
        }
        Ok(report)
    }
}

/// Where an entry named `raw` lands under `dest`, if that's safe.
fn entry_path(dest: &RelPath, raw: &str) -> Result<RelPath> {
    if raw.starts_with('/') || raw.split('/').any(|part| part == "..") {
        return Err(FileError::InvalidPath("unsafe"));
    }
    // A drive prefix like `C:` is an absolute path on Windows.
    if raw.as_bytes().get(1) == Some(&b':') {
        return Err(FileError::InvalidPath("unsafe"));
    }
    if dest.is_root() {
        RelPath::parse(raw)
    } else {
        RelPath::parse(&format!("{}/{raw}", dest.as_str()))
    }
}

/// Copies `from` into `to`, reporting the running total every 1 MiB.
fn copy_counting(
    from: &mut dyn Read,
    to: &mut dyn Write,
    report: &mut dyn FnMut(u64),
) -> io::Result<u64> {
    let mut buf = vec![0; 64 * 1024];
    let mut total: u64 = 0;
    let mut last = 0;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..n])?;
        total += n as u64;
        if total - last >= 1024 * 1024 {
            report(total);
            last = total;
        }
    }
    report(total);
    Ok(total)
}

fn bad_zip(archive: &RelPath) -> impl Fn(zip::result::ZipError) -> FileError + '_ {
    move |e| FileError::Invalid(format!("{archive} is damaged: {e}"))
}

fn zip_error(e: zip::result::ZipError) -> FileError {
    match e {
        zip::result::ZipError::Io(e) => FileError::Io(e),
        other => FileError::Invalid(other.to_string()),
    }
}

fn too_big(archive: &RelPath, limits: Limits) -> FileError {
    FileError::TooLarge {
        what: format!("{archive} when extracted"),
        limit: human_bytes(limits.max_bytes),
    }
}
