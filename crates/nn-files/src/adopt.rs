//! Moving finished uploads into a server's folder.

use std::io::{self, Write};
use std::path::Path;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, OpenOptions};

use crate::error::{FileError, Result};
use crate::path::{RelPath, validate_name};
use crate::sandbox::{ServerFiles, temp_name};

/// What to do when the name an upload wants is already taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnConflict {
    /// Keep both: the upload becomes `name (1).ext`.
    Rename,
    /// Replace the existing file (never a folder).
    Overwrite,
    /// Keep the existing file and discard the upload.
    Skip,
}

impl OnConflict {
    /// Parses `rename`, `overwrite`, or `skip`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rename" => Some(Self::Rename),
            "overwrite" => Some(Self::Overwrite),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }

    /// The name [`parse`](Self::parse) accepts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Overwrite => "overwrite",
            Self::Skip => "skip",
        }
    }
}

/// Where an adopted file ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adopted {
    /// Moved into place at this path.
    Placed(RelPath),
    /// The name was taken and the upload was discarded.
    Skipped(RelPath),
}

impl ServerFiles {
    /// Moves `src`, a file NetherNode wrote outside the sandbox (a finished
    /// upload), into folder `dir` as `name`. The move is atomic on the same
    /// filesystem; across filesystems the file is copied and then renamed
    /// into place. The file gets normal permissions and the server's owner
    /// (or, when replacing, those of the file it replaces).
    pub fn adopt(
        &self,
        src: &Path,
        dir: &RelPath,
        name: &str,
        on_conflict: OnConflict,
    ) -> Result<Adopted> {
        validate_name(name)?;
        let target = dir.join(name)?;
        let at = |e| FileError::at(target.as_str(), e);
        let pdir = self.open_folder(dir)?;
        let existing = match pdir.symlink_metadata(name) {
            Ok(meta) => Some(meta),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(at(e)),
        };

        let (src_dir, src_name) = open_source(src)?;
        let mut old: Option<File> = None;
        let final_name = match (&existing, on_conflict) {
            (None, _) => name.to_owned(),
            (Some(_), OnConflict::Skip) => {
                src_dir.remove_file(&src_name).map_err(at)?;
                return Ok(Adopted::Skipped(target));
            }
            (Some(_), OnConflict::Rename) => free_name(&pdir, name).map_err(at)?,
            (Some(meta), OnConflict::Overwrite) if meta.is_dir() => {
                return Err(FileError::IsDirectory(target.to_string()));
            }
            (Some(meta), OnConflict::Overwrite) if meta.is_symlink() => {
                return Err(FileError::Invalid(format!(
                    "{target} is a link; it wasn't replaced"
                )));
            }
            (Some(_), OnConflict::Overwrite) => {
                old = Some(pdir.open(name).map_err(at)?);
                name.to_owned()
            }
        };

        let file = src_dir.open(&src_name).map_err(at)?;
        self.prepare(&file, old.as_ref()).map_err(at)?;
        drop(file);
        match src_dir.rename(&src_name, &pdir, &final_name) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                self.copy_across(&src_dir, &src_name, &pdir, &final_name, old.as_ref())
                    .map_err(at)?;
                src_dir.remove_file(&src_name).map_err(at)?;
            }
            Err(e) => return Err(at(e)),
        }
        Ok(Adopted::Placed(dir.join(&final_name)?))
    }

    /// Permissions and owner for a file about to be placed.
    fn prepare(&self, file: &File, old: Option<&File>) -> io::Result<()> {
        match old {
            Some(old) => file.set_permissions(old.metadata()?.permissions())?,
            None => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(cap_std::fs::Permissions::from_std(
                        std::fs::Permissions::from_mode(0o644),
                    ))?;
                }
            }
        }
        self.owner().replacing(file, old)
    }

    fn copy_across(
        &self,
        src_dir: &Dir,
        src_name: &str,
        pdir: &Dir,
        final_name: &str,
        old: Option<&File>,
    ) -> io::Result<()> {
        let tmp = temp_name(final_name);
        let result = (|| {
            let mut src = src_dir.open(src_name)?;
            let mut dst = pdir.open_with(&tmp, OpenOptions::new().write(true).create_new(true))?;
            io::copy(&mut src, &mut dst)?;
            dst.flush()?;
            self.prepare(&dst, old)?;
            dst.sync_all()?;
            pdir.rename(&tmp, pdir, final_name)
        })();
        if result.is_err() {
            let _ = pdir.remove_file(&tmp);
        }
        result
    }
}

