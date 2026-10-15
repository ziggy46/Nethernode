//! `GET /api/catalog/icon?url=…`: modpack and mod icons, fetched by
//! NetherNode and cached, so browsers never contact the platforms' image
//! servers. Only Modrinth's and CurseForge's image hosts are allowed.

use std::time::{Duration, SystemTime};

use axum::Extension;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::middleware::CurrentUser;

const HOSTS: &[&str] = &["cdn.modrinth.com", "media.forgecdn.net"];
const FRESH: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// What NetherNode serves an icon URL from.
pub(crate) fn proxied(url: Option<&str>) -> String {
    match url.filter(|u| allowed(u)) {
        Some(url) => format!("/api/catalog/icon?url={}", encode(url)),
        None => String::new(),
    }
}

fn allowed(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.port().is_none()
        && parsed.host_str().is_some_and(|h| HOSTS.contains(&h))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Recognizes the image formats icons come in.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    }
}

fn image(bytes: Vec<u8>) -> Response {
    let Some(kind) = image_type(&bytes) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "private, max-age=86400"),
            // Never interpreted as anything but an image.
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'"),
        ],
        bytes,
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub(crate) struct IconQuery {
    url: String,
}

pub(crate) async fn icon(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Query(query): Query<IconQuery>,
) -> Response {
    if user.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !allowed(&query.url) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let name = hex::encode(Sha256::digest(query.url.as_bytes()));
    let path = state.content.icon_dir().map(|d| d.join(&name));
    if let Some(path) = &path
        && let Ok(meta) = tokio::fs::metadata(path).await
        && meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age < FRESH)
        && let Ok(bytes) = tokio::fs::read(path).await
    {
        return image(bytes);
    }

    let _permit = state.content.icon_fetches.acquire().await;
    let fetched = async {
        let response = state
            .content
            .icon_http
            .get(&query.url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (bytes.len() <= MAX_BYTES && image_type(&bytes).is_some()).then(|| bytes.to_vec())
    }
    .await;
    match fetched {
        Some(bytes) => {
            if let Some(path) = &path {
                if let Some(dir) = path.parent() {
                    let _ = tokio::fs::create_dir_all(dir).await;
                }
                let _ = tokio::fs::write(path, &bytes).await;
            }
            image(bytes)
        }
        // A stale icon beats none.
        None => match &path {
            Some(path) => match tokio::fs::read(path).await {
                Ok(bytes) => image(bytes),
                Err(_) => StatusCode::NOT_FOUND.into_response(),
            },
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_platform_image_hosts() {
        assert!(allowed("https://cdn.modrinth.com/data/AANobbMI/icon.png"));
        assert!(allowed(
            "https://media.forgecdn.net/avatars/thumbnails/1/2/a.png"
        ));
        assert!(!allowed("http://cdn.modrinth.com/x.png"), "https only");
        assert!(!allowed("https://cdn.modrinth.com:8443/x.png"));
        assert!(!allowed("https://cdn.modrinth.com.evil.example/x.png"));
        assert!(!allowed("https://127.0.0.1/x.png"));
        assert!(!allowed("file:///etc/passwd"));
        assert_eq!(proxied(Some("https://evil.example/a.png")), "");
        assert_eq!(
            proxied(Some("https://cdn.modrinth.com/a b.png")),
            "/api/catalog/icon?url=https%3A%2F%2Fcdn.modrinth.com%2Fa%20b.png"
        );
    }

    #[test]
    fn image_types() {
        assert_eq!(image_type(b"\x89PNG\r\n"), Some("image/png"));
        assert_eq!(image_type(b"<svg"), None, "no SVG (scripts)");
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8"), Some("image/webp"));
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct ModIconQuery {
    file: String,
}

/// `GET /api/servers/{id}/mods/icon?file=…`: the icon inside a mod jar.
pub(crate) async fn mod_icon(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    axum::extract::Path(server_id): axum::extract::Path<String>,
    Query(query): Query<ModIconQuery>,
) -> Response {
    if let Err(e) = crate::files_http::authorize(user) {
        return e.into_response();
    }
    let Ok(path) = crate::rpc::mods::mod_path(&query.file) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let files = match crate::files::open(&state, &server_id).await {
        Ok((_, files)) => files,
        Err(e) => return e.into_response(),
    };
    let found = files.mod_icon(&path).await.ok().flatten();
    match found {
        Some(bytes) => image(bytes),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
