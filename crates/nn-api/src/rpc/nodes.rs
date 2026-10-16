//! `nethernode.v1.NodeService`: the machines that run servers.

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_link::JoinToken;
use nn_link::messages::HostReport;
use nn_store::Node;
use serde_json::json;

use crate::AppState;
use crate::agents::compatible;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};

/// How long a join code works.
const JOIN_CODE_LIFETIME_MS: i64 = 30 * 60 * 1000;
const NAME_MAX: usize = 40;
/// Where agents' images come from.
const AGENT_IMAGE: &str = "ghcr.io/ziggy46/nethernode";
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) struct NodeRpc {
    state: AppState,
}

impl NodeRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    async fn to_proto(&self, node: Node, servers: u32) -> pb::Node {
        let store = self.state.auth.store();
        let mut out = pb::Node {
            local: node.is_local(),
            last_seen_ms: node.last_seen_at,
            created_at_ms: node.created_at,
            server_count: servers,
            ..Default::default()
        };
        if node.is_local() {
            out.status = pb::NodeStatus::NODE_STATUS_ONLINE.into();
            out.version = VERSION.to_owned();
            out.host = local_host(&self.state).await.into();
        } else if let Some(live) = self.state.agents.live(&node.id) {
            out.status = pb::NodeStatus::NODE_STATUS_ONLINE.into();
            out.update_needed = !compatible(VERSION, &live.version);
            out.version = live.version;
            if let Some(host) = live.host {
                out.host = proto_host(host).into();
            }
        } else if node.public_key.is_none() {
            out.status = pb::NodeStatus::NODE_STATUS_WAITING.into();
            out.join_pending = store
                .join_code_pending(&node.id, nn_store::now_ms())
                .await
                .unwrap_or(false);
        } else {
            out.status = pb::NodeStatus::NODE_STATUS_OFFLINE.into();
            out.version = node.version.clone().unwrap_or_default();
            out.update_needed = !compatible(VERSION, &out.version);
            // What it last reported.
            if let Some(host) = node
                .host_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<HostReport>(j).ok())
            {
                out.host = proto_host(host).into();
            }
        }
        out.id = node.id;
        out.name = node.name;
        out
    }

    async fn load(&self, id: &str) -> Result<Node, ConnectError> {
        self.state
            .auth
            .store()
            .find_node(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("no such node"))
    }

    /// A new join code for `node`, replacing any old one.
    async fn join_code(&self, node: &Node) -> Result<pb::JoinCode, ConnectError> {
        let panel_key = self.state.agents.public_key().ok_or_else(|| {
            ConnectError::failed_precondition(format!(
                "Agents can't connect: {}.",
                self.state
                    .agents
                    .unavailable_reason()
                    .unwrap_or("the panel has no node key")
            ))
        })?;
        let token = JoinToken::generate(panel_key);
        let expires = nn_store::now_ms() + JOIN_CODE_LIFETIME_MS;
        self.state
            .auth
            .store()
            .set_join_code(&node.id, &token.code_id, &token.secret_hash(), expires)
            .await
            .map_err(store_error)?;
        Ok(pb::JoinCode {
            token: token.encode(),
            expires_at_ms: expires,
            image: agent_image(),
            ..Default::default()
        })
    }
}

/// The agent image matching this panel: `…/nethernode:<major>.<minor>`.
fn agent_image() -> String {
    let minor: Vec<&str> = VERSION.split('.').take(2).collect();
    format!("{AGENT_IMAGE}:{}", minor.join("."))
}

fn checked_name(name: &str) -> Result<String, ConnectError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ConnectError::invalid_argument("Give the node a name."));
    }
    if name.chars().count() > NAME_MAX {
        return Err(ConnectError::invalid_argument(format!(
            "Keep the name to {NAME_MAX} characters."
        )));
    }
    Ok(name.to_owned())
}

fn taken(e: nn_store::Error) -> ConnectError {
    if e.is_unique_violation() {
        ConnectError::already_exists("Another node already has that name.")
    } else {
        store_error(e)
    }
}

fn proto_host(host: HostReport) -> pb::NodeHost {
    pb::NodeHost {
        docker_available: host.docker_available,
        docker_error: host.docker_error,
        hostname: host.hostname,
        operating_system: host.operating_system,
        docker_version: host.docker_version,
        cpus: host.cpus,
        memory_bytes: host.memory_bytes,
        disk_total_bytes: host.disk_total_bytes,
        disk_free_bytes: host.disk_free_bytes,
        ..Default::default()
    }
}

/// This machine, right now.
async fn local_host(state: &AppState) -> pb::NodeHost {
    let node = state.nodes.local();
    let disk = node.disk_space();
    let mut host = match node.host_info().await {
        Ok(info) => pb::NodeHost {
            docker_available: true,
            hostname: info.hostname,
            operating_system: info.operating_system,
            docker_version: info.docker_version,
            cpus: info.cpus,
            memory_bytes: info.memory_bytes,
            ..Default::default()
        },
        Err(nn_node::NodeError::Unavailable(reason)) => pb::NodeHost {
            docker_error: reason.to_string(),
            ..Default::default()
        },
        Err(e) => pb::NodeHost {
            docker_error: e.to_string(),
            ..Default::default()
        },
    };
    if let Some((total, free)) = disk {
        host.disk_total_bytes = Some(total);
        host.disk_free_bytes = Some(free);
    }
    host
}

