//! The tus endpoints.

// Errors here are finished HTTP responses, returned straight to the client.
#![allow(clippy::result_large_err)]

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use nn_auth::AuthUser;
use nn_files::{EntryKind, OnConflict, RelPath, validate_name};
use nn_store::Upload;
use nn_store::uploads::status;
use nn_uploads::protocol::{
    self, CHECKSUM_ALGORITHMS, PATCH_CONTENT_TYPE, TUS_EXTENSIONS, TUS_VERSION,
};
use nn_uploads::{ChunkError, write_chunk};
use serde_json::json;

use super::finish;
use crate::AppState;
use crate::files::{blocking, open};
use crate::files_http::authorize;
use crate::middleware::CurrentUser;

/// Free space kept back when accepting an upload, so a big upload never
/// fills the disk to the last byte.
const DISK_MARGIN: u64 = 512 * 1024 * 1024;

/// Status tus clients treat as "checksum mismatch" (from the checksum
/// extension).
const CHECKSUM_MISMATCH: u16 = 460;

/// A response with the headers every tus response carries.
fn tus(status: StatusCode) -> Response {
    let mut response = status.into_response();
    let h = response.headers_mut();
    h.insert("tus-resumable", HeaderValue::from_static(TUS_VERSION));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// An error with a message for the uploader.
fn refuse(status: StatusCode, message: impl Into<String>) -> Response {
    let mut response = tus(status);
    *response.body_mut() = Body::from(message.into());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Checks `Tus-Resumable` (required on everything but `OPTIONS`).
fn check_version(headers: &HeaderMap) -> Result<(), Response> {
    if header_str(headers, "tus-resumable") == Some(TUS_VERSION) {
        Ok(())
    } else {
        let mut response = refuse(StatusCode::PRECONDITION_FAILED, "unsupported tus version");
        response
            .headers_mut()
            .insert("tus-version", HeaderValue::from_static(TUS_VERSION));
        Err(response)
    }
}

fn user_of(user: Option<Extension<CurrentUser>>) -> Result<AuthUser, Response> {
    authorize(user).map_err(|e| {
        let status = match e.code {
            connectrpc::ErrorCode::Unauthenticated => StatusCode::UNAUTHORIZED,
            _ => StatusCode::FORBIDDEN,
        };
        refuse(status, e.message.unwrap_or_default())
    })
}

fn set_offset(response: &mut Response, upload: &Upload, received: i64) {
    let h = response.headers_mut();
    h.insert("upload-offset", HeaderValue::from(received));
    h.insert("upload-length", HeaderValue::from(upload.total_size));
}

fn set_expires(response: &mut Response, expires_at: i64) {
    if let Ok(v) = HeaderValue::from_str(&protocol::http_date(expires_at)) {
        response.headers_mut().insert("upload-expires", v);
    }
}

/// Loads upload `id` if it belongs to `user`. Someone else's upload is
/// reported as missing.
async fn load(state: &AppState, user: &AuthUser, id: &str) -> Result<Upload, Response> {
    match state.auth.store().find_upload(id).await {
        Ok(Some(upload)) if upload.user_id == user.id => Ok(upload),
        Ok(_) => Err(refuse(StatusCode::NOT_FOUND, "no such upload")),
        Err(e) => {
            tracing::error!(error = %e, "couldn't load an upload");
            Err(refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))
        }
    }
}

/// `OPTIONS /api/uploads`
pub(crate) async fn describe(State(state): State<AppState>) -> Response {
    let mut response = tus(StatusCode::NO_CONTENT);
    let h = response.headers_mut();
    h.insert("tus-version", HeaderValue::from_static(TUS_VERSION));
    h.insert("tus-extension", HeaderValue::from_static(TUS_EXTENSIONS));
    h.insert(
        "tus-checksum-algorithm",
        HeaderValue::from_static(CHECKSUM_ALGORITHMS),
    );
    h.insert(
        "tus-max-size",
        HeaderValue::from(state.uploads.settings().max_file),
    );
    response
}

/// `POST /api/uploads`
pub(crate) async fn create(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    headers: HeaderMap,
) -> Response {
    match create_inner(&state, user, &headers).await {
        Ok(response) | Err(response) => response,
    }
}

async fn create_inner(
    state: &AppState,
    user: Option<Extension<CurrentUser>>,
    headers: &HeaderMap,
) -> Result<Response, Response> {
    check_version(headers)?;
    let user = user_of(user)?;
    let uploads = &state.uploads;
    let settings = uploads.settings();
    let Some(temp) = uploads.temp() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "uploads aren't available: the data folder can't be reached",
        ));
    };
    if headers.contains_key("upload-defer-length") {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "the upload's length must be known when it starts",
        ));
    }
    let length = header_str(headers, "upload-length")
        .and_then(protocol::parse_u64)
        .ok_or_else(|| {
            refuse(
                StatusCode::BAD_REQUEST,
                "Upload-Length is missing or invalid",
            )
        })?;
    if length > settings.max_file {
        return Err(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "the file is larger than the {} upload limit",
                human_size(settings.max_file)
            ),
        ));
    }
    let metadata = protocol::parse_metadata(header_str(headers, "upload-metadata").unwrap_or(""))
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))?;
    let field = |key: &str| metadata.get(key).and_then(|v| v.as_deref());

    let filename = field("filename")
        .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "the file name is missing"))?;
    validate_name(filename)
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, format!("{filename}: {e}")))?;
    let server_id = field("server")
        .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "the destination server is missing"))?;
    let dir = RelPath::parse(field("dir").unwrap_or(""))
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))?;
    dir.join(filename)
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))?;
    let on_conflict = match field("conflict") {
        None => OnConflict::Rename,
        Some(v) => OnConflict::parse(v).ok_or_else(|| {
            refuse(
                StatusCode::BAD_REQUEST,
                "conflict must be rename, overwrite, or skip",
            )
        })?,
    };
    let sha256 = field("sha256")
        .map(|v| {
            protocol::parse_sha256_hex(v)
                .map(hex::encode)
                .ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "sha256 must be 64 hex digits"))
        })
        .transpose()?;
    let extract = matches!(metadata.get("extract"), Some(None))
        || matches!(field("extract"), Some("1" | "true"));

    // The destination must be a real folder inside the server's own: the
    // sandbox refuses anything that escapes it, links included.
    let (server, files) = open(state, server_id)
        .await
        .map_err(|e| refuse(connect_status(&e), e.message.unwrap_or_default()))?;
    let checked = dir.clone();
    let entry = blocking(move || files.stat(&checked))
        .await
        .map_err(|e| refuse(connect_status(&e), e.message.unwrap_or_default()))?;
    if entry.kind != EntryKind::Directory {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            format!("{dir} isn't a folder"),
        ));
    }

    let store = state.auth.store();
    let in_flight = store
        .upload_bytes_in_flight(&user.id)
        .await
        .map_err(internal)?;
    if u64::try_from(in_flight).unwrap_or(0).saturating_add(length) > settings.quota {
        return Err(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "you have {} of unfinished uploads; finishing this one would pass the {} limit",
                human_size(u64::try_from(in_flight).unwrap_or(0)),
                human_size(settings.quota)
            ),
        ));
    }
    if let Some(free) = temp.available_space()
        && free < length.saturating_add(DISK_MARGIN)
    {
        return Err(refuse(
            StatusCode::INSUFFICIENT_STORAGE,
            format!(
                "not enough free disk space: {} needed, {} free",
                human_size(length),
                human_size(free)
            ),
        ));
    }

    let id = nn_store::new_id();
    let expires_at = nn_store::now_ms().saturating_add(uploads.expiry_ms());
    let length_i64 =
        i64::try_from(length).map_err(|_| refuse(StatusCode::PAYLOAD_TOO_LARGE, "too large"))?;
    store
        .insert_upload(nn_store::NewUpload {
            id: &id,
            user_id: &user.id,
            server_id: &server.id,
            dest_path: dir.as_str(),
            filename,
            total_size: length_i64,
            sha256: sha256.as_deref(),
            on_conflict: on_conflict.as_str(),
            extract,
            expires_at,
        })
        .await
        .map_err(internal)?;
    // The row exists before the file, so the janitor never mistakes a new
    // upload's file for an orphan.
    let (temp_for_create, id_for_create) = (temp.clone(), id.clone());
    let created =
        tokio::task::spawn_blocking(move || temp_for_create.create(&id_for_create, length)).await;
    if let Err(message) = match created {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::StorageFull => {
            Err("not enough free disk space for this upload".to_owned())
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "couldn't create an upload file");
            Err(format!("couldn't reserve space for the upload: {e}"))
        }
        Err(e) => Err(e.to_string()),
    } {
        let _ = store
            .transition_upload(&id, status::UPLOADING, status::FAILED, Some(&message))
            .await;
        return Err(refuse(StatusCode::INSUFFICIENT_STORAGE, message));
    }

    tracing::info!(upload = %id, server = %server.id, size = length, "upload started");
    let mut response = tus(StatusCode::CREATED);
    // Relative, so it resolves against whatever address the client used
    // (a tunnel or reverse proxy changes scheme and host).
    if let Ok(v) = HeaderValue::from_str(&format!("/api/uploads/{id}")) {
        response.headers_mut().insert(header::LOCATION, v);
    }
    set_expires(&mut response, expires_at);
    Ok(response)
}

