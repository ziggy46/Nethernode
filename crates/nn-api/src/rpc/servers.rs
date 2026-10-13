//! `nethernode.v1.ServerService`.

use std::collections::HashMap;
use std::sync::Arc;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_events::Event;
use nn_orchestrator::{CreateServer, Orchestrator, OrchestratorError};
use nn_store::{Server, ServerSettings};
use serde_json::json;

use crate::AppState;
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};

pub(crate) struct ServerRpc {
    state: AppState,
}

impl ServerRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The orchestrator, or a clear error when Docker isn't reachable.
    fn orchestrator(&self) -> Result<Arc<Orchestrator>, ConnectError> {
        self.state.servers.orchestrator.clone().ok_or_else(|| {
            let reason = self.state.servers.unavailable_reason.as_deref().unwrap_or("not connected");
            ConnectError::failed_precondition(format!(
                "Docker isn't available ({reason}). Server management is disabled until NetherNode can reach Docker."
            ))
        })
    }

    async fn load(&self, id: &str) -> Result<Server, ConnectError> {
        self.state
            .auth
            .store()
            .find_server(id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("server not found"))
    }

    /// Hostnames that reach each server: exact routes, then the server's
    /// subdomain under each slug wildcard (`*.example.com` with no target).
    async fn hostnames(&self) -> Result<Hostnames, ConnectError> {
        let routes = self
            .state
            .auth
            .store()
            .list_routes()
            .await
            .map_err(store_error)?;
        let mut out = Hostnames::default();
        for route in routes {
            match (route.server_id, route.hostname.strip_prefix("*.")) {
                (None, Some(domain)) => out.slug_domains.push(domain.to_owned()),
                (Some(id), None) if route.hostname != "*" => {
                    out.exact.entry(id).or_default().push(route.hostname);
                }
                _ => {}
            }
        }
        Ok(out)
    }

    async fn to_proto(&self, server: Server, hostnames: &Hostnames) -> pb::Server {
        let orch = self.state.servers.orchestrator.as_ref();
        let players = orch.and_then(|o| o.players(&server.id));
        let restart_required = match orch {
            Some(o) => o.restart_required(&server).await,
            None => false,
        };
        pb::Server {
            status: proto_status(&server.status).into(),
            settings: proto_settings(&server.settings()).into(),
            hostnames: hostnames.for_server(&server),
            players_online: players.map(|p| p.online),
            players_max: players.map(|p| p.max),
            restart_required,
            internal_port: u32::try_from(server.internal_port).unwrap_or_default(),
            sleep_when_empty: server.sleep_when_empty,
            created_at_ms: server.created_at,
            last_started_at_ms: server.last_started_at,
            id: server.id,
            name: server.name,
            slug: server.slug,
            mc_version: server.mc_version,
            loader: server.loader,
            ..Default::default()
        }
    }

    /// Runs a lifecycle operation in the background, reporting failures as
    /// `server_operation_failed` events.
    fn spawn_operation<F, Fut>(&self, id: String, operation: &'static str, run: F)
    where
        F: FnOnce(Arc<Orchestrator>, String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), OrchestratorError>> + Send + 'static,
    {
        let Some(orch) = self.state.servers.orchestrator.clone() else {
            return;
        };
        let events = self.state.events.clone();
        tokio::spawn(async move {
            if let Err(error) = run(orch, id.clone()).await {
                tracing::warn!(server = %id, operation, %error, "server operation failed");
                events.publish(Event::ServerOperationFailed {
                    server_id: id,
                    operation: operation.to_owned(),
                    message: error.to_string(),
                });
            }
        });
    }
}

/// Maps orchestrator errors to Connect codes with user-facing messages.
/// Hostnames that reach each server, from the route table.
#[derive(Default)]
struct Hostnames {
    exact: HashMap<String, Vec<String>>,
    slug_domains: Vec<String>,
}

impl Hostnames {
    fn for_server(&self, server: &Server) -> Vec<String> {
        let mut names = self.exact.get(&server.id).cloned().unwrap_or_default();
        names.extend(
            self.slug_domains
                .iter()
                .map(|domain| format!("{}.{domain}", server.slug)),
        );
        names
    }
}

