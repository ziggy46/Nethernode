//! CloudflareService: the managed Cloudflare Tunnel and DNS.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use serde_json::json;

use crate::AppState;
use crate::cloudflare::{self, Mode};
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require};

pub(crate) struct CloudflareRpc {
    state: AppState,
}

impl CloudflareRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[allow(refining_impl_trait)]
impl pb::CloudflareService for CloudflareRpc {
    async fn get_cloudflare(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetCloudflareRequest>,
    ) -> ServiceResult<pb::GetCloudflareResponse> {
        require(&ctx, Permission::ManageSettings)?;
        let Some(connection) = cloudflare::connection(&self.state).await else {
            return Response::ok(pb::GetCloudflareResponse::default());
        };
        let status = cloudflare::status();
        let connector = self.state.nodes.local().tunnel_state().await.ok();
        Response::ok(pb::GetCloudflareResponse {
            connected: true,
            token_hint: connection.token_hint,
            account_name: connection.account_name,
            tunnel_id: connection.tunnel_id,
            connector_status: connector
                .as_ref()
                .map_or("absent".to_owned(), |c| c.status.clone()),
            connector_message: connector.and_then(|c| c.message).unwrap_or_default(),
            dns: connection.dns,
            zones: status.zones,
            routed: status.routed,
            conflicts: status.conflicts,
            outside: status.outside,
            changes: status.changes,
            last_error: status.last_error.unwrap_or_default(),
            last_sync_ms: status.last_sync_ms,
            ..Default::default()
        })
    }

    async fn connect_cloudflare(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ConnectCloudflareRequest>,
    ) -> ServiceResult<pb::ConnectCloudflareResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        cloudflare::connect(&self.state, request.token).await?;
        audit(
            &self.state,
            &user,
            "cloudflare.connect",
            ("settings", "cloudflare"),
            json!({}),
        )
        .await;
        Response::ok(pb::ConnectCloudflareResponse::default())
    }

    async fn preview_cloudflare(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::PreviewCloudflareRequest>,
    ) -> ServiceResult<pb::PreviewCloudflareResponse> {
        require(&ctx, Permission::ManageSettings)?;
        if cloudflare::connection(&self.state).await.is_none() {
            return Err(ConnectError::failed_precondition(
                "Connect Cloudflare first.",
            ));
        }
        let changes = cloudflare::sync(&self.state, Mode::Preview)
            .await
            .map_err(ConnectError::unavailable)?;
        Response::ok(pb::PreviewCloudflareResponse {
            changes,
            conflicts: cloudflare::status().conflicts,
            ..Default::default()
        })
    }

    async fn set_cloudflare_dns(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SetCloudflareDnsRequest>,
    ) -> ServiceResult<pb::SetCloudflareDnsResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        cloudflare::set_dns(&self.state, request.enabled).await?;
        let changes = cloudflare::status().changes;
        audit(
            &self.state,
            &user,
            "cloudflare.dns",
            ("settings", "cloudflare"),
            json!({ "enabled": request.enabled, "changes": changes }),
        )
        .await;
        Response::ok(pb::SetCloudflareDnsResponse {
            changes,
            ..Default::default()
        })
    }

    async fn disconnect_cloudflare(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::DisconnectCloudflareRequest>,
    ) -> ServiceResult<pb::DisconnectCloudflareResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        cloudflare::disconnect(&self.state).await?;
        audit(
            &self.state,
            &user,
            "cloudflare.disconnect",
            ("settings", "cloudflare"),
            json!({}),
        )
        .await;
        Response::ok(pb::DisconnectCloudflareResponse::default())
    }
}
