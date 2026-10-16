//! Shared plumbing for file access: opening a server's folder and mapping
//! file errors to RPC codes.

use connectrpc::ConnectError;
use nn_files::FileError;
use nn_node::Folder;
use nn_store::Server;

use crate::AppState;
use crate::rpc::servers::node_error;
use crate::rpc::store_error;

/// The folder of server `server_id`, on whichever node it runs.
pub(crate) async fn open(
    state: &AppState,
    server_id: &str,
) -> Result<(Server, Folder), ConnectError> {
    let server = state
        .auth
        .store()
        .find_server(server_id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| ConnectError::not_found("server not found"))?;
    crate::moves::not_moving(state, &server.id)?;
    let folder = state
        .nodes
        .for_server(&server)
        .folder(&server)
        .await
        .map_err(node_error)?;
    Ok((server, folder))
}

/// Maps file errors to Connect codes. Messages are written for admins.
pub(crate) fn file_error(err: FileError) -> ConnectError {
    match err {
        FileError::InvalidPath(_) | FileError::Invalid(_) => {
            ConnectError::invalid_argument(err.to_string())
        }
        FileError::NotFound(_) => ConnectError::not_found(err.to_string()),
        FileError::AlreadyExists(_) => ConnectError::already_exists(err.to_string()),
        FileError::Conflict => ConnectError::aborted(err.to_string()),
        FileError::IsDirectory(_) | FileError::NotDirectory(_) | FileError::TooLarge { .. } => {
            ConnectError::failed_precondition(err.to_string())
        }
        FileError::PermissionDenied(_) => ConnectError::permission_denied(err.to_string()),
        FileError::Io(ref e) => {
            tracing::error!(error = %e, "file operation failed");
            ConnectError::internal(format!("the file operation failed: {e}"))
        }
    }
}
