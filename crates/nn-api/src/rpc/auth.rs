//! `nethernode.v1.AuthService`.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use http::header::SET_COOKIE;
use nn_auth::{AuthUser, IssuedSession, Role, SignIn, token};
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
        let issued = match self
            .state
            .auth
            .login(request.username, request.password, meta.client())
            .await
            .map_err(auth_error)?
        {
            SignIn::Session(issued) => issued,
            SignIn::SecondFactor { challenge } => {
                return Response::ok(pb::LoginResponse {
                    second_factor_challenge: challenge,
                    ..Default::default()
                });
            }
        };
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

    async fn finish_sign_in(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::FinishSignInRequest>,
    ) -> ServiceResult<pb::FinishSignInResponse> {
        let meta = meta(&ctx);
        let issued = self
            .state
            .auth
            .finish_sign_in(request.challenge, request.code, meta.client())
            .await
            .map_err(auth_error)?;
        self.state.events.publish(Event::UserLoggedIn {
            user_id: issued.user.id.clone(),
        });
        let body = pb::FinishSignInResponse {
            user: proto_user(&issued.user).into(),
            ..Default::default()
        };
        Ok(self.signed_in(&ctx, body, &issued))
    }

    async fn change_password(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ChangePasswordRequest>,
    ) -> ServiceResult<pb::ChangePasswordResponse> {
        let authed = signed_in(&ctx)?;
        self.state
            .auth
            .change_password(
                &authed.user.id,
                &authed.session_id,
                request.current_password,
                request.new_password,
                meta(&ctx).client(),
            )
            .await
            .map_err(auth_error)?;
        let mut user = authed.user.clone();
        user.must_change_password = false;
        Response::ok(pb::ChangePasswordResponse {
            user: proto_user(&user).into(),
            ..Default::default()
        })
    }

    async fn get_security(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetSecurityRequest>,
    ) -> ServiceResult<pb::GetSecurityResponse> {
        let authed = signed_in(&ctx)?;
        let store = self.state.auth.store();
        let sessions = store
            .list_sessions(Some(&authed.user.id), nn_store::now_ms())
            .await
            .map_err(crate::rpc::store_error)?;
        let left = self
            .state
            .auth
            .recovery_codes_left(&authed.user.id)
            .await
            .map_err(auth_error)?;
        Response::ok(pb::GetSecurityResponse {
            sessions: sessions
                .into_iter()
                .map(|s| pb::Session {
                    current: s.id == authed.session_id,
                    id: s.id,
                    user_agent: s.user_agent.unwrap_or_default(),
                    ip: s.ip.unwrap_or_default(),
                    created_at_ms: s.created_at,
                    expires_at_ms: s.expires_at,
                    ..Default::default()
                })
                .collect(),
            totp_enabled: authed.user.totp_enabled,
            recovery_codes_left: left,
            totp_available: self.state.auth.two_factor_available(),
            ..Default::default()
        })
    }

    async fn revoke_session(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RevokeSessionRequest>,
    ) -> ServiceResult<pb::RevokeSessionResponse> {
        let authed = signed_in(&ctx)?;
        if request.id == authed.session_id {
            return Err(ConnectError::invalid_argument(
                "sign out to end this session",
            ));
        }
        let ended = self
            .state
            .auth
            .store()
            .delete_session(request.id, &authed.user.id)
            .await
            .map_err(crate::rpc::store_error)?;
        if !ended {
            return Err(ConnectError::not_found("no such session"));
        }
        Response::ok(pb::RevokeSessionResponse::default())
    }

    async fn begin_totp_setup(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::BeginTotpSetupRequest>,
    ) -> ServiceResult<pb::BeginTotpSetupResponse> {
        let authed = signed_in(&ctx)?;
        if authed.user.totp_enabled {
            return Err(ConnectError::failed_precondition(
                "two-factor sign-in is already on",
            ));
        }
        let enrollment = self
            .state
            .auth
            .begin_totp(&authed.user)
            .await
            .map_err(auth_error)?;
        Response::ok(pb::BeginTotpSetupResponse {
            secret: enrollment.secret,
            otpauth_uri: enrollment.uri,
            qr_svg: enrollment.qr_svg,
            ..Default::default()
        })
    }

    async fn confirm_totp_setup(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ConfirmTotpSetupRequest>,
    ) -> ServiceResult<pb::ConfirmTotpSetupResponse> {
        let authed = signed_in(&ctx)?;
        let codes = self
            .state
            .auth
            .confirm_totp(&authed.user.id, request.code, meta(&ctx).client())
            .await
            .map_err(auth_error)?;
        Response::ok(pb::ConfirmTotpSetupResponse {
            recovery_codes: codes,
            ..Default::default()
        })
    }

    async fn disable_totp(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DisableTotpRequest>,
    ) -> ServiceResult<pb::DisableTotpResponse> {
        let authed = signed_in(&ctx)?;
        self.state
            .auth
            .disable_totp(&authed.user.id, request.password, meta(&ctx).client())
            .await
            .map_err(auth_error)?;
        Response::ok(pb::DisableTotpResponse::default())
    }
}

fn signed_in(ctx: &RequestContext) -> Result<&nn_auth::Authenticated, ConnectError> {
    ctx.extensions()
        .get::<CurrentUser>()
        .map(|CurrentUser(authed)| authed)
        .ok_or_else(|| ConnectError::unauthenticated("sign in required"))
}

pub(crate) fn meta(ctx: &RequestContext) -> RequestMeta {
    ctx.extensions()
        .get::<RequestMeta>()
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn proto_user(user: &AuthUser) -> pb::User {
    pb::User {
        id: user.id.clone(),
        username: user.username.clone(),
        role: proto_role(user.role).into(),
        created_at_ms: user.created_at,
        last_login_at_ms: user.last_login_at,
        must_change_password: user.must_change_password,
        totp_enabled: user.totp_enabled,
        ..Default::default()
    }
}

pub(crate) fn proto_role(role: Role) -> pb::Role {
    match role {
        Role::Viewer => pb::Role::ROLE_VIEWER,
        Role::Moderator => pb::Role::ROLE_MODERATOR,
        Role::Admin => pb::Role::ROLE_ADMIN,
        Role::Owner => pb::Role::ROLE_OWNER,
    }
}
