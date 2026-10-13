//! `nethernode.v1.ServerService`.

use connectrpc::{RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{require, store_error};

pub(crate) struct ServerRpc {
    state: AppState,
}

impl ServerRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

/// Maps a stored status string to the proto enum.
pub(crate) fn proto_status(status: &str) -> pb::ServerStatus {
    match status {
        "installing" => pb::ServerStatus::SERVER_STATUS_INSTALLING,
        "stopped" => pb::ServerStatus::SERVER_STATUS_STOPPED,
        "starting" => pb::ServerStatus::SERVER_STATUS_STARTING,
        "running" => pb::ServerStatus::SERVER_STATUS_RUNNING,
        "stopping" => pb::ServerStatus::SERVER_STATUS_STOPPING,
        "sleeping" => pb::ServerStatus::SERVER_STATUS_SLEEPING,
        "crashed" => pb::ServerStatus::SERVER_STATUS_CRASHED,
        _ => pb::ServerStatus::SERVER_STATUS_UNSPECIFIED,
    }
}

#[allow(refining_impl_trait)]
impl pb::ServerService for ServerRpc {
    async fn list_servers(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListServersRequest>,
    ) -> ServiceResult<pb::ListServersResponse> {
        require(&ctx, Permission::View)?;
        let servers = self
            .state
            .auth
            .store()
            .list_servers()
            .await
            .map_err(store_error)?;
        let servers = servers
            .into_iter()
            .map(|s| pb::Server {
                status: proto_status(&s.status).into(),
                id: s.id,
                name: s.name,
                slug: s.slug,
                mc_version: s.mc_version,
                loader: s.loader,
                internal_port: u32::try_from(s.internal_port).unwrap_or_default(),
                sleep_when_empty: s.sleep_when_empty,
                ..Default::default()
            })
            .collect();
        Response::ok(pb::ListServersResponse {
            servers,
            ..Default::default()
        })
    }
}
