//! The embedded web UI.
//!
//! Files in `web/dist` are compiled into the binary in release builds (and
//! read from disk in debug builds). Hashed files under `assets/` are cached
//! for a year; everything else, including `index.html`, is revalidated.
//!
//! Page navigations that don't match a file get `index.html`, after these
//! redirects:
//! - before first-run setup, every page except `/setup` goes to `/setup`
//! - after setup, `/setup` goes to `/login` (or `/` when signed in)
//! - signed-out visitors go to `/login`; signed-in visitors on `/login` go to `/`

use std::borrow::Cow;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Redirect, Response};
use http::{HeaderValue, Method, StatusCode, header};
use rust_embed::RustEmbed;

use crate::AppState;
use crate::middleware::CurrentUser;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

/// Fallback handler: static files, then the SPA shell.
pub(crate) async fn serve(State(state): State<AppState>, req: Request) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = req.uri().path().trim_start_matches('/');

    if !path.is_empty()
        && let Some(file) = Assets::get(path)
    {
        let cache = if path.starts_with("assets/") {
            IMMUTABLE
        } else {
            REVALIDATE
        };
        return file_response(path, file.data, cache);
    }

    // Unknown API or asset paths are real 404s, not the SPA.
    if path.starts_with("api/") || path.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }

    let signed_in = req.extensions().get::<CurrentUser>().is_some();
    let page = path.trim_end_matches('/');
    let needs_setup = match state.auth.needs_setup().await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "could not check setup state");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };
    let redirect = match (needs_setup, page, signed_in) {
        (true, "setup", _) => None,
        (true, _, _) => Some("/setup"),
        (false, "setup" | "login", true) => Some("/"),
        (false, "setup", false) => Some("/login"),
        (false, "login", false) => None,
        (false, _, false) => Some("/login"),
        (false, _, true) => None,
    };
    if let Some(to) = redirect {
        return Redirect::to(to).into_response();
    }

    match Assets::get("index.html") {
        Some(index) => file_response("index.html", index.data, REVALIDATE),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "The web UI has not been built. Run `just build`, or `just dev` for development.",
        )
            .into_response(),
    }
}

fn file_response(path: &str, data: Cow<'static, [u8]>, cache: &'static str) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let content_type = HeaderValue::from_str(mime.as_ref())
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
        ],
        data,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::Assets;

    /// Guards the embed path: if the UI has been built, it must be embedded.
    #[test]
    fn embeds_built_ui_when_present() {
        let dist =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist/index.html");
        if dist.exists() {
            assert!(
                Assets::get("index.html").is_some(),
                "web/dist exists but was not embedded"
            );
        }
    }
}
