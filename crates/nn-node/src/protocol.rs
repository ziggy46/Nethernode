//! What the panel asks an agent, and what the agent tells the panel.
//!
//! Each request opens its own stream: the panel sends a [`Request`] and
//! reads one [`Reply`] (a console subscription keeps reading lines). The
//! agent keeps one more stream open for [`AgentEvent`]s.

use nn_events::Event;
use nn_files::{Expect, FileError, OnConflict, RelPath};
use nn_orchestrator::{CreateServer, OrchestratorError};
use nn_store::{Server, ServerSettings};
use serde::{Deserialize, Serialize};

use crate::NodeError;

/// A request from the panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The agent's servers.
    ListServers,
    /// Create a server; the reply is the new server.
    Create {
        /// What to create, with the panel's slug and ports to avoid
        /// (boxed: it's much larger than the other requests).
        input: Box<CreateServer>,
    },
    /// Start a stopped server.
    Start {
        /// Server ID.
        id: String,
    },
    /// Start a sleeping server for a joining player.
    Wake {
        /// Server ID.
        id: String,
    },
    /// Stop a server, or put it to sleep.
    Stop {
        /// Server ID.
        id: String,
        /// Sleep instead of stopping.
        sleep: bool,
    },
    /// Stop and start a server.
    Restart {
        /// Server ID.
        id: String,
    },
    /// Stop a server at once.
    Kill {
        /// Server ID.
        id: String,
    },
    /// Change settings; the reply is the server and whether it needs a
    /// restart.
    Update {
        /// Server ID.
        id: String,
        /// The new settings.
        settings: ServerSettings,
    },
    /// Delete a server, and with `delete_data` its files.
    Delete {
        /// Server ID.
        id: String,
        /// Delete its files too.
        delete_data: bool,
    },
    /// Whether the server's container is running.
    ContainerRunning {
        /// Server ID.
        id: String,
    },
    /// Whether the server's settings changed since its container was made.
    RestartRequired {
        /// Server ID.
        id: String,
    },
    /// Up to an hour of resource samples.
    StatsHistory {
        /// Server ID.
        id: String,
    },
    /// Run a console command; the reply is its output.
    Command {
        /// Server ID.
        id: String,
        /// The command.
        command: String,
    },
    /// Follow the console: a [`Reply`] with the backlog, then one
    /// [`LogLine`] per line until the stream closes.
    Logs {
        /// Server ID.
        id: String,
    },
    /// Open the server's game port: a [`Reply`], then the stream carries a
    /// player's connection, bytes as they are.
    Connect {
        /// Server ID.
        id: String,
    },
    /// Something in a server's folder; the reply is a [`FolderReply`]. Some
    /// operations send or receive the file's bytes, raw, right after their
    /// request or reply (see [`FolderOp`]).
    Folder {
        /// Server ID.
        id: String,
        /// What to do.
        call: FolderOp,
    },
    /// Free space where backups go.
    BackupsFreeSpace,
    /// Archive a server as `name`, reporting progress on job `job_id`.
    Backup {
        /// Server ID.
        id: String,
        /// Archive name.
        name: String,
        /// Job ID for progress events.
        job_id: String,
        /// Top-level folders to leave out (a world, for a copy without it).
        #[serde(default)]
        leave_out: Vec<String>,
    },
    /// Restore archive `source` over a stopped server, after archiving
    /// what's there as `pre_restore`.
    Restore {
        /// Server ID.
        id: String,
        /// Archive to restore.
        source: String,
        /// Name for the copy of what's there now.
        pre_restore: String,
        /// Job ID for progress events.
        job_id: String,
    },
    /// An archive's size, or none if it isn't there.
    BackupSize {
        /// Archive name.
        name: String,
    },
    /// Streams an archive from `offset`: a reply, then its bytes.
    OpenBackup {
        /// Archive name.
        name: String,
        /// Where to start.
        offset: u64,
    },
    /// Delete an archive.
    DeleteBackup {
        /// Archive name.
        name: String,
    },
    /// Update this agent to NetherNode `version`. The reply says how it went;
    /// on success the agent then restarts as that version. Every version
    /// must keep accepting this request as it is, since it's how an agent
    /// too old for the panel gets updated.
    SelfUpdate {
        /// The version to run, like `0.2.1`.
        version: String,
    },
    /// How much space a server's folder and worlds take; the reply is a
    /// `FolderUsage`, or none.
    FolderUsage {
        /// Server ID.
        id: String,
    },
    /// Connect to a server's management protocol port. The reply is its
    /// secret (none if it has no management port), then the stream carries
    /// bytes both ways.
    Management {
        /// Server ID.
        id: String,
    },
    /// Receive an archive from another machine, for a server moving here.
    /// Its bytes follow the request.
    ReceiveBackup {
        /// Archive name.
        name: String,
        /// Bytes that follow.
        len: u64,
    },
    /// Set up a server moving here from a received archive; the reply is
    /// this machine's record of it.
    Adopt {
        /// The panel's record of the server, without secrets.
        server: Box<Server>,
        /// The received archive.
        archive: String,
        /// Ports other machines' servers use.
        avoid_ports: Vec<i64>,
        /// Job to report progress on.
        job_id: String,
    },
    /// The CurseForge API key, for CurseForge modpacks.
    SetCurseforgeKey {
        /// The key, or none.
        key: Option<String>,
    },
}

