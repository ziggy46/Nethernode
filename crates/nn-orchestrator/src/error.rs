//! Orchestrator errors.

/// Why an orchestrator operation failed. Messages are suitable for display.
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    /// No server with that ID.
    #[error("server not found")]
    NotFound,
    /// The operation doesn't make sense in the server's current state.
    #[error("{0}")]
    InvalidState(String),
    /// User input failed validation.
    #[error("{0}")]
    InvalidInput(String),
    /// No free ports left in the configured range.
    #[error("no free ports left in {0}; widen the range in the configuration")]
    NoPorts(String),
    /// Docker reported an error.
    #[error("Docker: {0}")]
    Docker(#[from] bollard::errors::Error),
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] nn_store::Error),
    /// The server's secrets couldn't be read.
    #[error(transparent)]
    Secret(#[from] nn_auth::secrets::SecretError),
    /// Filesystem failure (data directories).
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias.
pub type Result<T, E = OrchestratorError> = std::result::Result<T, E>;
