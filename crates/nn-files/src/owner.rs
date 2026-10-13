//! Who files NetherNode writes belong to.
//!
//! A server reads and writes its own files as an unprivileged user. When
//! NetherNode runs as that same user (the orchestrator arranges this on
//! Linux) there's nothing to do. When NetherNode runs as root, files it
//! creates would be root-owned and the server couldn't change them (think
//! `ops.json` after an edit), so new files are handed to whoever owns the
//! server's folder, and replaced files keep their previous owner.

use std::io;
use std::path::Path;

use cap_std::fs::{Dir, File};

/// Ownership fix-ups to apply after writing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Owner {
    /// `Some((uid, gid))` when NetherNode is root and must chown new files.
    chown_to: Option<(u32, u32)>,
}

impl Owner {
    /// Ownership policy for a server whose root folder is `root`.
    pub fn for_root(root: &Dir) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            if rustix::process::geteuid().is_root() {
                let stat = rustix::fs::fstat(root)?;
                return Ok(Self {
                    chown_to: Some((stat.st_uid, stat.st_gid)),
                });
            }
        }
        let _ = root;
        Ok(Self::default())
    }

    /// A fixed policy, for tests.
    pub fn chown_to(uid: u32, gid: u32) -> Self {
        Self {
            chown_to: Some((uid, gid)),
        }
    }

    /// Hands a newly created entry (not following symlinks) to the folder owner.
    pub(crate) fn created(&self, dir: &Dir, path: &Path) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some((uid, gid)) = self.chown_to {
            use rustix::fs::{AtFlags, Gid, Uid, chownat};
            chownat(
                dir,
                path,
                Some(Uid::from_raw(uid)),
                Some(Gid::from_raw(gid)),
                AtFlags::SYMLINK_NOFOLLOW,
            )?;
        }
        let _ = (dir, path);
        Ok(())
    }

    /// Gives a replacement file the owner of the file it replaces, or the
    /// folder owner if it's new.
    pub(crate) fn replacing(&self, new: &File, old: Option<&File>) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some((uid, gid)) = self.chown_to {
            use rustix::fs::{Gid, Uid, fchown, fstat};
            let (uid, gid) = match old {
                Some(old) => {
                    let stat = fstat(old)?;
                    (stat.st_uid, stat.st_gid)
                }
                None => (uid, gid),
            };
            fchown(new, Some(Uid::from_raw(uid)), Some(Gid::from_raw(gid)))?;
        }
        let _ = (new, old);
        Ok(())
    }
}
