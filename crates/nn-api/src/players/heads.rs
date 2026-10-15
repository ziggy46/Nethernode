//! `GET /api/players/{uuid}/head`: a player's face as a PNG, cropped from
//! their Mojang skin and cached on disk. Answers 404 when there's no skin
//! to show (offline-mode players, no network); the UI then draws its own
//! fallback head.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use axum::Extension;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use nn_players::mojang::render_head;
use nn_players::profile::normalize_uuid;

use crate::AppState;
use crate::middleware::CurrentUser;

const FRESH: Duration = Duration::from_secs(24 * 3600);
const MISSING_FRESH: Duration = Duration::from_secs(6 * 3600);
const SIZE: u32 = 64;

fn cache_dir(state: &AppState) -> Option<PathBuf> {
    state.content.heads_dir()
}

fn age(path: &std::path::Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

fn png(bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        bytes,
    )
        .into_response()
}

pub(crate) async fn head(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    user: Option<Extension<CurrentUser>>,
) -> Response {
    if user.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(uuid) = normalize_uuid(&uuid) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Only Mojang accounts (version 4 UUIDs) have skins; offline-mode
    // players have derived (version 3) ones.
    if uuid.as_bytes().get(14) != Some(&b'4') {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dir = cache_dir(&state);
    let cached = dir.as_ref().map(|d| d.join(format!("{uuid}.png")));
    let missing = dir.as_ref().map(|d| d.join(format!("{uuid}.none")));
    if let Some(path) = &cached
        && age(path).is_some_and(|a| a < FRESH)
        && let Ok(bytes) = tokio::fs::read(path).await
    {
        return png(bytes);
    }
    if missing
        .as_deref()
        .and_then(age)
        .is_some_and(|a| a < MISSING_FRESH)
    {
        return StatusCode::NOT_FOUND.into_response();
    }

    let _permit = state.players.head_fetches.acquire().await;
    let mojang = &state.players.mojang;
    let fetched = async {
        let Some(url) = mojang.skin_url(&uuid).await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let skin = mojang.skin(&url).await.map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || render_head(&skin, SIZE))
            .await
            .map_err(|e| e.to_string())?
            .map(Some)
    }
    .await;

    match fetched {
        Ok(Some(bytes)) => {
            if let (Some(dir), Some(path)) = (&dir, &cached) {
                let _ = tokio::fs::create_dir_all(dir).await;
                let _ = tokio::fs::write(path, &bytes).await;
            }
            png(bytes)
        }
        Ok(None) => {
            if let (Some(dir), Some(path)) = (&dir, &missing) {
                let _ = tokio::fs::create_dir_all(dir).await;
                let _ = tokio::fs::write(path, b"").await;
            }
            StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => {
            tracing::debug!(%uuid, %error, "couldn't fetch a player head");
            // A stale head beats none while Mojang is unreachable.
            match cached.as_ref() {
                Some(path) => match tokio::fs::read(path).await {
                    Ok(bytes) => png(bytes),
                    Err(_) => StatusCode::NOT_FOUND.into_response(),
                },
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    }
}
