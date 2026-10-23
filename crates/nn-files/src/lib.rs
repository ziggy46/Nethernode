//! Sandboxed file manager scoped to a server's data directory.
//!
//! Every path comes from the browser, so [`RelPath`] validates it first and
//! `cap-std` then confines all access to the server's folder, symlinks
//! included. Deletions go to a restorable trash, writes are atomic and
//! detect concurrent changes, and zip extraction is protected against
//! zip-slip and zip bombs.

mod adopt;
mod archive;
pub mod backup;
mod error;
mod owner;
mod path;
pub mod properties;
mod sandbox;
mod trash;
mod world;

pub use adopt::{Adopted, OnConflict};
pub use archive::{CompressReport, ExtractReport, Limits, Progress};
pub use error::{FileError, Result};
pub use owner::Owner;
pub use path::{RelPath, TRASH_DIR, validate_name};
pub use properties::Properties;
pub use sandbox::{CopyReport, EDITOR_LIMIT, Entry, EntryKind, Expect, FileContent, ServerFiles};
pub use trash::{TRASH_RETENTION, TrashItem};
pub use world::{Dimension, TrimOptions, TrimReport};