/// `HEAD /api/uploads/{id}`
pub(crate) async fn offset(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        check_version(&headers)?;
        let user = user_of(user)?;
        let upload = load(&state, &user, &id).await?;
        match upload.status.as_str() {
            status::UPLOADING | status::FINISHING | status::DONE => {
                let mut response = tus(StatusCode::OK);
                set_offset(&mut response, &upload, upload.received);
                if upload.status == status::UPLOADING {
                    set_expires(&mut response, upload.expires_at);
                }
                Ok(response)
            }
            // tus clients start over on 410.
            _ => Err(tus(StatusCode::GONE)),
        }
    }
    .await;
    match result {
        Ok(response) | Err(response) => response,
    }
}

/// `PATCH /api/uploads/{id}`
pub(crate) async fn patch(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    match patch_inner(&state, user, &id, &headers, body).await {
        Ok(response) | Err(response) => response,
    }
}

async fn patch_inner(
    state: &AppState,
    user: Option<Extension<CurrentUser>>,
    id: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Response> {
    check_version(headers)?;
    let user = user_of(user)?;
    if header_str(headers, header::CONTENT_TYPE.as_str()) != Some(PATCH_CONTENT_TYPE) {
        return Err(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("Content-Type must be {PATCH_CONTENT_TYPE}"),
        ));
    }
    let offset = header_str(headers, "upload-offset")
        .and_then(protocol::parse_u64)
        .ok_or_else(|| {
            refuse(
                StatusCode::BAD_REQUEST,
                "Upload-Offset is missing or invalid",
            )
        })?;
    let checksum = header_str(headers, "upload-checksum")
        .map(protocol::parse_checksum)
        .transpose()
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))?;
    let settings = state.uploads.settings();
    if header_str(headers, header::CONTENT_LENGTH.as_str())
        .and_then(protocol::parse_u64)
        .is_some_and(|len| len > settings.chunk_limit)
    {
        return Err(chunk_too_large(settings.chunk_limit));
    }
    let Some(temp) = state.uploads.temp() else {
        return Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "uploads aren't available",
        ));
    };

    load(state, &user, id).await?;
    // One writer at a time; tus clients retry on 423.
    let Some(_guard) = state.uploads.try_lock(id) else {
        return Err(refuse(
            StatusCode::LOCKED,
            "another request is writing to this upload",
        ));
    };
    let upload = load(state, &user, id).await?;
    let received = u64::try_from(upload.received).unwrap_or(0);
    let total = u64::try_from(upload.total_size).unwrap_or(0);

    if upload.status != status::UPLOADING {
        // A retried final chunk after everything arrived: nothing to do.
        if (upload.status == status::FINISHING || upload.status == status::DONE) && offset == total
        {
            let mut response = tus(StatusCode::NO_CONTENT);
            set_offset(&mut response, &upload, upload.received);
            return Ok(response);
        }
        return Err(tus(StatusCode::GONE));
    }
    if offset != received {
        let mut response = refuse(
            StatusCode::CONFLICT,
            format!("the upload is at byte {received}, not {offset}"),
        );
        set_offset(&mut response, &upload, upload.received);
        return Err(response);
    }

    let path = temp
        .path(id)
        .map_err(|_| refuse(StatusCode::NOT_FOUND, "no such upload"))?;
    let max = settings.chunk_limit.min(total - received);
    let written = match write_chunk(&path, offset, max, body.into_data_stream(), checksum).await {
        Ok(chunk) => chunk,
        Err(ChunkError::TooLarge) if max < settings.chunk_limit => {
            return Err(refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                "the chunk goes past the end of the upload",
            ));
        }
        Err(ChunkError::TooLarge) => return Err(chunk_too_large(settings.chunk_limit)),
        Err(ChunkError::ChecksumMismatch) => {
            return Err(refuse(
                StatusCode::from_u16(CHECKSUM_MISMATCH).unwrap_or(StatusCode::BAD_REQUEST),
                "the chunk doesn't match its checksum",
            ));
        }
        Err(ChunkError::Interrupted) => {
            return Err(refuse(
                StatusCode::BAD_REQUEST,
                "the connection dropped mid-chunk",
            ));
        }
        Err(ChunkError::Io(e)) => {
            tracing::error!(upload = %id, error = %e, "writing an upload chunk failed");
            let status = if e.kind() == std::io::ErrorKind::StorageFull {
                StatusCode::INSUFFICIENT_STORAGE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            return Err(refuse(status, format!("couldn't save the chunk: {e}")));
        }
    };

    let now_received = received + written.written;
    let now_i64 = i64::try_from(now_received).unwrap_or(i64::MAX);
    let expires_at = nn_store::now_ms().saturating_add(state.uploads.expiry_ms());
    let store = state.auth.store();
    if written.written > 0
        && !store
            .set_upload_received(id, now_i64, expires_at)
            .await
            .map_err(internal)?
    {
        return Err(tus(StatusCode::GONE));
    }
    if written.interrupted {
        // The client is gone; it'll ask where to resume with HEAD.
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "the connection dropped mid-chunk",
        ));
    }
    if now_received == total
        && store
            .transition_upload(id, status::UPLOADING, status::FINISHING, None)
            .await
            .map_err(internal)?
    {
        state.uploads.forget(id);
        let mut upload = upload.clone();
        upload.received = now_i64;
        finish::spawn(state.clone(), upload);
    }

    let mut response = tus(StatusCode::NO_CONTENT);
    set_offset(&mut response, &upload, now_i64);
    if now_received < total {
        set_expires(&mut response, expires_at);
    }
    Ok(response)
}

