//! `nethernode.v1.AccountService`: the signed-in user's own security.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::auth::{meta, proto_user, signed_in};
use crate::rpc::auth_error;

pub(crate) struct AccountRpc {
    state: AppState,
}

impl AccountRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[allow(refining_impl_trait)]
impl pb::AccountService for AccountRpc {
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
