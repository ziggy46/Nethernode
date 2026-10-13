//! Paths relative to a server's root folder.
//!
//! Paths arrive from the browser, so they are validated before anything
//! touches the disk: no `..`, nothing absolute, no NUL or backslash, and the
//! trash folder is off limits. `cap-std` then confines every access to the
//! server's folder, which also stops symlinks from leading outside it.

use std::fmt;
use std::path::Path;

use crate::error::{FileError, Result};

/// Folder at the server root holding deleted items. Hidden from listings
/// and unreachable through [`RelPath`].
pub const TRASH_DIR: &str = ".trash";

const MAX_PATH: usize = 4096;
const MAX_COMPONENT: usize = 255;

/// A validated, normalized path inside a server's root. The empty path is
/// the root itself. Components are joined with `/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct RelPath(String);

impl RelPath {
    /// The server's root folder.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Validates a client-supplied path. `""`, `"."`, and `"/"` mean the
    /// root; `.` components and repeated slashes are dropped.
    pub fn parse(input: &str) -> Result<Self> {
        if input.len() > MAX_PATH {
            return Err(FileError::InvalidPath("too long"));
        }
        if input.contains('\0') {
            return Err(FileError::InvalidPath("contains a NUL byte"));
        }
        if input.contains('\\') {
            return Err(FileError::InvalidPath("use / to separate folders"));
        }
        if input.starts_with('/') && input != "/" {
            return Err(FileError::InvalidPath(
                "must be relative to the server folder",
            ));
        }
        let mut parts: Vec<&str> = Vec::new();
        for part in input.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(FileError::InvalidPath("`..` isn't allowed")),
                _ => {
                    validate_name(part)?;
                    parts.push(part);
                }
            }
        }
        if parts.first() == Some(&TRASH_DIR) {
            return Err(FileError::InvalidPath(
                "the trash folder is managed by NetherNode",
            ));
        }
        Ok(Self(parts.join("/")))
    }

    /// The path as `a/b/c`, empty for the root.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the server's root folder.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The last component, or `None` for the root.
    pub fn file_name(&self) -> Option<&str> {
        (!self.is_root()).then(|| self.0.rsplit('/').next().unwrap_or(&self.0))
    }

    /// The containing folder, or `None` for the root.
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        Some(Self(
            self.0.rsplit_once('/').map_or("", |(p, _)| p).to_owned(),
        ))
    }

    /// Appends one validated component.
    pub fn join(&self, name: &str) -> Result<Self> {
        validate_name(name)?;
        if self.is_root() && name == TRASH_DIR {
            return Err(FileError::InvalidPath(
                "the trash folder is managed by NetherNode",
            ));
        }
        Ok(Self(if self.is_root() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.0)
        }))
    }

    /// Whether `self` is `other` or inside it.
    pub fn starts_with(&self, other: &Self) -> bool {
        other.is_root()
            || self.0 == other.0
            || (self.0.starts_with(&other.0) && self.0.as_bytes().get(other.0.len()) == Some(&b'/'))
    }

    /// The path for `cap-std`, which wants `.` for the root.
    pub(crate) fn as_path(&self) -> &Path {
        Path::new(if self.is_root() { "." } else { &self.0 })
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_root() { "/" } else { &self.0 })
    }
}

/// Checks one path component (a file or folder name).
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(FileError::InvalidPath("not a valid name"));
    }
    if name.len() > MAX_COMPONENT {
        return Err(FileError::InvalidPath("a name is too long"));
    }
    if name.contains(['/', '\\', '\0']) {
        return Err(FileError::InvalidPath("names can't contain / or \\"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(RelPath::parse("").unwrap(), RelPath::root());
        assert_eq!(RelPath::parse("/").unwrap(), RelPath::root());
        assert_eq!(RelPath::parse("./a//b/./c/").unwrap().as_str(), "a/b/c");
    }

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        for bad in [
            "..",
            "../x",
            "a/../../b",
            "a/..",
            "/etc/passwd",
            "//etc",
            "a\\..\\b",
            "a\0b",
            ".trash",
            ".trash/x",
            "./.trash",
        ] {
            assert!(RelPath::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        // A trash-like name below the root is just a name.
        assert!(RelPath::parse("world/.trash").is_ok());
    }

    #[test]
    fn parent_name_join() {
        let p = RelPath::parse("config/paper/global.yml").unwrap();
        assert_eq!(p.file_name(), Some("global.yml"));
        assert_eq!(p.parent().unwrap().as_str(), "config/paper");
        assert_eq!(RelPath::parse("a").unwrap().parent(), Some(RelPath::root()));
        assert_eq!(RelPath::root().parent(), None);
        assert_eq!(RelPath::root().join("a").unwrap().as_str(), "a");
        assert!(RelPath::root().join("..").is_err());
        assert!(RelPath::root().join("a/b").is_err());
        assert!(RelPath::root().join(TRASH_DIR).is_err());
    }

    #[test]
    fn starts_with_is_component_wise() {
        let p = |s| RelPath::parse(s).unwrap();
        assert!(p("world/region").starts_with(&p("world")));
        assert!(p("world").starts_with(&p("world")));
        assert!(!p("world_nether").starts_with(&p("world")));
        assert!(p("anything").starts_with(&RelPath::root()));
    }
}