/// `DELETE /api/uploads/{id}`
pub(crate) async fn cancel(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let result = async {
        check_version(&headers)?;
        let user = user_of(user)?;
        load(&state, &user, &id).await?;
        let Some(_guard) = state.uploads.try_lock(&id) else {
            return Err(refuse(
                StatusCode::LOCKED,
                "a chunk is being written; try again",
            ));
        };
        let cancelled = state
            .auth
            .store()
            .transition_upload(
                &id,
                status::UPLOADING,
                status::CANCELLED,
                Some("Cancelled."),
            )
            .await
            .map_err(internal)?;
        if !cancelled {
            // Already finished (or finishing): too late to cancel.
            return Err(tus(StatusCode::GONE));
        }
        if let Some(temp) = state.uploads.temp()
            && let Err(e) = temp.remove(&id)
        {
            tracing::warn!(upload = %id, error = %e, "couldn't delete a cancelled upload");
        }
        state.uploads.forget(&id);
        Ok(tus(StatusCode::NO_CONTENT))
    }
    .await;
    match result {
        Ok(response) | Err(response) => response,
    }
}

/// `GET /api/uploads`: the caller's unfinished uploads, so the UI can offer
/// to resume them.
pub(crate) async fn list(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
) -> Response {
    let user = match user_of(user) {
        Ok(user) => user,
        Err(response) => return response,
    };
    match state.auth.store().unfinished_uploads_for(&user.id).await {
        Ok(uploads) => {
            let items: Vec<_> = uploads
                .into_iter()
                .map(|u| {
                    json!({
                        "id": u.id,
                        "server_id": u.server_id,
                        "dir": u.dest_path,
                        "filename": u.filename,
                        "size": u.total_size,
                        "received": u.received,
                        "status": u.status,
                        "expires_at": u.expires_at,
                    })
                })
                .collect();
            let mut response = axum::Json(items).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => internal(e),
    }
}

fn chunk_too_large(limit: u64) -> Response {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("chunks can be at most {}", human_size(limit)),
    )
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "upload bookkeeping failed");
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

fn connect_status(e: &connectrpc::ConnectError) -> StatusCode {
    use connectrpc::ErrorCode as Code;
    match e.code {
        Code::NotFound => StatusCode::NOT_FOUND,
        Code::InvalidArgument | Code::FailedPrecondition => StatusCode::BAD_REQUEST,
        Code::PermissionDenied => StatusCode::FORBIDDEN,
        Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `5.0 GB`-style sizes for messages.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
