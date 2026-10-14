//! CSRF, session, and RPC access middleware.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use connectrpc::ConnectError;
use http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use nn_auth::{Authenticated, token};

use crate::request_meta::RequestMeta;
use crate::{AppState, RPC_PREFIX, cookies};

/// RPC procedures callable without a session. Everything else is denied.
pub const PUBLIC_PROCEDURES: &[&str] = &[
    "/nethernode.v1.AuthService/Setup",
    "/nethernode.v1.AuthService/Login",
    "/nethernode.v1.AuthService/FinishSignIn",
];

/// What a user whose password must be changed can still call.
pub const PASSWORD_CHANGE_PROCEDURES: &[&str] = &[
    "/nethernode.v1.AuthService/Me",
    "/nethernode.v1.AuthService/Logout",
    "/nethernode.v1.AccountService/ChangePassword",
];

/// Marks a request from a user who must change their password first; they
/// get no [`CurrentUser`] for anything else.
#[derive(Debug, Clone, Copy)]
pub struct PasswordChangeRequired;

/// Response header set on 401s while first-run setup is pending, so the UI
/// can route to `/setup` instead of `/login`.
pub const SETUP_REQUIRED_HEADER: &str = "nn-setup-required";

/// The signed-in user, inserted into request extensions by [`resolve_session`].
#[derive(Debug, Clone)]
pub struct CurrentUser(pub Authenticated);

/// Double-submit CSRF protection.
///
/// Every response to a client without a CSRF cookie gets one. Requests with
/// side effects (anything but GET/HEAD/OPTIONS) must echo the cookie's value
/// in the `x-csrf-token` header, or they get 403.
pub(crate) async fn csrf(req: Request, next: Next) -> Response {
    let secure = req
        .extensions()
        .get::<RequestMeta>()
        .is_some_and(|m| m.secure_cookies);
    let cookie = cookies::get(req.headers(), cookies::CSRF_COOKIE);

    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let mut response = if safe || csrf_matches(req.headers(), cookie.as_deref()) {
        next.run(req).await
    } else {
        tracing::debug!(path = %req.uri().path(), "rejected request without valid CSRF token");
        let message = "missing or invalid CSRF token";
        if req.uri().path().starts_with(RPC_PREFIX) {
            ConnectError::permission_denied(message).into_response()
        } else {
            (StatusCode::FORBIDDEN, message).into_response()
        }
    };

    if cookie.is_none() && !cookies::response_sets(response.headers(), cookies::CSRF_COOKIE) {
        response.headers_mut().append(
            header::SET_COOKIE,
            cookies::csrf(&token::generate_token(), secure),
        );
    }
    response
}

fn csrf_matches(headers: &HeaderMap, cookie: Option<&str>) -> bool {
    let header = headers
        .get(cookies::CSRF_HEADER)
        .and_then(|v| v.to_str().ok());
    match (header, cookie) {
        (Some(h), Some(c)) if !c.is_empty() => token::constant_time_eq(h, c),
        _ => false,
    }
}

/// Resolves the session cookie to a [`CurrentUser`], and reissues the
/// cookie when the session's expiry slides forward.
pub(crate) async fn resolve_session(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let mut renewed = None;
    if let Some(token) = cookies::get(req.headers(), cookies::SESSION_COOKIE) {
        match state.auth.authenticate(&token).await {
            Ok(Some(authed)) => {
                if authed.renewed {
                    renewed = Some(token);
                }
                let procedure = req.uri().path().strip_prefix(RPC_PREFIX);
                if authed.user.must_change_password
                    && !procedure.is_some_and(|p| PASSWORD_CHANGE_PROCEDURES.contains(&p))
                {
                    req.extensions_mut().insert(PasswordChangeRequired);
                } else {
                    req.extensions_mut().insert(CurrentUser(authed));
                }
            }
            Ok(None) => {}
            Err(e) => tracing::error!(error = %e, "session lookup failed"),
        }
    }
    let secure = req
        .extensions()
        .get::<RequestMeta>()
        .is_some_and(|m| m.secure_cookies);

    let mut response = next.run(req).await;
    if let Some(token) = renewed
        && !cookies::response_sets(response.headers(), cookies::SESSION_COOKIE)
    {
        let cookie = cookies::session(&token, state.auth.session_ttl(), secure);
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

/// Marks API responses uncacheable: they carry per-user data, and RPC reads
/// use GET.
pub(crate) async fn no_store(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Default-deny for RPCs: anything outside [`PUBLIC_PROCEDURES`] needs a session.
pub(crate) async fn require_session_for_rpc(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let public = PUBLIC_PROCEDURES.contains(&req.uri().path());
    if public || req.extensions().get::<CurrentUser>().is_some() {
        return next.run(req).await;
    }
    if req.extensions().get::<PasswordChangeRequired>().is_some() {
        return ConnectError::permission_denied("Choose a new password to continue.")
            .into_response();
    }
    unauthenticated(&state).await
}

/// A Connect `unauthenticated` (HTTP 401) error, flagged when setup is pending.
pub(crate) async fn unauthenticated(state: &AppState) -> Response {
    let mut response = ConnectError::unauthenticated("sign in required").into_response();
    if state.auth.needs_setup().await.unwrap_or(false) {
        response
            .headers_mut()
            .insert(SETUP_REQUIRED_HEADER, HeaderValue::from_static("1"));
    }
    response
}

/// Tells browsers to use HTTPS from now on, on responses to requests that
/// arrived over HTTPS (directly or through a proxy that says so). Plain-HTTP
/// LAN use never gets it, so it can't lock anyone out.
pub(crate) async fn strict_transport(req: Request, next: Next) -> Response {
    let https = req
        .extensions()
        .get::<RequestMeta>()
        .is_some_and(|m| m.secure_cookies);
    let mut response = next.run(req).await;
    if https {
        response.headers_mut().insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    response
}
