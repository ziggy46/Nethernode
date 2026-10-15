//! What the panel asks an agent, and what the agent tells the panel.
//!
//! Each request opens its own stream: the panel sends a [`Request`] and
//! reads one [`Reply`] (a console subscription keeps reading lines). The
//! agent keeps one more stream open for [`AgentEvent`]s.

use nn_events::Event;
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
        /// What to create, with the panel's slug and ports to avoid.
        input: CreateServer,
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
            NodeError::Unavailable(_) | NodeError::NoDataFolder | NodeError::Offline(_) => {
                ErrorKind::Unavailable
            }
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