#[allow(refining_impl_trait)]
impl pb::NodeService for NodeRpc {
    async fn list_nodes(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListNodesRequest>,
    ) -> ServiceResult<pb::ListNodesResponse> {
        require(&ctx, Permission::View)?;
        let store = self.state.auth.store();
        let _ = store.prune_join_codes(nn_store::now_ms()).await;
        let mut nodes = Vec::new();
        for node in store.list_nodes().await.map_err(store_error)? {
            let count = store
                .count_servers_on(&node.id)
                .await
                .map_err(store_error)?;
            nodes.push(
                self.to_proto(node, u32::try_from(count).unwrap_or(u32::MAX))
                    .await,
            );
        }
        Response::ok(pb::ListNodesResponse {
            nodes,
            unavailable_reason: self
                .state
                .agents
                .unavailable_reason()
                .unwrap_or_default()
                .to_owned(),
            ..Default::default()
        })
    }

    async fn create_node(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateNodeRequest>,
    ) -> ServiceResult<pb::CreateNodeResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let name = checked_name(request.name)?;
        if self.state.agents.public_key().is_none() {
            return Err(ConnectError::failed_precondition(format!(
                "Agents can't connect: {}.",
                self.state
                    .agents
                    .unavailable_reason()
                    .unwrap_or("the panel has no node key")
            )));
        }
        let store = self.state.auth.store();
        let node = store.create_node(&name).await.map_err(taken)?;
        let join = self.join_code(&node).await?;
        audit(
            &self.state,
            &user,
            "node.create",
            ("node", &node.id),
            json!({ "name": name }),
        )
        .await;
        let node = self.to_proto(node, 0).await;
        Response::ok(pb::CreateNodeResponse {
            node: node.into(),
            join: join.into(),
            ..Default::default()
        })
    }

    async fn regenerate_join_code(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RegenerateJoinCodeRequest>,
    ) -> ServiceResult<pb::RegenerateJoinCodeResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let node = self.load(request.id).await?;
        if node.is_local() || node.public_key.is_some() {
            return Err(ConnectError::failed_precondition(
                "This node's agent has already joined.",
            ));
        }
        let join = self.join_code(&node).await?;
        audit(
            &self.state,
            &user,
            "node.join_code",
            ("node", &node.id),
            json!({}),
        )
        .await;
        Response::ok(pb::RegenerateJoinCodeResponse {
            join: join.into(),
            ..Default::default()
        })
    }

    async fn rename_node(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RenameNodeRequest>,
    ) -> ServiceResult<pb::RenameNodeResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let name = checked_name(request.name)?;
        let store = self.state.auth.store();
        if !store.rename_node(request.id, &name).await.map_err(taken)? {
            return Err(ConnectError::not_found("no such node"));
        }
        audit(
            &self.state,
            &user,
            "node.rename",
            ("node", request.id),
            json!({ "name": name }),
        )
        .await;
        let node = self.load(request.id).await?;
        let count = store
            .count_servers_on(&node.id)
            .await
            .map_err(store_error)?;
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        let node = self.to_proto(node, count).await;
        self.state.events.publish(nn_events::Event::NodeChanged {
            node_id: node.id.clone(),
            online: node.status.as_known() == Some(pb::NodeStatus::NODE_STATUS_ONLINE),
        });
        Response::ok(pb::RenameNodeResponse {
            node: node.into(),
            ..Default::default()
        })
    }

    async fn delete_node(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteNodeRequest>,
    ) -> ServiceResult<pb::DeleteNodeResponse> {
        let user = require(&ctx, Permission::ManageSettings)?.clone();
        let node = self.load(request.id).await?;
        if node.is_local() {
            return Err(ConnectError::failed_precondition(
                "This machine can't be removed.",
            ));
        }
        let store = self.state.auth.store();
        let servers: Vec<_> = store
            .list_servers()
            .await
            .map_err(store_error)?
            .into_iter()
            .filter(|s| s.node_id == node.id)
            .collect();
        if !servers.is_empty() && !request.forget_servers {
            return Err(ConnectError::failed_precondition(format!(
                "{} still has servers. Move or delete them first, or remove it anyway and forget them.",
                node.name
            )));
        }
        if servers.iter().any(|s| self.state.nodes.is_moving(&s.id)) {
            return Err(ConnectError::failed_precondition(format!(
                "A server is moving to or from {}. Try again once that's done.",
                node.name
            )));
        }
        // Nothing should keep running there unmanaged.
        let machine = self.state.nodes.get(&node.id);
        if machine.unavailable().is_none() {
            let stops = servers
                .iter()
                .filter(|s| !matches!(s.status.as_str(), "stopped" | "crashed"))
                .map(|s| machine.stop(&s.id, false));
            let stopped = tokio::time::timeout(
                std::time::Duration::from_secs(90),
                futures_util::future::join_all(stops),
            )
            .await;
            if stopped.is_err() {
                tracing::warn!(node = %node.id, "some servers on a removed node didn't stop in time");
            }
        }
        store.delete_node(&node.id).await.map_err(store_error)?;
        self.state.agents.disconnect(&node.id);
        for server in &servers {
            store.delete_server(&server.id).await.map_err(store_error)?;
            self.state
                .events
                .publish(nn_events::Event::ServerStatusChanged {
                    server_id: server.id.clone(),
                    status: "deleted".into(),
                });
        }
        let backups = store
            .delete_backups_on_node(&node.id)
            .await
            .map_err(store_error)?;
        if !servers.is_empty()
            && let Err(e) = self.state.proxy.reload().await
        {
            tracing::warn!(error = %e, "couldn't reload the proxy after removing a node");
        }
        audit(
            &self.state,
            &user,
            "node.delete",
            ("node", &node.id),
            json!({
                "name": node.name,
                "forgot_servers": servers.iter().map(|s| &s.name).collect::<Vec<_>>(),
                "forgot_backups": backups,
            }),
        )
        .await;
        self.state.events.publish(nn_events::Event::NodeChanged {
            node_id: node.id,
            online: false,
        });
        Response::ok(pb::DeleteNodeResponse::default())
    }
}
