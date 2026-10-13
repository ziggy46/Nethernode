//! Plain-HTTP file transfer for the file manager:
//!
//! - `GET /api/files/download?server=<id>&path=<path>` streams a file, with
//!   single-range support so interrupted downloads can resume.
//!
//! Uploads use tus (see [`crate::uploads`]). Downloads need a session with
//! permission to manage servers.

use std::io::SeekFrom;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use connectrpc::ConnectError;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use nn_auth::{AuthUser, Permission};
use nn_files::RelPath;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::AppState;
use crate::files::{blocking, file_error, open};
use crate::middleware::CurrentUser;

#[derive(serde::Deserialize)]
pub(crate) struct FileQuery {
    server: String,
    path: String,
}

pub(crate) fn authorize(user: Option<Extension<CurrentUser>>) -> Result<AuthUser, ConnectError> {
    let Some(Extension(CurrentUser(authed))) = user else {
        return Err(ConnectError::unauthenticated("sign in required"));
    };
    if !authed.user.role.can(Permission::ManageServers) {
        return Err(ConnectError::permission_denied(
            "your role doesn't allow this action",
        ));
    }
    Ok(authed.user)
}

pub(crate) async fn download(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let path = match RelPath::parse(&query.path) {
        Ok(path) => path,
        Err(e) => return file_error(e).into_response(),
    };
    let files = match open(&state, &query.server).await {
        Ok((_, files)) => files,
        Err(e) => return e.into_response(),
    };
    let opened = path.clone();
    let (file, size) = match blocking(move || files.open_read(&opened)).await {
        Ok(found) => found,
        Err(e) => return e.into_response(),
    };

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, size));
    let (status, start, len) = match range {
        Some(Some(Ok((start, end)))) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        Some(Some(Err(()))) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response();
        }
        // No Range header, or one we don't support: send everything.
        Some(None) | None => (StatusCode::OK, 0, size),
    };
    let mut file = tokio::fs::File::from_std(file);
    if start > 0
        && let Err(e) = file.seek(SeekFrom::Start(start)).await
    {
        tracing::warn!(error = %e, "seek for range download failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let body = Body::from_stream(ReaderStream::new(file.take(len)));

    let mut response = Response::new(body);
    *response.status_mut() = status;
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    // Already final bytes: keeps the compression layer off, which would
    // break lengths and ranges.
    h.insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("identity"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(v) =
        HeaderValue::from_str(&content_disposition(path.file_name().unwrap_or("download")))
    {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + len - 1))
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    response
}

/// Parses a single `bytes=` range against a file of `size` bytes.
/// `None`: not a form we serve (send the whole file). `Some(Err)`:
/// unsatisfiable.
fn parse_range(value: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    let range = if start.is_empty() {
        // Suffix: the last N bytes.
        let n: u64 = end.parse().ok()?;
        if n == 0 || size == 0 {
            return Some(Err(()));
        }
        (size.saturating_sub(n), size - 1)
    } else {
        let start: u64 = start.parse().ok()?;
        let end: u64 = if end.is_empty() {
            size.saturating_sub(1)
        } else {
            end.parse::<u64>().ok()?.min(size.saturating_sub(1))
        };
        if start >= size || end < start {
            return Some(Err(()));
        }
        (start, end)
    };
    Some(Ok(range))
}

/// `attachment` with an ASCII fallback name and the exact UTF-8 name.
fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some(Ok((0, 9))));
        assert_eq!(parse_range("bytes=90-", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=-10", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=-500", 100), Some(Ok((0, 99))));
        assert_eq!(parse_range("bytes=50-500", 100), Some(Ok((50, 99))));
        assert_eq!(parse_range("bytes=100-", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=9-0", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=x-1", 100), None);
    }

    #[test]
    fn disposition_escapes_names() {
        assert_eq!(
            content_disposition("world.zip"),
            "attachment; filename=\"world.zip\"; filename*=UTF-8''world.zip"
        );
        assert_eq!(
            content_disposition("my \"wörld\".zip"),
            "attachment; filename=\"my _w_rld_.zip\"; filename*=UTF-8''my%20%22w%C3%B6rld%22.zip"
        );
    }
}