fn open_source(src: &Path) -> Result<(Dir, String)> {
    let (Some(parent), Some(name)) = (src.parent(), src.file_name().and_then(|n| n.to_str()))
    else {
        return Err(FileError::Invalid("bad upload location".into()));
    };
    let dir =
        Dir::open_ambient_dir(parent, ambient_authority()).map_err(|e| FileError::at("", e))?;
    Ok((dir, name.to_owned()))
}

/// `name (1).ext`, `name (2).ext`, … — the first that's free.
fn free_name(dir: &Dir, name: &str) -> io::Result<String> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    for n in 1..10_000 {
        let candidate = format!("{stem} ({n}){ext}");
        match dir.symlink_metadata(&candidate) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name for the upload",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner::Owner;

    fn setup() -> (tempfile::TempDir, ServerFiles, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("server");
        std::fs::create_dir_all(root.join("world")).unwrap();
        let uploads = tmp.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let files = ServerFiles::open(&root)
            .unwrap()
            .with_owner(Owner::default());
        (tmp, files, uploads)
    }

    fn upload(dir: &Path, id: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.join(id);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn places_renames_overwrites_and_skips() {
        let (tmp, files, uploads) = setup();
        let world = RelPath::parse("world").unwrap();
        let root = tmp.path().join("server");

        let placed = files
            .adopt(
                &upload(&uploads, "A", b"one"),
                &world,
                "level.dat",
                OnConflict::Rename,
            )
            .unwrap();
        assert_eq!(
            placed,
            Adopted::Placed(RelPath::parse("world/level.dat").unwrap())
        );
        assert_eq!(std::fs::read(root.join("world/level.dat")).unwrap(), b"one");

        let renamed = files
            .adopt(
                &upload(&uploads, "B", b"two"),
                &world,
                "level.dat",
                OnConflict::Rename,
            )
            .unwrap();
        assert_eq!(
            renamed,
            Adopted::Placed(RelPath::parse("world/level (1).dat").unwrap())
        );

        let skipped = files
            .adopt(
                &upload(&uploads, "C", b"three"),
                &world,
                "level.dat",
                OnConflict::Skip,
            )
            .unwrap();
        assert!(matches!(skipped, Adopted::Skipped(_)));
        assert!(!uploads.join("C").exists(), "a skipped upload is discarded");
        assert_eq!(std::fs::read(root.join("world/level.dat")).unwrap(), b"one");

        files
            .adopt(
                &upload(&uploads, "D", b"four"),
                &world,
                "level.dat",
                OnConflict::Overwrite,
            )
            .unwrap();
        assert_eq!(
            std::fs::read(root.join("world/level.dat")).unwrap(),
            b"four"
        );
        assert!(std::fs::read_dir(&uploads).unwrap().next().is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("world/level (1).dat"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o644);
        }
    }

    #[test]
    fn never_replaces_folders_or_links_or_escapes() {
        let (tmp, files, uploads) = setup();
        let root = RelPath::root();
        assert!(matches!(
            files.adopt(
                &upload(&uploads, "A", b"x"),
                &root,
                "world",
                OnConflict::Overwrite
            ),
            Err(FileError::IsDirectory(_))
        ));
        assert!(
            files
                .adopt(
                    &upload(&uploads, "B", b"x"),
                    &root,
                    "../escape",
                    OnConflict::Rename
                )
                .is_err()
        );
        assert!(
            files
                .adopt(
                    &upload(&uploads, "C", b"x"),
                    &root,
                    "a/b",
                    OnConflict::Rename
                )
                .is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path().join("uploads"), tmp.path().join("server/out"))
                .unwrap();
            let out = RelPath::parse("out").unwrap();
            assert!(
                files
                    .adopt(&upload(&uploads, "D", b"x"), &out, "f", OnConflict::Rename)
                    .is_err(),
                "a link to outside the server folder is refused"
            );
        }
    }

    #[test]
    fn free_names() {
        let (_tmp, files, uploads) = setup();
        let root = RelPath::root();
        for id in ["A", "B", "C"] {
            files
                .adopt(
                    &upload(&uploads, id, b"x"),
                    &root,
                    "mods.tar.gz",
                    OnConflict::Rename,
                )
                .unwrap();
        }
        for id in ["D", "E"] {
            files
                .adopt(
                    &upload(&uploads, id, b"x"),
                    &root,
                    ".env",
                    OnConflict::Rename,
                )
                .unwrap();
        }
        let mut names: Vec<_> = files
            .list(&root)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ".env",
                ".env (1)",
                "mods.tar (1).gz",
                "mods.tar (2).gz",
                "mods.tar.gz",
                "world"
            ]
        );
    }
}