pub(crate) fn orchestrator_error(err: OrchestratorError) -> ConnectError {
    match err {
        OrchestratorError::NotFound => ConnectError::not_found("server not found"),
        OrchestratorError::InvalidState(m) => ConnectError::failed_precondition(m),
        OrchestratorError::InvalidInput(m) => ConnectError::invalid_argument(m),
        OrchestratorError::NoPorts(_) => ConnectError::resource_exhausted(err.to_string()),
        OrchestratorError::Docker(_) => {
            tracing::warn!(error = %err, "Docker error");
            ConnectError::unavailable(err.to_string())
        }
        OrchestratorError::Store(_) | OrchestratorError::Secret(_) | OrchestratorError::Io(_) => {
            tracing::error!(error = %err, "server operation failed");
            ConnectError::internal("internal error")
        }
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

fn proto_theme(theme: &str) -> pb::CardTheme {
    match theme {
        "overworld" => pb::CardTheme::CARD_THEME_OVERWORLD,
        "end" => pb::CardTheme::CARD_THEME_END,
        "sky" => pb::CardTheme::CARD_THEME_SKY,
        _ => pb::CardTheme::CARD_THEME_NETHER,
    }
}

fn theme_name(theme: Option<pb::CardTheme>) -> &'static str {
    match theme {
        Some(pb::CardTheme::CARD_THEME_OVERWORLD) => "overworld",
        Some(pb::CardTheme::CARD_THEME_END) => "end",
        Some(pb::CardTheme::CARD_THEME_SKY) => "sky",
        _ => "nether",
    }
}

fn proto_settings(s: &ServerSettings) -> pb::ServerSettings {
    pb::ServerSettings {
        name: s.name.clone(),
        mc_version: s.mc_version.clone(),
        loader: s.loader.clone(),
        loader_version: s.loader_version.clone().unwrap_or_default(),
        image: s.image.clone(),
        java_version: s.java_version.and_then(|j| u32::try_from(j).ok()),
        memory_mb: u32::try_from(s.memory_mb).unwrap_or_default(),
        cpu_limit: s.cpu_limit,
        auto_start: s.auto_start,
        sleep_when_empty: s.sleep_when_empty,
        idle_minutes: s.auto_stop_idle_minutes.and_then(|m| u32::try_from(m).ok()),
        restart_on_crash: s.restart_on_crash,
        use_aikar_flags: s.use_aikar_flags,
        motd: s.motd.clone().unwrap_or_default(),
        max_players: u32::try_from(s.max_players).unwrap_or_default(),
        card_theme: proto_theme(&s.card_theme).into(),
        ..Default::default()
    }
}

fn settings_from_proto(s: &pb::ServerSettingsView<'_>) -> ServerSettings {
    let text = |v: &str| Some(v.trim().to_owned()).filter(|v| !v.is_empty());
    ServerSettings {
        name: s.name.trim().to_owned(),
        mc_version: s.mc_version.trim().to_owned(),
        loader: s.loader.trim().to_ascii_lowercase(),
        loader_version: text(s.loader_version),
        image: s.image.trim().to_owned(),
        java_version: s.java_version.map(i64::from),
        memory_mb: i64::from(s.memory_mb),
        cpu_limit: s.cpu_limit,
        auto_start: s.auto_start,
        sleep_when_empty: s.sleep_when_empty,
        auto_stop_idle_minutes: s.idle_minutes.map(i64::from),
        restart_on_crash: s.restart_on_crash,
        use_aikar_flags: s.use_aikar_flags,
        motd: text(s.motd),
        max_players: i64::from(s.max_players),
        card_theme: theme_name(s.card_theme.as_known()).to_owned(),
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
        let hostnames = self.hostnames().await?;
        let mut out = Vec::with_capacity(servers.len());
        for server in servers {
            out.push(self.to_proto(server, &hostnames).await);
        }
        Response::ok(pb::ListServersResponse {
            servers: out,
            ..Default::default()
        })
    }

    async fn get_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetServerRequest>,
    ) -> ServiceResult<pb::GetServerResponse> {
        require(&ctx, Permission::View)?;
        let server = self.load(request.id).await?;
        let server = self.to_proto(server, &self.hostnames().await?).await;
        Response::ok(pb::GetServerResponse {
            server: server.into(),
            ..Default::default()
        })
    }

    async fn create_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateServerRequest>,
    ) -> ServiceResult<pb::CreateServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let orch = self.orchestrator()?;
        let input = CreateServer {
            name: request.name.to_owned(),
            loader: request.loader.to_owned(),
            mc_version: request.mc_version.to_owned(),
            loader_version: Some(request.loader_version.to_owned()),
            memory_mb: i64::from(request.memory_mb),
            card_theme: theme_name(request.card_theme.as_known()).to_owned(),
            eula_accepted: request.eula_accepted,
            sleep_when_empty: request.sleep_when_empty,
            start_after_create: request.start_after_create,
        };
        let server = orch.create(input).await.map_err(orchestrator_error)?;
        let details = json!({ "name": server.name, "loader": server.loader, "version": server.mc_version, "eula_accepted": true });
        audit(
            &self.state,
            &user,
            "server.create",
            ("server", &server.id),
            details,
        )
        .await;
        let server = self.to_proto(server, &self.hostnames().await?).await;
        Response::ok(pb::CreateServerResponse {
            server: server.into(),
            ..Default::default()
        })
    }

    async fn update_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateServerRequest>,
    ) -> ServiceResult<pb::UpdateServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let orch = self.orchestrator()?;
        let settings = request
            .settings
            .as_option()
            .map(settings_from_proto)
            .ok_or_else(|| ConnectError::invalid_argument("settings are required"))?;
        let (server, _) = orch
            .update(request.id, settings)
            .await
            .map_err(orchestrator_error)?;
        audit(
            &self.state,
            &user,
            "server.update",
            ("server", &server.id),
            json!({}),
        )
        .await;
        let server = self.to_proto(server, &self.hostnames().await?).await;
        Response::ok(pb::UpdateServerResponse {
            server: server.into(),
            ..Default::default()
        })
    }

    async fn delete_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteServerRequest>,
    ) -> ServiceResult<pb::DeleteServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let orch = self.orchestrator()?;
        let server = self.load(request.id).await?;
        orch.delete(&server.id, request.delete_data)
            .await
            .map_err(orchestrator_error)?;
        self.state.proxy.reload().await.map_err(store_error)?;
        let details = json!({ "name": server.name, "deleted_data": request.delete_data });
        audit(
            &self.state,
            &user,
            "server.delete",
            ("server", &server.id),
            details,
        )
        .await;
        Response::ok(pb::DeleteServerResponse::default())
    }

    async fn start_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::StartServerRequest>,
    ) -> ServiceResult<pb::StartServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        self.orchestrator()?;
        let server = self.load(request.id).await?;
        match server.status.as_str() {
            "installing" => {
                return Err(ConnectError::failed_precondition(
                    "The server is still installing.",
                ));
            }
            "stopping" => {
                return Err(ConnectError::failed_precondition(
                    "The server is stopping. Try again in a moment.",
                ));
            }
            _ => {}
        }
        if server.eula_accepted_at.is_none() {
            return Err(ConnectError::failed_precondition(
                "Accept the Minecraft EULA before starting this server.",
            ));
        }
        self.spawn_operation(server.id.clone(), "start", |o, id| async move {
            o.start(&id).await
        });
        audit(
            &self.state,
            &user,
            "server.start",
            ("server", &server.id),
            json!({}),
        )
        .await;
        Response::ok(pb::StartServerResponse::default())
    }

    async fn stop_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::StopServerRequest>,
    ) -> ServiceResult<pb::StopServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        self.orchestrator()?;
        let server = self.load(request.id).await?;
        self.spawn_operation(server.id.clone(), "stop", |o, id| async move {
            o.stop(&id, false).await
        });
        audit(
            &self.state,
            &user,
            "server.stop",
            ("server", &server.id),
            json!({}),
        )
        .await;
        Response::ok(pb::StopServerResponse::default())
    }

    async fn restart_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::RestartServerRequest>,
    ) -> ServiceResult<pb::RestartServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        self.orchestrator()?;
        let server = self.load(request.id).await?;
        self.spawn_operation(server.id.clone(), "restart", |o, id| async move {
            o.restart(&id).await
        });
        audit(
            &self.state,
            &user,
            "server.restart",
            ("server", &server.id),
            json!({}),
        )
        .await;
        Response::ok(pb::RestartServerResponse::default())
    }

    async fn kill_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::KillServerRequest>,
    ) -> ServiceResult<pb::KillServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        self.orchestrator()?;
        let server = self.load(request.id).await?;
        self.spawn_operation(server.id.clone(), "kill", |o, id| async move {
            o.kill(&id).await
        });
        audit(
            &self.state,
            &user,
            "server.kill",
            ("server", &server.id),
            json!({}),
        )
        .await;
        Response::ok(pb::KillServerResponse::default())
    }

    async fn get_server_stats(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetServerStatsRequest>,
    ) -> ServiceResult<pb::GetServerStatsResponse> {
        require(&ctx, Permission::View)?;
        let samples = self
            .state
            .servers
            .orchestrator
            .as_ref()
            .map(|o| o.stats_history(request.id))
            .unwrap_or_default()
            .into_iter()
            .map(|s| pb::StatSample {
                at_ms: s.at_ms,
                cpu_percent: s.cpu_percent,
                memory_bytes: s.memory_bytes,
                memory_limit_bytes: s.memory_limit_bytes,
                net_rx_bytes: s.net_rx_bytes,
                net_tx_bytes: s.net_tx_bytes,
                ..Default::default()
            })
            .collect();
        Response::ok(pb::GetServerStatsResponse {
            samples,
            ..Default::default()
        })
    }

    async fn get_host_info(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetHostInfoRequest>,
    ) -> ServiceResult<pb::GetHostInfoResponse> {
        require(&ctx, Permission::View)?;
        let response = match &self.state.servers.orchestrator {
            None => pb::GetHostInfoResponse {
                docker_available: false,
                error: self
                    .state
                    .servers
                    .unavailable_reason
                    .as_deref()
                    .unwrap_or("not connected")
                    .to_owned(),
                ..Default::default()
            },
            Some(orch) => match orch.host_info().await {
                Ok(info) => pb::GetHostInfoResponse {
                    docker_available: true,
                    hostname: info.hostname,
                    operating_system: info.operating_system,
                    docker_version: info.docker_version,
                    cpus: info.cpus,
                    memory_bytes: info.memory_bytes,
                    ..Default::default()
                },
                Err(e) => pb::GetHostInfoResponse {
                    docker_available: false,
                    error: e.to_string(),
                    ..Default::default()
                },
            },
        };
        Response::ok(response)
    }
}