/// The answer to a [`Request`]: its result as JSON, or why it failed.
pub type Reply = Result<serde_json::Value, RemoteError>;

/// A line of console output, after a [`Request::Logs`] backlog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// The line.
    pub line: String,
}

/// Why a request failed on the agent, in kinds that map to the same
/// errors (and messages) as on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RemoteError {
    /// What sort of failure.
    pub kind: ErrorKind,
    /// The message, written for admins.
    pub message: String,
}

/// Sorts of [`RemoteError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No such server.
    NotFound,
    /// Not possible in the server's current state.
    InvalidState,
    /// The input was refused.
    InvalidInput,
    /// No ports left.
    NoPorts,
    /// A console command couldn't be delivered.
    Command,
    /// Docker (or something else the agent needs) isn't reachable.
    Unavailable,
    /// Something else went wrong.
    Internal,
}

impl From<&NodeError> for RemoteError {
    fn from(e: &NodeError) -> Self {
        let kind = match e {
            NodeError::Orchestrator(o) => match o {
                OrchestratorError::NotFound => ErrorKind::NotFound,
                OrchestratorError::InvalidState(_) => ErrorKind::InvalidState,
                OrchestratorError::InvalidInput(_) => ErrorKind::InvalidInput,
                OrchestratorError::NoPorts(_) => ErrorKind::NoPorts,
                OrchestratorError::Command(_) => ErrorKind::Command,
                OrchestratorError::Docker(_) => ErrorKind::Unavailable,
                _ => ErrorKind::Internal,
            },
            NodeError::Unavailable(_)
            | NodeError::NoDataFolder
            | NodeError::Offline(_)
            | NodeError::NeedsUpdate(_) => ErrorKind::Unavailable,
            NodeError::Remote(r) => r.kind,
            NodeError::File(_) | NodeError::NotYet(_) => ErrorKind::Internal,
        };
        Self {
            kind,
            message: e.to_string(),
        }
    }
}

/// What the agent tells the panel unprompted, on its events stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Every server the agent has (on connecting, and after missing
    /// events).
    Servers {
        /// The servers.
        servers: Vec<Server>,
    },
    /// A server as it is now (after any change).
    Server {
        /// The server.
        server: Box<Server>,
    },
    /// A server was deleted.
    Removed {
        /// Server ID.
        id: String,
    },
    /// Something happened, for the panel's own event stream.
    Event {
        /// The event.
        event: Event,
    },
}

/// Operations on a server's folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FolderOp {
    /// A folder's entries.
    List {
        /// The folder.
        path: RelPath,
    },
    /// One entry.
    Stat {
        /// The entry.
        path: RelPath,
    },
    /// A whole file, up to `limit` bytes: a [`FileHeader`] reply, then
    /// its bytes.
    Read {
        /// The file.
        path: RelPath,
        /// Largest size to read.
        limit: u64,
    },
    /// Writes a file: `len` bytes follow the request. Replies with the new
    /// version.
    Write {
        /// The file.
        path: RelPath,
        /// What must be there now.
        expect: Expect,
        /// How many bytes follow.
        len: u64,
    },
    /// Makes a folder.
    Mkdir {
        /// The folder.
        path: RelPath,
    },
    /// Moves or renames.
    Rename {
        /// From.
        from: RelPath,
        /// To.
        to: RelPath,
    },
    /// Copies.
    Copy {
        /// From.
        from: RelPath,
        /// To.
        to: RelPath,
    },
    /// Moves files to the trash, clearing out older trash first.
    Delete {
        /// What to delete.
        paths: Vec<RelPath>,
        /// How long trash is kept.
        retention_secs: u64,
    },
    /// What's in the trash.
    Trash {
        /// How long trash is kept.
        retention_secs: u64,
    },
    /// Puts a trashed item back.
    Restore {
        /// Trash item ID.
        id: String,
    },
    /// Deletes trash for good: one item, or all of it.
    Purge {
        /// Trash item ID, or none for all.
        id: Option<String>,
    },
    /// A file's SHA-1.
    Sha1 {
        /// The file.
        path: RelPath,
    },
    /// What a mod jar says about itself.
    ModInfo {
        /// The jar.
        path: RelPath,
    },
    /// A mod jar's icon: a reply with its length (or none), then its bytes.
    ModIcon {
        /// The jar.
        path: RelPath,
    },
    /// Extracts an archive as a job; the reply is the job ID.
    Extract {
        /// The archive.
        archive: RelPath,
        /// Where to.
        dest: RelPath,
        /// Replace files that exist.
        overwrite: bool,
    },
    /// Compresses files into a zip as a job; the reply is the job ID.
    Compress {
        /// What to compress.
        sources: Vec<RelPath>,
        /// The zip.
        dest: RelPath,
    },
    /// Streams a file from `offset`: a reply with its size, then its bytes
    /// to the end.
    OpenRead {
        /// The file.
        path: RelPath,
        /// Where to start.
        offset: u64,
    },
    /// Places an uploaded file: `len` bytes follow the request. Replies
    /// with where it went.
    Adopt {
        /// The folder.
        dir: RelPath,
        /// The file name.
        name: String,
        /// What to do if the name is taken.
        on_conflict: OnConflict,
        /// How many bytes follow.
        len: u64,
    },
}

