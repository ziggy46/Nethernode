use std::io;

/// Why a file operation failed. Messages are shown to admins as-is.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// The path is malformed or not allowed.
    #[error("invalid path: {0}")]
    InvalidPath(&'static str),
    /// Nothing exists at the path.
    #[error("{0} doesn't exist")]
    NotFound(String),
    /// Something already exists at the path.
    #[error("{0} already exists")]
    AlreadyExists(String),
    /// The file changed after it was read, so saving would lose those changes.
    #[error("the file changed since it was opened; reload it before saving")]
    Conflict,
    /// A folder where a file was expected.
    #[error("{0} is a folder")]
    IsDirectory(String),
    /// A file where a folder was expected.
    #[error("{0} isn't a folder")]
    NotDirectory(String),
    /// Over a size or count limit.
    #[error("{what} is too large (limit {limit})")]
    TooLarge {
        /// What was too large.
        what: String,
        /// The limit, human-readable.
        limit: String,
    },
    /// The operation doesn't make sense, e.g. moving a folder into itself.
    #[error("{0}")]
    Invalid(String),
    /// The OS refused, or the path leads outside the server's folder.
    #[error("not allowed: {0}")]
    PermissionDenied(String),
    /// Any other I/O failure.
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl FileError {
    /// Maps an I/O error on `path` to a specific variant where one fits.
    pub(crate) fn at(path: &str, err: io::Error) -> Self {
        let shown = if path.is_empty() {
            "the server folder"
        } else {
            path
        };
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound(shown.to_owned()),
            io::ErrorKind::AlreadyExists => Self::AlreadyExists(shown.to_owned()),
            io::ErrorKind::IsADirectory => Self::IsDirectory(shown.to_owned()),
            io::ErrorKind::NotADirectory => Self::NotDirectory(shown.to_owned()),
            io::ErrorKind::PermissionDenied => Self::PermissionDenied(shown.to_owned()),
            _ => Self::Io(err),
        }
    }
}

/// Result alias for this crate.
pub type Result<T, E = FileError> = std::result::Result<T, E>;
