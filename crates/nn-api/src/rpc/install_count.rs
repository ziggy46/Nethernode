//! InstallCountService: the opt-in active installs count, for owners.

use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use serde_json::json;

use crate::AppState;
use crate::install_count::payload;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require};

pub(crate) struct InstallCountRpc {
    state: AppState,
}

impl InstallCountRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[allow(refining_impl_trait)]
impl pb::InstallCountService for InstallCountRpc {
    async fn get_install_count(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetInstallCountRequest>,
    ) -> ServiceResult<pb::GetInstallCountResponse> {
        // Owner-only, like users: it's a decision about the whole install.
        require(&ctx, Permission::ManageUsers)?;
        let status = self
            .state
            .install_count
            .status(self.state.auth.store())
            .await;
        Response::ok(pb::GetInstallCountResponse {
            enabled: status.enabled,
            forced: status.forced,
            payload: status
                .id
                .map(|id| payload(&id).to_string())
                .unwrap_or_default(),
            url: status.url,
            last_sent_ms: status.last_sent_ms,
            last_error: status.last_error.unwrap_or_default(),
            ..Default::default()
        })
    }

    async fn set_install_count(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetInstallCountRequest>,
    ) -> ServiceResult<pb::SetInstallCountResponse> {
        let user = require(&ctx, Permission::ManageUsers)?.clone();
        self.state
            .install_count
            .set(self.state.auth.store(), request.enabled)
            .await?;
        audit(
            &self.state,
            &user,
            "settings.install_count",
            ("settings", "install_count"),
            json!({ "enabled": request.enabled }),
        )
        .await;
        Response::ok(pb::SetInstallCountResponse::default())
    }
}
