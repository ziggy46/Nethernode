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

/// Once a day, empties what's been in each server's trash longer than
/// [`nn_files::TRASH_RETENTION`]. (Opening the trash or deleting something
/// does it too, but a server nobody touches would otherwise keep it all.)
pub fn spawn_trash_cleanup(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
        loop {
            for server in state.auth.store().list_servers().await.unwrap_or_default() {
                if state.nodes.is_moving(&server.id) {
                    continue;
                }
                // Machines that are offline are tried again tomorrow.
                let Ok(folder) = state.nodes.for_server(&server).folder(&server).await else {
                    continue;
                };
                if let Err(e) = folder.trash(nn_files::TRASH_RETENTION).await {
                    tracing::debug!(server = %server.id, error = %e, "couldn't empty old items from the trash");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
        }
    });
}