/// The reply to a [`Request::Folder`].
pub type FolderReply = Result<serde_json::Value, FileFailure>;

/// A file's details, sent before its bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHeader {
    /// How many bytes follow.
    pub len: u64,
    /// Whether it's text.
    pub is_text: bool,
    /// Its version, for writing it back.
    pub version: String,
    /// Last change, in Unix milliseconds.
    pub modified_ms: i64,
}

/// A [`FileError`] on its way between machines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileFailure {
    /// Nothing at the path.
    NotFound {
        /// The path.
        path: String,
    },
    /// Something already at the path.
    AlreadyExists {
        /// The path.
        path: String,
    },
    /// Changed since it was read.
    Conflict,
    /// A folder where a file was expected.
    IsDirectory {
        /// The path.
        path: String,
    },
    /// A file where a folder was expected.
    NotDirectory {
        /// The path.
        path: String,
    },
    /// Over a limit.
    TooLarge {
        /// What.
        what: String,
        /// The limit.
        limit: String,
    },
    /// Not allowed.
    PermissionDenied {
        /// The path.
        path: String,
    },
    /// Anything else that's the request's fault.
    Invalid {
        /// Why.
        message: String,
    },
    /// Anything else.
    Io {
        /// What happened.
        message: String,
    },
}

impl From<&FileError> for FileFailure {
    fn from(e: &FileError) -> Self {
        match e {
            FileError::NotFound(path) => Self::NotFound { path: path.clone() },
            FileError::AlreadyExists(path) => Self::AlreadyExists { path: path.clone() },
            FileError::Conflict => Self::Conflict,
            FileError::IsDirectory(path) => Self::IsDirectory { path: path.clone() },
            FileError::NotDirectory(path) => Self::NotDirectory { path: path.clone() },
            FileError::TooLarge { what, limit } => Self::TooLarge {
                what: what.clone(),
                limit: limit.clone(),
            },
            FileError::PermissionDenied(path) => Self::PermissionDenied { path: path.clone() },
            FileError::InvalidPath(_) | FileError::Invalid(_) => Self::Invalid {
                message: e.to_string(),
            },
            FileError::Io(_) => Self::Io {
                message: e.to_string(),
            },
        }
    }
}

impl From<FileFailure> for FileError {
    fn from(f: FileFailure) -> Self {
        match f {
            FileFailure::NotFound { path } => Self::NotFound(path),
            FileFailure::AlreadyExists { path } => Self::AlreadyExists(path),
            FileFailure::Conflict => Self::Conflict,
            FileFailure::IsDirectory { path } => Self::IsDirectory(path),
            FileFailure::NotDirectory { path } => Self::NotDirectory(path),
            FileFailure::TooLarge { what, limit } => Self::TooLarge { what, limit },
            FileFailure::PermissionDenied { path } => Self::PermissionDenied(path),
            FileFailure::Invalid { message } => Self::Invalid(message),
            FileFailure::Io { message } => Self::Io(std::io::Error::other(message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_from_the_other_side_are_checked_like_any_other() {
        let ok: FolderOp =
            serde_json::from_str(r#"{"op":"stat","path":"world/level.dat"}"#).unwrap();
        assert!(matches!(ok, FolderOp::Stat { path } if path.as_str() == "world/level.dat"));
        for bad in ["../etc/passwd", "/etc/passwd", "world/../../x"] {
            let json = format!(r#"{{"op":"stat","path":"{bad}"}}"#);
            assert!(serde_json::from_str::<FolderOp>(&json).is_err(), "{bad}");
        }
    }

    #[test]
    fn file_errors_survive_the_trip() {
        for e in [
            FileError::NotFound("a.txt".into()),
            FileError::Conflict,
            FileError::TooLarge {
                what: "a.txt".into(),
                limit: "5 MB".into(),
            },
        ] {
            let back: FileError = FileFailure::from(&e).into();
            assert_eq!(back.to_string(), e.to_string());
        }
    }
}
