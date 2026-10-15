//! `GET /api/backups/{id}/download`: a backup archive, with range support so
//! an interrupted download can resume.

use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, StatusCode};

use crate::AppState;
use crate::files_http::{authorize, stream_file};
use crate::middleware::CurrentUser;

pub(crate) async fn download(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let backup = match state.auth.store().find_backup(&id).await {
        Ok(Some(b)) if b.status == "ready" => b,
        Ok(_) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, "couldn't load a backup");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    match super::node_of(&state, &backup)
        .open_backup(&backup.file_name)
        .await
    {
        Some(file) => stream_file(file, &backup.file_name, &headers).await,
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
