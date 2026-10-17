//! Minecraft item icons for inventories (see `nn_catalog::items`).
//!
//! - `GET /api/servers/{id}/items?ids=minecraft:stone,…`: how to draw each
//!   item in that server's Minecraft version.
//! - `GET /api/minecraft/{version}/textures/{path}.png`: a texture.

use std::collections::HashMap;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::files_http::authorize;
use crate::middleware::CurrentUser;

/// Items asked about at once, at most.
const MAX_IDS: usize = 200;

#[derive(Deserialize)]
pub(crate) struct IconQuery {
    ids: String,
}

#[derive(Serialize)]
struct Icons {
    /// The Minecraft version the textures come from.
    version: String,
    icons: HashMap<String, nn_catalog::items::Icon>,
}

fn failed(e: &nn_catalog::Error) -> Response {
    let status = match e {
        nn_catalog::Error::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_GATEWAY,
    };
    (status, e.to_string()).into_response()
}

/// How to draw the items in a server's inventories.
pub(crate) async fn icons(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path(id): Path<String>,
    Query(query): Query<IconQuery>,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let server = match state.auth.store().find_server(&id).await {
        Ok(Some(server)) => server,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let ids: Vec<String> = query
        .ids
        .split(',')
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .take(MAX_IDS)
        .map(str::to_owned)
        .collect();
    let content = &state.content;
    let version = server.mc_version.clone();
    match content
        .item_textures
        .icons(&content.catalog, &version, ids)
        .await
    {
        Ok((version, icons)) => Json(Icons { version, icons }).into_response(),
        Err(e) => failed(&e),
    }
}

/// One texture.
pub(crate) async fn texture(
    State(state): State<AppState>,
    user: Option<Extension<CurrentUser>>,
    Path((version, path)): Path<(String, String)>,
) -> Response {
    if let Err(e) = authorize(user) {
        return e.into_response();
    }
    let Some(path) = path.strip_suffix(".png") else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content = &state.content;
    match content
        .item_textures
        .texture(&content.catalog, &version, path)
        .await
    {
        Ok(png) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                // A version's textures never change.
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable",
                ),
            ],
            png,
        )
            .into_response(),
        Err(e) => failed(&e),
    }
}
