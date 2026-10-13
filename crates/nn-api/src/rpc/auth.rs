//! `nethernode.v1.AuthService`.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use http::header::SET_COOKIE;
use nn_auth::{AuthUser, IssuedSession, Role, token};
use nn_events::Event;

use crate::middleware::CurrentUser;
use crate::proto::nethernode::v1 as pb;
use crate::request_meta::RequestMeta;
use crate::rpc::auth_error;
use crate::{AppState, cookies};

pub(crate) struct AuthRpc {
    state: AppState,
}

impl AuthRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Wraps `body` in a response that sets the session cookie and rotates
    /// the CSRF token (so a pre-login token can't be fixed by an attacker).
    fn signed_in<T>(&self, ctx: &RequestContext, body: T, issued: &IssuedSession) -> Response<T> {
        let secure = meta(ctx).secure_cookies;
        Response::new(body)
            .with_header(
                SET_COOKIE,
                cookies::session(&issued.token, self.state.auth.session_ttl(), secure),
            )
            .with_header(SET_COOKIE, cookies::csrf(&token::generate_token(), secure))
    }
}

// Handlers return concrete message types, a refinement of the trait's
// `impl Encodable` that connectrpc documents as the normal pattern.
#[allow(refining_impl_trait)]
impl pb::AuthService for AuthRpc {
    async fn setup(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetupRequest>,
    ) -> ServiceResult<pb::SetupResponse> {
        let meta = meta(&ctx);
        let issued = self
            .state
            .auth
            .setup(request.username, request.password, meta.client())
            .await
            .map_err(auth_error)?;
        tracing::info!(user = %issued.user.username, "first-run setup completed");
        self.state.events.publish(Event::UserLoggedIn {
            user_id: issued.user.id.clone(),
        });
        let body = pb::SetupResponse {
            user: proto_user(&issued.user).into(),
            ..Default::default()
        };
        Ok(self.signed_in(&ctx, body, &issued))
    }

    async fn login(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::LoginRequest>,
    ) -> ServiceResult<pb::LoginResponse> {
        let meta = meta(&ctx);
        let issued = self
            .state
            .auth
            .login(request.username, request.password, meta.client())
            .await
            .map_err(auth_error)?;
        self.state.events.publish(Event::UserLoggedIn {
            user_id: issued.user.id.clone(),
        });
        let body = pb::LoginResponse {
            user: proto_user(&issued.user).into(),
            ..Default::default()
        };
        Ok(self.signed_in(&ctx, body, &issued))
    }

    async fn logout(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::LogoutRequest>,
    ) -> ServiceResult<pb::LogoutResponse> {
        let meta = meta(&ctx);
        if let Some(token) = cookies::get(ctx.headers(), cookies::SESSION_COOKIE) {
            self.state
                .auth
                .logout(&token, meta.client())
                .await
                .map_err(auth_error)?;
        }
        if let Some(CurrentUser(authed)) = ctx.extensions().get::<CurrentUser>() {
            self.state.events.publish(Event::UserLoggedOut {
                user_id: authed.user.id.clone(),
            });
        }
        Ok(Response::new(pb::LogoutResponse::default())
            .with_header(SET_COOKIE, cookies::clear_session(meta.secure_cookies)))
    }

    async fn me(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::MeRequest>,
    ) -> ServiceResult<pb::MeResponse> {
        let CurrentUser(authed) = ctx
            .extensions()
            .get::<CurrentUser>()
            .ok_or_else(|| ConnectError::unauthenticated("sign in required"))?;
        Response::ok(pb::MeResponse {
            user: proto_user(&authed.user).into(),
            ..Default::default()
        })
    }
}

fn meta(ctx: &RequestContext) -> RequestMeta {
    ctx.extensions()
        .get::<RequestMeta>()
        .cloned()
        .unwrap_or_default()
}

fn proto_user(user: &AuthUser) -> pb::User {
    pb::User {
        id: user.id.clone(),
        username: user.username.clone(),
        role: proto_role(user.role).into(),
        created_at_ms: user.created_at,
        last_login_at_ms: user.last_login_at,
        ..Default::default()
    }
}

fn proto_role(role: Role) -> pb::Role {
    match role {
        Role::Viewer => pb::Role::ROLE_VIEWER,
        Role::Moderator => pb::Role::ROLE_MODERATOR,
        Role::Admin => pb::Role::ROLE_ADMIN,
        Role::Owner => pb::Role::ROLE_OWNER,
    }
}
