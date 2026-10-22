//! Health, metrics, and request instrumentation.

use std::time::Instant;

use axum::Json;
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{StatusCode, header};
use nn_auth::Permission;
use serde_json::json;

use crate::AppState;
use crate::middleware::CurrentUser;

/// `GET /healthz`: liveness probe. Unauthenticated; reveals only the version.
pub(crate) async fn healthz() -> impl IntoResponse {
    Json(json!({ "status": "ok", "version": crate::VERSION }))
}

/// `GET /metrics`: Prometheus text format, owner only.
pub(crate) async fn metrics(
    State(state): State<AppState>,
    user: Option<axum::Extension<CurrentUser>>,
) -> Response {
    let Some(axum::Extension(CurrentUser(authed))) = user else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !authed.user.role.can(Permission::ViewMetrics) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match state.metrics {
        Some(handle) => (
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            handle.render(),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "metrics recorder not installed",
        )
            .into_response(),
    }
}

/// Records request counts and latency by route and status.
pub(crate) async fn record_http_metrics(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = req.method().to_string();
    // Label by matched route, not raw path, to keep cardinality bounded.
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| route_class(req.uri().path()).to_owned());
    let response = next.run(req).await;
    let status = response.status().as_u16().to_string();
    let labels = [("method", method), ("route", route), ("status", status)];
    metrics::counter!("nethernode_http_requests_total", &labels).increment(1);
    metrics::histogram!("nethernode_http_request_duration_seconds", &labels)
        .record(start.elapsed().as_secs_f64());
    response
}

fn route_class(path: &str) -> &'static str {
    if path.starts_with(crate::RPC_PREFIX) {
        "rpc"
    } else if path.starts_with("/assets/") {
        "asset"
    } else {
        "page"
    }
}
