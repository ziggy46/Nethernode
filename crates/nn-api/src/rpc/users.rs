//! UserService: panel accounts, for owners.

use std::collections::HashMap;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::{AuthUser, Permission, Role};

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::auth::{meta, proto_user};
use crate::rpc::{auth_error, require, store_error};

pub(crate) struct UserRpc {
    state: AppState,
}

impl UserRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

fn role(r: Option<pb::Role>) -> Result<Role, ConnectError> {
    Ok(match r {
        Some(pb::Role::ROLE_VIEWER) => Role::Viewer,
        Some(pb::Role::ROLE_MODERATOR) => Role::Moderator,
        Some(pb::Role::ROLE_ADMIN) => Role::Admin,
        Some(pb::Role::ROLE_OWNER) => Role::Owner,
        _ => return Err(ConnectError::invalid_argument("choose a role")),
    })
}

#[allow(refining_impl_trait)]
impl pb::UserService for UserRpc {
    async fn list_users(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListUsersRequest>,
    ) -> ServiceResult<pb::ListUsersResponse> {
        require(&ctx, Permission::ManageUsers)?;
        let store = self.state.auth.store();
        let mut sessions: HashMap<String, i64> = HashMap::new();
        for s in store
            .list_sessions(None, nn_store::now_ms())
            .await
            .map_err(store_error)?
        {
            *sessions.entry(s.user_id).or_default() += 1;
        }
        let users = store
            .list_users()
            .await
            .map_err(store_error)?
            .into_iter()
            .filter_map(|u| AuthUser::try_from(u).ok())
            .map(|u| pb::UserSummary {
                active_sessions: sessions.get(&u.id).copied().unwrap_or(0),
                user: proto_user(&u).into(),
                ..Default::default()
            })
            .collect();
        Response::ok(pb::ListUsersResponse {
            users,
            ..Default::default()
        })
    }

    async fn create_user(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateUserRequest>,
    ) -> ServiceResult<pb::CreateUserResponse> {
        let actor = require(&ctx, Permission::ManageUsers)?.clone();
        let (user, password) = self
            .state
            .auth
            .create_user(
                &actor,
                request.username,
                role(request.role.as_known())?,
                meta(&ctx).client(),
            )
            .await
            .map_err(auth_error)?;
        Response::ok(pb::CreateUserResponse {
            user: proto_user(&user).into(),
            password,
            ..Default::default()
        })
    }

    async fn set_user_role(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetUserRoleRequest>,
    ) -> ServiceResult<pb::SetUserRoleResponse> {
        let actor = require(&ctx, Permission::ManageUsers)?.clone();
        let user = self
            .state
            .auth
            .set_role(
                &actor,
                request.id,
                role(request.role.as_known())?,
                meta(&ctx).client(),
            )
            .await
            .map_err(auth_error)?;
        Response::ok(pb::SetUserRoleResponse {
            user: proto_user(&user).into(),
            ..Default::default()
        })
    }

    async fn reset_user_password(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ResetUserPasswordRequest>,
    ) -> ServiceResult<pb::ResetUserPasswordResponse> {
        let actor = require(&ctx, Permission::ManageUsers)?.clone();
        let password = self
            .state
            .auth
            .reset_password(&actor, request.id, meta(&ctx).client())
            .await
            .map_err(auth_error)?;
        Response::ok(pb::ResetUserPasswordResponse {
            password,
            ..Default::default()
        })
    }

    async fn reset_user_totp(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ResetUserTotpRequest>,
    ) -> ServiceResult<pb::ResetUserTotpResponse> {
        let actor = require(&ctx, Permission::ManageUsers)?.clone();
        self.state
            .auth
            .reset_totp(&actor, request.id, meta(&ctx).client())
            .await
            .map_err(auth_error)?;
        Response::ok(pb::ResetUserTotpResponse::default())
    }

    async fn revoke_user_sessions(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RevokeUserSessionsRequest>,
    ) -> ServiceResult<pb::RevokeUserSessionsResponse> {
        let actor = require(&ctx, Permission::ManageUsers)?.clone();
        let ended = self
            .state
            .auth
            .revoke_sessions(&actor, request.id, meta(&ctx).client())
            .await
            .map_err(auth_error)?;
        Response::ok(pb::RevokeUserSessionsResponse {
            ended: i64::try_from(ended).unwrap_or(i64::MAX),
            ..Default::default()
        })
    }

    async fn delete_user(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteUserRequest>,
    ) -> ServiceResult<pb::DeleteUserResponse> {
        let actor = require(&ctx, Permission::ManageUsers)?.clone();
        self.state
            .auth
            .delete_user(&actor, request.id, meta(&ctx).client())
            .await
            .map_err(auth_error)?;
        Response::ok(pb::DeleteUserResponse::default())
    }
}
