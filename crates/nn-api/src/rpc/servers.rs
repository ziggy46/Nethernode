//! `nethernode.v1.ServerService`.

use std::collections::HashMap;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use nn_auth::Permission;
use nn_events::Event;
use nn_files::{Expect, FileError, RelPath};
use nn_node::{Node, NodeError};
use nn_orchestrator::{CreateServer, OrchestratorError};
use nn_store::{Server, ServerSettings};
use serde_json::json;

use crate::AppState;
use crate::countdown;
use crate::files::{file_error, open};
use crate::proto::nethernode::v1 as pb;
use crate::rpc::{audit, require, store_error};

/// Serializes server creation across machines.
pub(crate) static CREATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) struct ServerRpc {
    state: AppState,
}

impl ServerRpc {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The node `server` runs on, or a clear error when it can't manage
    /// servers right now.
    fn node(&self, server: &Server) -> Result<Node, ConnectError> {
        crate::moves::not_moving(&self.state, &server.id)?;
        let node = self.state.nodes.for_server(server);
        match node.unavailable() {
            Some(e) => Err(node_error(e)),
            None => Ok(node.clone()),
        }
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
        out.node_names = self
            .state
            .auth
            .store()
            .list_nodes()
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|n| (n.id, n.name))
            .collect();
        Ok(out)
    }

    /// The server's stored stats for one of the longer ranges, and where
    /// the range starts.
    async fn stat_periods(
        &self,
        id: &str,
        range: pb::StatsRange,
    ) -> Result<(Vec<pb::StatPeriod>, i64), ConnectError> {
        const MINUTE: i64 = 60_000;
        const HOUR: i64 = 60 * MINUTE;
        let (period, span, bucket) = match range {
            pb::StatsRange::STATS_RANGE_WEEK => (3600, 7 * 24 * HOUR, HOUR),
            pb::StatsRange::STATS_RANGE_MONTH => (3600, 30 * 24 * HOUR, 4 * HOUR),
            _ => (60, 24 * HOUR, 5 * MINUTE),
        };
        let since = nn_store::now_ms() - span;
        let since = since - since.rem_euclid(bucket);
        let rows = self
            .state
            .auth
            .store()
            .stats_since(id, period, since, bucket)
            .await
            .map_err(store_error)?;
        let seconds = u32::try_from(bucket / 1000).unwrap_or(u32::MAX);
        let periods = rows
            .into_iter()
            .map(|r| {
                let n = r.samples.max(1) as f64;
                let ticks = r.tick_samples.max(1) as f64;
                let has_ticks = r.tick_samples > 0;
                pb::StatPeriod {
                    at_ms: r.at_ms,
                    seconds,
                    running_seconds: u32::try_from(r.samples).unwrap_or(u32::MAX),
                    cpu_avg: r.cpu_sum / n,
                    cpu_max: r.cpu_max,
                    memory_avg: (r.memory_sum as f64 / n) as u64,
                    memory_max: u64::try_from(r.memory_max).unwrap_or(0),
                    memory_limit: u64::try_from(r.memory_limit).unwrap_or(0),
                    net_rx: u64::try_from(r.net_rx).unwrap_or(0),
                    net_tx: u64::try_from(r.net_tx).unwrap_or(0),
                    disk_read: u64::try_from(r.disk_read).unwrap_or(0),
                    disk_write: u64::try_from(r.disk_write).unwrap_or(0),
                    players_max: r.players_max.and_then(|p| u32::try_from(p).ok()),
                    mspt_avg: has_ticks.then(|| r.mspt_sum / ticks),
                    mspt_max: r.mspt_max,
                    tps_avg: has_ticks.then(|| r.tps_sum / ticks),
                    tps_min: r.tps_min,
                    paused_seconds: u32::try_from(r.paused_samples).unwrap_or(u32::MAX),
                    ..Default::default()
                }
            })
            .collect();
        Ok((periods, since))
    }

    async fn to_proto(&self, server: Server, hostnames: &Hostnames) -> pb::Server {
        let node = self.state.nodes.for_server(&server);
        let node_offline = matches!(&node, Node::Remote(r) if !r.is_online());
        let node_update_needed = matches!(&node, Node::Remote(r) if r.outdated().is_some());
        let node_name = hostnames
            .node_names
            .get(&server.node_id)
            .cloned()
            .unwrap_or_else(|| server.node_id.clone());
        let players = node.players(&server.id);
        let restart_required = node.restart_required(&server).await;
        let countdown = countdown::pending(&server.id);
        pb::Server {
            status: proto_status(&server.status).into(),
            settings: proto_settings(&server.settings()).into(),
            hostnames: hostnames.for_server(&server),
            players_online: players.map(|p| p.online),
            players_max: players.map(|p| p.max),
            restart_required,
            paused: server.status == "running" && node.paused(&server.id),
            countdown: countdown
                .map(|(a, _)| a.as_str().to_owned())
                .unwrap_or_default(),
            countdown_at_ms: countdown.map_or(0, |(_, at)| at),
            modpack: proto_modpack(server.modpack_json.as_deref()).into(),
            internal_port: u32::try_from(server.internal_port).unwrap_or_default(),
            sleep_when_empty: server.sleep_when_empty,
            created_at_ms: server.created_at,
            last_started_at_ms: server.last_started_at,
            node_offline,
            node_update_needed,
            node_name,
            node_id: server.node_id,
            id: server.id,
            name: server.name,
            slug: server.slug,
            mc_version: server.mc_version,
            loader: server.loader,
            ..Default::default()
        }
    }

    /// Creates a server on `node`. Servers on other machines get a slug and
    /// ports unique across the panel, and are mirrored here.
    async fn create_on(
        &self,
        node: &Node,
        node_id: &str,
        mut input: CreateServer,
    ) -> Result<Server, ConnectError> {
        // One create at a time, so slugs and ports chosen here stay unique.
        let _guard = CREATE_LOCK.lock().await;
        if node.is_local() {
            return node.create(input).await.map_err(node_error);
        }
        let store = self.state.auth.store();
        let base = nn_orchestrator::slugify(&input.name);
        let mut slug = base.clone();
        let mut n = 2;
        while store.slug_exists(&slug).await.map_err(store_error)? {
            slug = format!("{base}-{n}");
            n += 1;
        }
        input.slug = Some(slug);
        input.avoid_ports = store.used_ports().await.map_err(store_error)?;
        let mut server = node.create(input).await.map_err(node_error)?;
        // The agent's own records call its machine "local".
        server.node_id = node_id.to_owned();
        store
            .upsert_server_mirror(&server, node_id)
            .await
            .map_err(store_error)?;
        Ok(server)
    }

    /// Runs a lifecycle operation in the background, reporting failures as
    /// `server_operation_failed` events.
    /// Restarts or stops a server now, or after warning its players for
    /// `warn_minutes`. Doing it now replaces a countdown that's running.
    fn after_warning<F, Fut>(
        &self,
        node: Node,
        id: String,
        action: countdown::Action,
        warn_minutes: u32,
        run: F,
    ) -> Result<(), ConnectError>
    where
        F: FnOnce(Node, String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), NodeError>> + Send + 'static,
    {
        if warn_minutes == 0 {
            countdown::supersede(&self.state, &id);
            self.spawn_operation(node, id, action.as_str(), run);
            return Ok(());
        }
        let ticket = countdown::begin(&self.state, &id, action, warn_minutes)
            .map_err(ConnectError::failed_precondition)?;
        let (state, this) = (self.state.clone(), Self::new(self.state.clone()));
        tokio::spawn(async move {
            if ticket.wait(&state).await {
                this.spawn_operation(node, id, action.as_str(), run);
            }
        });
        Ok(())
    }

    fn spawn_operation<F, Fut>(&self, node: Node, id: String, operation: &'static str, run: F)
    where
        F: FnOnce(Node, String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), NodeError>> + Send + 'static,
    {
        let events = self.state.events.clone();
        tokio::spawn(async move {
            if let Err(error) = run(node, id.clone()).await {
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
    /// Node names by ID.
    node_names: HashMap<String, String>,
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

const PROPERTIES_FILE: &str = "server.properties";

/// Reads `server.properties` as form fields, plus the file version.
async fn read_properties(
    files: &nn_node::Folder,
) -> Result<Option<(nn_files::Properties, String)>, ConnectError> {
    let path = RelPath::parse(PROPERTIES_FILE).map_err(file_error)?;
    let content = files
        .read_if_exists(&path, nn_files::EDITOR_LIMIT)
        .await
        .map_err(file_error)?;
    Ok(content.map(|c| {
        (
            nn_files::Properties::parse(&String::from_utf8_lossy(&c.data)),
            c.version,
        )
    }))
}

fn properties_response(
    read: Option<(nn_files::Properties, String)>,
) -> pb::GetServerPropertiesResponse {
    use nn_orchestrator::properties::{self as schema, Group, Kind};
    let Some((props, version)) = read else {
        return pb::GetServerPropertiesResponse::default();
    };
    let entries = props.entries();
    let value_of = |key: &str| entries.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let mut out = Vec::new();
    for group in Group::ALL {
        for field in schema::FIELDS.iter().filter(|f| f.group == group) {
            let Some(value) = value_of(field.key) else {
                continue;
            };
            let (ty, options, min, max) = match field.kind {
                Kind::Bool => (pb::PropertyType::PROPERTY_TYPE_BOOL, Vec::new(), None, None),
                Kind::Int { min, max } => (
                    pb::PropertyType::PROPERTY_TYPE_INT,
                    Vec::new(),
                    Some(min),
                    Some(max),
                ),
                Kind::Choice(options) => (
                    pb::PropertyType::PROPERTY_TYPE_CHOICE,
                    options.iter().map(|o| (*o).to_owned()).collect(),
                    None,
                    None,
                ),
                Kind::Text => (pb::PropertyType::PROPERTY_TYPE_TEXT, Vec::new(), None, None),
            };
            let secret = schema::is_secret(field.key);
            out.push(pb::ServerProperty {
                key: field.key.to_owned(),
                value: if secret {
                    String::new()
                } else {
                    value.to_owned()
                },
                label: field.label.to_owned(),
                help: field.help.to_owned(),
                group: group.label().to_owned(),
                r#type: ty.into(),
                options,
                min,
                max,
                managed_reason: schema::managed_reason(field.key)
                    .unwrap_or_default()
                    .to_owned(),
                secret,
                ..Default::default()
            });
        }
    }
    let mut other: Vec<_> = entries
        .iter()
        .filter(|(k, _)| schema::field(k).is_none())
        .collect();
    other.sort_by_key(|(k, _)| *k);
    for (key, value) in other {
        out.push(pb::ServerProperty {
            key: (*key).to_owned(),
            value: (*value).to_owned(),
            label: (*key).to_owned(),
            group: Group::Other.label().to_owned(),
            r#type: pb::PropertyType::PROPERTY_TYPE_TEXT.into(),
            ..Default::default()
        });
    }
    pb::GetServerPropertiesResponse {
        exists: true,
        properties: out,
        version,
        ..Default::default()
    }
}

/// Maps node errors to Connect codes with user-facing messages.
pub(crate) fn node_error(err: NodeError) -> ConnectError {
    match err {
        NodeError::Unavailable(_) => ConnectError::failed_precondition(err.to_string()),
        NodeError::Orchestrator(e) => orchestrator_error(e),
        NodeError::NoDataFolder | NodeError::Offline(_) | NodeError::NeedsUpdate(_) => {
            ConnectError::unavailable(err.to_string())
        }
        NodeError::File(e) => crate::files::file_error(e),
        NodeError::NotYet(_) => ConnectError::failed_precondition(err.to_string()),
        NodeError::Remote(e) => {
            use nn_node::protocol::ErrorKind;
            match e.kind {
                ErrorKind::NotFound => ConnectError::not_found(e.message),
                ErrorKind::InvalidState => ConnectError::failed_precondition(e.message),
                ErrorKind::InvalidInput => ConnectError::invalid_argument(e.message),
                ErrorKind::NoPorts => ConnectError::resource_exhausted(e.message),
                ErrorKind::Command | ErrorKind::Unavailable => ConnectError::unavailable(e.message),
                ErrorKind::Internal => {
                    tracing::error!(error = %e.message, "a server operation failed on another machine");
                    ConnectError::internal(e.message)
                }
            }
        }
    }
}

pub(crate) fn orchestrator_error(err: OrchestratorError) -> ConnectError {
    match err {
        OrchestratorError::NotFound => ConnectError::not_found("server not found"),
        OrchestratorError::InvalidState(m) => ConnectError::failed_precondition(m),
        OrchestratorError::InvalidInput(m) => ConnectError::invalid_argument(m),
        OrchestratorError::NoPorts(_) => ConnectError::resource_exhausted(err.to_string()),
        OrchestratorError::Command(_) => ConnectError::unavailable(err.to_string()),
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
        image: Some(s.image.clone()),
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

/// Keeps what a client left out of an update as it is: the image and the
/// CPU limit (the Configuration tab has no CPU limit field). An image sent
/// empty is the default one; a CPU limit sent as 0 is none.
fn keep_unsent(
    settings: &mut ServerSettings,
    current: &nn_store::Server,
    image_sent: bool,
    cpu_sent: bool,
) {
    if !image_sent {
        settings.image = current.image.clone();
    }
    if !cpu_sent {
        settings.cpu_limit = current.cpu_limit;
    }
}

fn settings_from_proto(s: &pb::ServerSettingsView<'_>) -> ServerSettings {
    let text = |v: &str| Some(v.trim().to_owned()).filter(|v| !v.is_empty());
    ServerSettings {
        name: s.name.trim().to_owned(),
        mc_version: s.mc_version.trim().to_owned(),
        loader: s.loader.trim().to_ascii_lowercase(),
        loader_version: text(s.loader_version),
        image: s.image.unwrap_or_default().trim().to_owned(),
        java_version: s.java_version.map(i64::from),
        memory_mb: i64::from(s.memory_mb),
        // 0 (or less) removes the limit.
        cpu_limit: s.cpu_limit.filter(|c| *c > 0.0),
        auto_start: s.auto_start,
        sleep_when_empty: s.sleep_when_empty,
        auto_stop_idle_minutes: s.idle_minutes.map(i64::from),
        restart_on_crash: s.restart_on_crash,
        use_aikar_flags: s.use_aikar_flags,
        motd: text(s.motd),
        max_players: i64::from(s.max_players),
        card_theme: theme_name(s.card_theme.as_known()).to_owned(),
        // Not part of the editable settings; kept from the server.
        modpack_json: None,
    }
}

pub(crate) fn proto_modpack(json: Option<&str>) -> Option<pb::Modpack> {
    let pack: nn_orchestrator::spec::Modpack = serde_json::from_str(json?).ok()?;
    Some(pb::Modpack {
        platform: match pack.platform.as_str() {
            "curseforge" => pb::Platform::PLATFORM_CURSEFORGE,
            _ => pb::Platform::PLATFORM_MODRINTH,
        }
        .into(),
        icon_url: crate::content::icons::proxied(pack.icon_url.as_deref()),
        project_id: pack.project_id,
        slug: pack.slug,
        title: pack.title,
        version_id: pack.version_id.unwrap_or_default(),
        version_name: pack.version_name.unwrap_or_default(),
        loader: pack.loader.unwrap_or_default(),
        page_url: pack.page_url.unwrap_or_default(),
        loader_version: pack.loader_version.unwrap_or_default(),
        server_pack: pack.server_pack.map(|p| p.file_name).unwrap_or_default(),
        no_server_pack: pack.no_server_pack.unwrap_or_default(),
        ..Default::default()
    })
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
        let node_id = match request.node_id.trim() {
            "" => nn_store::LOCAL_NODE,
            id => id,
        };
        if self
            .state
            .auth
            .store()
            .find_node(node_id)
            .await
            .map_err(store_error)?
            .is_none()
        {
            return Err(ConnectError::invalid_argument("no such node"));
        }
        let node = self.state.nodes.get(node_id);
        if let Some(e) = node.unavailable() {
            return Err(node_error(e));
        }
        let loader = request.loader.trim().to_ascii_lowercase();
        let mut mc_version = request.mc_version.trim().to_owned();
        let modpack = match loader.as_str() {
            "modrinth" | "curseforge" => {
                let source = request
                    .modpack
                    .as_option()
                    .ok_or_else(|| ConnectError::invalid_argument("choose a modpack"))?;
                let project = source.project.trim();
                let version = Some(source.version_id.trim()).filter(|v| !v.is_empty());
                let content = &self.state.content;
                let resolved = if loader == "modrinth" {
                    content.resolve_modrinth(project, version).await?
                } else {
                    content
                        .resolve_curseforge(project, version, Some(mc_version.as_str()))
                        .await?
                };
                mc_version = resolved.mc_version;
                Some(resolved.modpack)
            }
            _ => None,
        };
        let input = CreateServer {
            name: request.name.to_owned(),
            loader,
            mc_version,
            modpack,
            loader_version: Some(request.loader_version.to_owned()),
            memory_mb: i64::from(request.memory_mb),
            card_theme: theme_name(request.card_theme.as_known()).to_owned(),
            eula_accepted: request.eula_accepted,
            sleep_when_empty: request.sleep_when_empty,
            start_after_create: request.start_after_create,
            slug: None,
            avoid_ports: Vec::new(),
        };
        let server = self.create_on(&node, node_id, input).await?;
        let pack = server
            .modpack_json
            .as_deref()
            .and_then(|j| serde_json::from_str::<nn_orchestrator::spec::Modpack>(j).ok())
            .map(|p| json!({ "platform": p.platform, "slug": p.slug, "version": p.version_id }));
        let details = json!({ "name": server.name, "loader": server.loader, "version": server.mc_version, "modpack": pack, "eula_accepted": true });
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
        let current = self.load(request.id).await?;
        let node = self.node(&current)?;
        let mut settings = request
            .settings
            .as_option()
            .map(settings_from_proto)
            .ok_or_else(|| ConnectError::invalid_argument("settings are required"))?;
        if let Some(sent) = request.settings.as_option() {
            keep_unsent(
                &mut settings,
                &current,
                sent.image.is_some(),
                sent.cpu_limit.is_some(),
            );
        }
        settings.modpack_json = current.modpack_json;
        let (mut server, _) = node
            .update(request.id, settings)
            .await
            .map_err(node_error)?;
        if !node.is_local() {
            server.node_id = current.node_id.clone();
            self.state
                .auth
                .store()
                .upsert_server_mirror(&server, &current.node_id)
                .await
                .map_err(store_error)?;
        }
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
        let server = self.load(request.id).await?;
        let node = self.node(&server)?;
        node.delete(&server.id, request.delete_data)
            .await
            .map_err(node_error)?;
        if !node.is_local() {
            self.state
                .auth
                .store()
                .delete_server(&server.id)
                .await
                .map_err(store_error)?;
            self.state.events.publish(Event::ServerStatusChanged {
                server_id: server.id.clone(),
                status: "deleted".into(),
            });
        }
        // Its own port, unless other routes use that listener now.
        if let Err(e) = self
            .state
            .auth
            .store()
            .remove_unused_server_ports(&server.id)
            .await
        {
            tracing::warn!(server = %server.id, error = %e, "couldn't remove a deleted server's port");
        }
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

    async fn move_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::MoveServerRequest>,
    ) -> ServiceResult<pb::MoveServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let server = self.load(request.id).await?;
        let (id, from) = (server.id.clone(), server.node_id.clone());
        let job_id =
            crate::moves::start_move(&self.state, server, request.node_id, &user.id).await?;
        let store = self.state.auth.store();
        let name = async |node_id: &str| match store.find_node(node_id).await {
            Ok(Some(node)) => node.name,
            _ => node_id.to_owned(),
        };
        let details = json!({ "from": name(&from).await, "to": name(request.node_id).await });
        audit(&self.state, &user, "server.move", ("server", &id), details).await;
        Response::ok(pb::MoveServerResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn start_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::StartServerRequest>,
    ) -> ServiceResult<pb::StartServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let server = self.load(request.id).await?;
        let node = self.node(&server)?;
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
        self.spawn_operation(node, server.id.clone(), "start", |n, id| async move {
            n.start(&id).await
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
        let server = self.load(request.id).await?;
        let node = self.node(&server)?;
        self.after_warning(
            node,
            server.id.clone(),
            countdown::Action::Stop,
            request.warn_minutes,
            |n, id| async move { n.stop(&id, false).await },
        )?;
        audit(
            &self.state,
            &user,
            "server.stop",
            ("server", &server.id),
            json!({ "warn_minutes": request.warn_minutes }),
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
        let server = self.load(request.id).await?;
        let node = self.node(&server)?;
        self.after_warning(
            node,
            server.id.clone(),
            countdown::Action::Restart,
            request.warn_minutes,
            |n, id| async move { n.restart(&id).await },
        )?;
        audit(
            &self.state,
            &user,
            "server.restart",
            ("server", &server.id),
            json!({ "warn_minutes": request.warn_minutes }),
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
        let server = self.load(request.id).await?;
        let node = self.node(&server)?;
        countdown::supersede(&self.state, &server.id);
        self.spawn_operation(node, server.id.clone(), "kill", |n, id| async move {
            n.kill(&id).await
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

    async fn cancel_countdown(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CancelCountdownRequest>,
    ) -> ServiceResult<pb::CancelCountdownResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let server = self.load(request.id).await?;
        let Some((action, _)) = countdown::pending(&server.id) else {
            return Err(ConnectError::failed_precondition(
                "Nothing is counting down for this server.",
            ));
        };
        countdown::cancel(&self.state, &server.id);
        audit(
            &self.state,
            &user,
            "server.countdown_cancel",
            ("server", &server.id),
            json!({ "action": action.as_str() }),
        )
        .await;
        Response::ok(pb::CancelCountdownResponse::default())
    }

    async fn get_server_stats(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetServerStatsRequest>,
    ) -> ServiceResult<pb::GetServerStatsResponse> {
        require(&ctx, Permission::View)?;
        let node = self.state.nodes.for_server_id(request.id).await;
        let range = request.range.as_known();
        let (periods, since_ms) = match range {
            Some(pb::StatsRange::STATS_RANGE_UNSPECIFIED) | None => (Vec::new(), 0),
            Some(range) => self.stat_periods(request.id, range).await?,
        };
        let samples = if periods.is_empty() && since_ms == 0 {
            node.stats_history(request.id).await
        } else {
            Vec::new()
        };
        let samples = samples
            .into_iter()
            .map(|s| pb::StatSample {
                at_ms: s.at_ms,
                cpu_percent: s.cpu_percent,
                memory_bytes: s.memory_bytes,
                memory_limit_bytes: s.memory_limit_bytes,
                net_rx_bytes: s.net_rx_bytes,
                net_tx_bytes: s.net_tx_bytes,
                disk_read_bytes: s.disk_read_bytes,
                disk_write_bytes: s.disk_write_bytes,
                players_online: s.players_online,
                mspt: s.tick.map(|t| t.mspt),
                mspt_p95: s.tick.and_then(|t| t.mspt_p95),
                tps: s.tick.map(|t| t.tps),
                ..Default::default()
            })
            .collect();
        let usage = node.folder_usage(request.id).await;
        let day_ago = nn_store::now_ms() - 24 * 60 * 60 * 1000;
        let players_peak_day = self
            .state
            .auth
            .store()
            .player_peak_since(request.id, day_ago)
            .await
            .unwrap_or(0);
        Response::ok(pb::GetServerStatsResponse {
            folder_bytes: usage.map(|u| u.total_bytes),
            world_bytes: usage.map(|u| u.world_bytes),
            players_peak_day,
            samples,
            periods,
            since_ms,
            ..Default::default()
        })
    }

    async fn copy_server(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CopyServerRequest>,
    ) -> ServiceResult<pb::CopyServerResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let source = self.load(request.id).await?;
        let job_id = crate::copies::start_copy(
            &self.state,
            source.clone(),
            request.name,
            request.node_id,
            request.include_world,
        )
        .await?;
        audit(
            &self.state,
            &user,
            "server.copy",
            ("server", &source.id),
            json!({ "name": request.name.trim(), "node": request.node_id, "world": request.include_world }),
        )
        .await;
        Response::ok(pb::CopyServerResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn save_server_template(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SaveServerTemplateRequest>,
    ) -> ServiceResult<pb::SaveServerTemplateResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let source = self.load(request.id).await?;
        let job_id =
            crate::copies::start_save_template(&self.state, source.clone(), request.name).await?;
        audit(
            &self.state,
            &user,
            "server.template_save",
            ("server", &source.id),
            json!({ "name": request.name.trim() }),
        )
        .await;
        Response::ok(pb::SaveServerTemplateResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn list_server_templates(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListServerTemplatesRequest>,
    ) -> ServiceResult<pb::ListServerTemplatesResponse> {
        require(&ctx, Permission::ManageServers)?;
        let templates = self
            .state
            .auth
            .store()
            .list_templates()
            .await
            .map_err(store_error)?
            .into_iter()
            .map(|t| pb::ServerTemplate {
                id: t.id,
                name: t.name,
                source_name: t.source_name,
                loader: t.loader,
                mc_version: t.mc_version,
                memory_mb: u32::try_from(t.memory_mb).unwrap_or(0),
                size_bytes: u64::try_from(t.archive_bytes).unwrap_or(0),
                created_at_ms: t.created_at,
                ..Default::default()
            })
            .collect();
        Response::ok(pb::ListServerTemplatesResponse {
            templates,
            ..Default::default()
        })
    }

    async fn delete_server_template(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteServerTemplateRequest>,
    ) -> ServiceResult<pb::DeleteServerTemplateResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let template = crate::copies::delete_template(&self.state, request.id).await?;
        audit(
            &self.state,
            &user,
            "server.template_delete",
            ("template", &template.id),
            json!({ "name": template.name }),
        )
        .await;
        Response::ok(pb::DeleteServerTemplateResponse::default())
    }

    async fn create_server_from_template(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateServerFromTemplateRequest>,
    ) -> ServiceResult<pb::CreateServerFromTemplateResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let template = self
            .state
            .auth
            .store()
            .find_template(request.template_id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| ConnectError::not_found("That template doesn't exist."))?;
        let template_id = template.id.clone();
        let job_id = crate::copies::start_from_template(
            &self.state,
            template,
            request.name,
            request.node_id,
        )
        .await?;
        audit(
            &self.state,
            &user,
            "server.create_from_template",
            ("template", &template_id),
            json!({ "name": request.name.trim(), "node": request.node_id }),
        )
        .await;
        Response::ok(pb::CreateServerFromTemplateResponse {
            job_id,
            ..Default::default()
        })
    }

    async fn search_server_logs(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::SearchServerLogsRequest>,
    ) -> ServiceResult<pb::SearchServerLogsResponse> {
        require(&ctx, Permission::ManagePlayers)?;
        let query = request.query.trim();
        if query.is_empty() && !request.chat_only {
            return Err(ConnectError::invalid_argument(
                "type something to search for",
            ));
        }
        if query.chars().count() > 200 {
            return Err(ConnectError::invalid_argument("that search is too long"));
        }
        let (_, folder) = crate::files::open(&self.state, request.id).await?;
        let found = crate::log_search::search(&folder, query, request.chat_only)
            .await
            .map_err(crate::files::file_error)?;
        Response::ok(pb::SearchServerLogsResponse {
            matches: found
                .matches
                .into_iter()
                .map(|m| pb::LogMatch {
                    file: m.file,
                    file_modified_ms: m.file_modified_ms,
                    line: m.line,
                    text: m.text,
                    ..Default::default()
                })
                .collect(),
            files_searched: found.files_searched,
            files_total: found.files_total,
            truncated: found.truncated,
            ..Default::default()
        })
    }

    async fn get_host_info(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetHostInfoRequest>,
    ) -> ServiceResult<pb::GetHostInfoResponse> {
        require(&ctx, Permission::View)?;
        let node = self.state.nodes.local();
        let disk = node.disk_space();
        let mut response = match node.host_info().await {
            Ok(info) => pb::GetHostInfoResponse {
                docker_available: true,
                hostname: info.hostname,
                operating_system: info.operating_system,
                docker_version: info.docker_version,
                cpus: info.cpus,
                memory_bytes: info.memory_bytes,
                ..Default::default()
            },
            Err(NodeError::Unavailable(reason)) => pb::GetHostInfoResponse {
                docker_available: false,
                error: reason.to_string(),
                ..Default::default()
            },
            Err(e) => pb::GetHostInfoResponse {
                docker_available: false,
                error: e.to_string(),
                ..Default::default()
            },
        };
        if let Some((total, free)) = disk {
            response.disk_total_bytes = Some(total);
            response.disk_free_bytes = Some(free);
        }
        Response::ok(response)
    }

    async fn get_server_properties(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetServerPropertiesRequest>,
    ) -> ServiceResult<pb::GetServerPropertiesResponse> {
        require(&ctx, Permission::ManageServers)?;
        let (_, files) = open(&self.state, request.id).await?;
        Response::ok(properties_response(read_properties(&files).await?))
    }

    async fn update_server_properties(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateServerPropertiesRequest>,
    ) -> ServiceResult<pb::UpdateServerPropertiesResponse> {
        let user = require(&ctx, Permission::ManageServers)?.clone();
        let mut changes: Vec<(String, String)> = Vec::new();
        for (key, value) in &request.values {
            nn_orchestrator::properties::validate(key, value)
                .map_err(ConnectError::invalid_argument)?;
            changes.push(((*key).to_owned(), value.trim().to_owned()));
        }
        let (server, files) = open(&self.state, request.id).await?;
        let Some((mut props, version)) = read_properties(&files).await? else {
            return Err(ConnectError::failed_precondition(
                "server.properties doesn't exist yet; start the server once to create it",
            ));
        };
        if version != request.version {
            return Err(file_error(FileError::Conflict));
        }
        for (key, value) in &changes {
            props.set(key, value);
        }
        let text = props.to_string();
        let path = RelPath::parse(PROPERTIES_FILE).map_err(file_error)?;
        files
            .write(&path, text.into_bytes(), Expect::Version(version))
            .await
            .map_err(file_error)?;
        let details: serde_json::Map<String, serde_json::Value> = changes
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        audit(
            &self.state,
            &user,
            "server.properties",
            ("server", &server.id),
            json!({ "changes": details }),
        )
        .await;
        Response::ok(pb::UpdateServerPropertiesResponse {
            properties: properties_response(read_properties(&files).await?).into(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> nn_store::Server {
        serde_json::from_value(serde_json::json!({
            "id": "s", "name": "SMP", "slug": "smp", "status": "stopped", "mc_version": "1.21.4",
            "loader": "paper", "loader_version": null, "image": "ghcr.io/me/custom:1",
            "java_version": null, "memory_mb": 2048, "cpu_limit": 2.0, "internal_port": 30000,
            "public_port": null, "hostname": null, "auto_start": false,
            "auto_stop_idle_minutes": null, "sleep_when_empty": false, "restart_on_crash": true,
            "use_aikar_flags": true, "motd": null, "max_players": 20, "card_theme": "nether",
            "container_id": null, "rcon_port": null, "rcon_password_enc": null,
            "eula_accepted_at": null, "last_started_at": null, "created_at": 0, "updated_at": 0,
            "env_json": "{}", "mgmt_port": null, "mgmt_secret_enc": null, "modpack_json": null,
            "node_id": "local"
        }))
        .unwrap()
    }

    #[test]
    fn updates_keep_what_isnt_sent() {
        let current = server();
        let mut settings = current.settings();
        settings.image = String::new();
        settings.cpu_limit = None;
        // The Configuration tab: the image sent empty, no CPU limit.
        let mut from_tab = settings.clone();
        keep_unsent(&mut from_tab, &current, true, false);
        assert_eq!(from_tab.image, "", "empty means the default image");
        assert_eq!(from_tab.cpu_limit, Some(2.0), "the CPU limit stays");
        // Neither sent: both stay.
        let mut neither = settings.clone();
        keep_unsent(&mut neither, &current, false, false);
        assert_eq!(neither.image, "ghcr.io/me/custom:1");
        // Both sent: as sent.
        keep_unsent(&mut settings, &current, true, true);
        assert_eq!((settings.image.as_str(), settings.cpu_limit), ("", None));
    }
}
