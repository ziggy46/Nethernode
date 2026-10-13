//! HTTP API for NetherNode.
//!
//! One axum router serves everything on the panel port:
//!
//! - `/api/rpc/*`: ConnectRPC services (see `proto/`)
//! - `/api/events`: the event bus as Server-Sent Events (signed-in users)
//! - `/healthz`: liveness probe
//! - `/metrics`: Prometheus metrics (owner only)
//! - everything else: the embedded web UI, with SPA fallback
//!
//! Requests pass through, outermost first: tracing, compression, security
//! headers, request metadata (client IP, scheme), CSRF double-submit checks,
//! and session resolution. RPCs are default-deny: only the procedures in
//! [`PUBLIC_PROCEDURES`] can be called without a session.

use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use connectrpc::Router as ConnectRouter;
use http::HeaderValue;
use http::header;
use metrics_exporter_prometheus::PrometheusHandle;
use nn_auth::Auth;
use nn_events::EventBus;
use serde::Deserialize;
use tower_http::compression::CompressionLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

mod cookies;
mod events;
mod middleware;
mod observability;
pub mod proxy_runtime;
mod request_meta;
mod rpc;
mod spa;

pub use middleware::{CurrentUser, PUBLIC_PROCEDURES, SETUP_REQUIRED_HEADER};
pub use proxy_runtime::{ProxyRuntime, StoreProxyHooks};
pub use request_meta::RequestMeta;

/// Generated protobuf messages and ConnectRPC service traits.
#[allow(missing_docs, clippy::all)]
pub mod proto {
    connectrpc::include_generated!();
}

/// Path prefix for ConnectRPC services.
pub const RPC_PREFIX: &str = "/api/rpc";

/// When to mark cookies `Secure`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecureCookies {
    /// Secure when the request arrived over HTTPS, including via a reverse
    /// proxy or Cloudflare Tunnel that sets `X-Forwarded-Proto: https`.
    /// Plain-HTTP access (e.g. on a LAN) gets non-Secure cookies so login
    /// still works.
    #[default]
    Auto,
    /// Always Secure. Login over plain HTTP won't work except on `localhost`
    /// in browsers that treat it as a secure context.
    Always,
    /// Never Secure. Not recommended.
    Never,
}

/// HTTP-layer settings.
#[derive(Debug, Clone, Default)]
pub struct ApiConfig {
    /// When to set the `Secure` cookie attribute.
    pub secure_cookies: SecureCookies,
    /// Trust `CF-Connecting-IP`, `X-Real-IP`, and `X-Forwarded-For` for the
    /// client IP. Enable only behind a reverse proxy or Cloudflare Tunnel;
    /// otherwise clients can spoof their IP and dodge rate limiting.
    pub trust_proxy_headers: bool,
}

/// Shared state for handlers and middleware.
#[derive(Clone)]
pub struct AppState {
    /// Authentication service.
    pub auth: Arc<Auth>,
    /// Event bus.
    pub events: EventBus,
    /// HTTP-layer settings.
    pub config: Arc<ApiConfig>,
    /// Prometheus renderer, if a recorder is installed.
    pub metrics: Option<PrometheusHandle>,
    /// The Smart Proxy and its database-backed configuration.
    pub proxy: Arc<ProxyRuntime>,
}

/// Builds the full application router.
pub fn router(state: AppState) -> Router {
    let connect = rpc::register(ConnectRouter::new(), &state);
    let rpc_routes = Router::new()
        .fallback_service(connect.into_axum_service())
        .layer(from_fn_with_state(
            state.clone(),
            middleware::require_session_for_rpc,
        ));

    Router::new()
        .route("/healthz", get(observability::healthz))
        .route("/metrics", get(observability::metrics))
        .route("/api/events", get(events::stream))
        .nest(RPC_PREFIX, rpc_routes)
        .fallback(spa::serve)
        .layer(from_fn_with_state(
            state.clone(),
            middleware::resolve_session,
        ))
        .layer(from_fn_with_state(state.clone(), middleware::csrf))
        .layer(from_fn_with_state(state.clone(), request_meta::attach))
        .layer(axum::middleware::from_fn(
            observability::record_http_metrics,
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("same-origin"),
        ))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
