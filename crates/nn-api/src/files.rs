//! Shared plumbing for file access: opening a server's folder, mapping file
//! errors to RPC codes, and running blocking work and background jobs.

use std::sync::Arc;

use connectrpc::ConnectError;
use nn_events::Event;
use nn_files::{FileError, Limits, RelPath, ServerFiles};
use nn_store::Server;

use crate::AppState;
use crate::rpc::store_error;

/// Opens the folder of server `server_id` as a sandbox.
pub(crate) async fn open(
    state: &AppState,
    server_id: &str,
) -> Result<(Server, Arc<ServerFiles>), ConnectError> {
    let server = state
        .auth
        .store()
        .find_server(server_id)
        .await
        .map_err(store_error)?
        .ok_or_else(|| ConnectError::not_found("server not found"))?;
    let Some(servers_dir) = state.servers.servers_dir.clone() else {
        return Err(ConnectError::unavailable(
            "the data directory isn't available, so server files can't be reached",
        ));
    };
    // Slugs are validated when servers are created; check again since this
    // becomes a path.
    if nn_files::validate_name(&server.slug).is_err() || server.slug.starts_with('.') {
        return Err(ConnectError::internal("invalid server folder"));
    }
    let root = servers_dir.join(&server.slug);
    let files = blocking(move || match ServerFiles::open(&root) {
        Err(FileError::NotFound(_)) => Err(FileError::Invalid(
            "this server's folder doesn't exist yet; start the server once to create it".into(),
        )),
        other => other,
    })
    .await?;
    Ok((server, Arc::new(files)))
}

/// Runs blocking file work off the async runtime.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, ConnectError>
where
    F: FnOnce() -> Result<T, FileError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result.map_err(file_error),
        Err(e) => {
            tracing::error!(error = %e, "file task failed");
            Err(ConnectError::internal("internal error"))
        }
    }
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

/// Runs a long file job (extract, compress) in the background, publishing
/// `file_job_progress` and `file_job_finished` events. Returns the job ID.
pub(crate) fn spawn_job<F>(state: &AppState, server_id: &str, kind: &'static str, work: F) -> String
where
    F: FnOnce(&mut dyn FnMut(u64, u64)) -> Result<String, FileError> + Send + 'static,
{
    let job_id = nn_store::new_id();
    let events = state.events.clone();
    let (server_id, id) = (server_id.to_owned(), job_id.clone());
    tokio::task::spawn_blocking(move || {
        let mut last: Option<u8> = None;
        let mut progress = |done: u64, total: u64| {
            let percent = (done.min(total) * 100)
                .checked_div(total)
                .map_or(0, |p| u8::try_from(p).unwrap_or(100));
            if last != Some(percent) {
                last = Some(percent);
                events.publish(Event::FileJobProgress {
                    server_id: server_id.clone(),
                    job_id: id.clone(),
                    kind: kind.to_owned(),
                    percent,
                });
            }
        };
        let (message, failed) = match work(&mut progress) {
            Ok(message) => (message, false),
            Err(e) => {
                tracing::warn!(server = %server_id, kind, error = %e, "file job failed");
                (e.to_string(), true)
            }
        };
        events.publish(Event::FileJobFinished {
            server_id,
            job_id: id,
            kind: kind.to_owned(),
            message,
            failed,
        });
    });
    job_id
}

/// Extracts `archive` into `dest` as a background job. Returns the job ID.
pub(crate) fn start_extract(
    state: &AppState,
    server_id: &str,
    files: Arc<ServerFiles>,
    archive: RelPath,
    dest: RelPath,
    overwrite: bool,
) -> String {
    spawn_job(state, server_id, "extract", move |progress| {
        let report = files.extract(&archive, &dest, overwrite, Limits::default(), progress)?;
        let mut message = format!(
            "Extracted {} files from {archive} into {dest}",
            report.files
        );
        if report.skipped_links > 0 {
            message.push_str(&format!(" ({} links left out)", report.skipped_links));
        }
        Ok(message)
    })
}
