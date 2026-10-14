//! ConnectRPC service implementations.

use std::sync::Arc;

use connectrpc::{ConnectError, RequestContext, Router as ConnectRouter};
use nn_auth::{AuthError, AuthUser, Permission};
use nn_store::NewAuditEntry;

use crate::AppState;
use crate::middleware::CurrentUser;
use crate::proto::nethernode::v1::{
    AuthServiceExt, CatalogServiceExt, FileServiceExt, PlayerServiceExt, ProxyServiceExt,
    ServerServiceExt,
};

mod auth;
pub(crate) mod catalog;
mod files;
mod players;
mod proxy;
pub(crate) mod servers;

/// Registers every service on `router`.
pub(crate) fn register(router: ConnectRouter, state: &AppState) -> ConnectRouter {
    let router = Arc::new(auth::AuthRpc::new(state.clone())).register(router);
    let router = Arc::new(servers::ServerRpc::new(state.clone())).register(router);
    let router = Arc::new(files::FileRpc::new(state.clone())).register(router);
    let router = Arc::new(players::PlayerRpc::new(state.clone())).register(router);
    let router = Arc::new(catalog::CatalogRpc::new(state.clone())).register(router);
    Arc::new(proxy::ProxyRpc::new(state.clone()))
        .register(router)
        // The editor saves files up to 5 MB, which is larger than the default
        // message limit once encoded.
        .with_route_limits(
            "/nethernode.v1.FileService/WriteFile",
            connectrpc::Limits::default()
                .with_max_request_body_size(8 * 1024 * 1024)
                .with_max_message_size(8 * 1024 * 1024),
        )
}

/// The signed-in user, if they hold `permission`.
pub(crate) fn require(
    ctx: &RequestContext,
    permission: Permission,
) -> Result<&AuthUser, ConnectError> {
    let CurrentUser(authed) = ctx
        .extensions()
        .get::<CurrentUser>()
        .ok_or_else(|| ConnectError::unauthenticated("sign in required"))?;
    if !authed.user.role.can(permission) {
        return Err(ConnectError::permission_denied(
            "your role doesn't allow this action",
        ));
    }
    Ok(&authed.user)
}

/// Logs and hides a storage error.
pub(crate) fn store_error(err: nn_store::Error) -> ConnectError {
    tracing::error!(error = %err, "storage operation failed");
    ConnectError::internal("internal error")
}

/// Records a mutating action in the audit log. Failures are logged only.
pub(crate) async fn audit(
    state: &AppState,
    user: &AuthUser,
    action: &str,
    target: (&str, &str),
    details: serde_json::Value,
) {
    let entry = NewAuditEntry {
        user_id: Some(&user.id),
        action,
        target_type: Some(target.0),
        target_id: Some(target.1),
        details: Some(details),
    };
    if let Err(e) = state.auth.store().insert_audit(entry).await {
        tracing::warn!(error = %e, action, "failed to write audit log entry");
    }
}

/// Maps auth errors onto Connect codes, hiding internal details from clients.
pub(crate) fn auth_error(err: AuthError) -> ConnectError {
    match err {
        AuthError::InvalidCredentials => ConnectError::unauthenticated(err.to_string()),
        AuthError::RateLimited { retry_after } => {
            let mut e = ConnectError::resource_exhausted(err.to_string());
            if let Ok(v) = retry_after.as_secs().max(1).to_string().parse() {
                e.response_headers_mut()
                    .insert(http::header::RETRY_AFTER, v);
            }
            e
        }
        AuthError::SetupAlreadyComplete => ConnectError::failed_precondition(err.to_string()),
        AuthError::InvalidInput(msg) => ConnectError::invalid_argument(msg),
        AuthError::UserNotFound => ConnectError::not_found(err.to_string()),
        AuthError::Store(_) | AuthError::Internal(_) => {
            tracing::error!(error = %err, "auth operation failed");
            ConnectError::internal("internal error")
        }
    }
}
