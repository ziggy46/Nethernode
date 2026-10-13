//! ConnectRPC service implementations.

use std::sync::Arc;

use connectrpc::{ConnectError, Router as ConnectRouter};
use nn_auth::AuthError;

use crate::AppState;
use crate::proto::nethernode::v1::AuthServiceExt;

mod auth;

/// Registers every service on `router`.
pub(crate) fn register(router: ConnectRouter, state: &AppState) -> ConnectRouter {
    Arc::new(auth::AuthRpc::new(state.clone())).register(router)
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
