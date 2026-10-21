//! Modules: helper containers that run next to a server, made from
//! [templates](template) (a web map, for example).
//!
//! - Each module is its own container, `nethernode-<slug>.<name>`, labelled
//!   with its module and server IDs but never
//!   [`LABEL_MANAGED`](crate::spec::LABEL_MANAGED), so server
//!   reconciliation and Docker event handling don't see it.
//! - A server with modules gets a private network, `nethernode-<slug>.modules`.
//!   Its modules join it, and the server's container is connected to it
//!   (as `minecraft`) when either starts. The network isn't part of the
//!   server's container definition, so adding it doesn't recreate servers.
//! - Files live in the server's folder under `modules/<name>/`, with
//!   `modules/<name>/cache/` for what the module makes again on its own
//!   (left out of backups).
//! - Web pages are published on the backend address, like game ports.
//! - Modules that run with their server follow its status; always-on ones
//!   are started when NetherNode starts.

pub mod template;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkConnectRequest, NetworkCreateRequest,
    NetworkDisconnectRequest, PortBinding, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, StartContainerOptions, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use nn_events::Event;
use nn_store::{LOCAL_NODE, Module, NewModule, Server};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{OrchestratorError, Result};
use crate::lifecycle::{Orchestrator, is_not_found};
use crate::spec::{self, ContainerSpec};
pub use template::{Lifecycle, Template};

/// Label holding a module container's module ID.
pub const LABEL_MODULE: &str = "nethernode.module";
/// Label holding the ID of the server a module belongs to.
pub const LABEL_MODULE_SERVER: &str = "nethernode.module_server";
/// Label holding a hash of the module's container definition.
const LABEL_MODULE_HASH: &str = "nethernode.module_hash";
/// The folder in a server's folder holding its modules' files.
pub const MODULES_DIR: &str = "modules";
/// The folder in a module's folder that backups leave out.
pub const CACHE_DIR: &str = "cache";
/// Seconds a module gets to stop before it's killed.
const STOP_SECONDS: i32 = 30;
const MEMORY_RANGE: std::ops::RangeInclusive<i64> = 128..=16_384;

/// A module's container name.
pub fn container_name(slug: &str, name: &str) -> String {
    format!("{}.{name}", spec::container_name(slug))
}

/// The private network of a server's modules.
pub fn network_name(slug: &str) -> String {
    format!("{}.modules", spec::container_name(slug))
}

/// What a module's container is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleState {
    /// `running`, `stopped`, `failed` (exited with an error) or `absent`
    /// (never started, or removed).
    pub status: String,
    /// Why it failed, when it did.
    pub message: Option<String>,
}

/// Everything that goes into a module's container.
struct SpecInput<'a> {
    module: &'a Module,
    slug: &'a str,
    template: &'a Template,
    env: BTreeMap<String, String>,
    binds: Vec<String>,
    publish_ip: std::net::IpAddr,
    run_as: Option<(u32, u32)>,
}

/// A module's container definition.
fn build(input: &SpecInput<'_>) -> ContainerSpec {
    let mut port_bindings = HashMap::new();
    let mut exposed = Vec::new();
    if let (Some(web), Some(port)) = (&input.template.web, input.module.web_port) {
        let inside = format!("{}/tcp", web.port);
        port_bindings.insert(
            inside.clone(),
            Some(vec![PortBinding {
                host_ip: Some(input.publish_ip.to_string()),
                host_port: Some(port.to_string()),
            }]),
        );
        exposed.push(inside);
    }
    let host_config = HostConfig {
        binds: Some(input.binds.clone()),
        port_bindings: Some(port_bindings),
        memory: Some(input.module.memory_mb * 1024 * 1024),
        network_mode: Some(network_name(input.slug)),
        // NetherNode starts and stops modules itself.
        restart_policy: Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::NO),
            maximum_retry_count: None,
        }),
        security_opt: Some(vec!["no-new-privileges".into()]),
        init: Some(true),
        ..Default::default()
    };
    let mut labels = HashMap::from([
        (LABEL_MODULE.to_owned(), input.module.id.clone()),
        (
            LABEL_MODULE_SERVER.to_owned(),
            input.module.server_id.clone(),
        ),
    ]);
    let mut body = ContainerCreateBody {
        image: Some(input.template.image.clone()),
        cmd: (!input.template.command.is_empty()).then(|| input.template.command.clone()),
        env: Some(input.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        exposed_ports: Some(exposed),
        user: input.run_as.map(|(uid, gid)| format!("{uid}:{gid}")),
        host_config: Some(host_config),
        labels: Some(labels.clone()),
        ..Default::default()
    };
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&body).unwrap_or_default(),
    ));
    labels.insert(LABEL_MODULE_HASH.to_owned(), hash.clone());
    body.labels = Some(labels);
    ContainerSpec {
        name: container_name(input.slug, &input.module.name),
        body,
        hash,
    }
}

/// The world folder named in `server.properties` (`world` by default).
fn level_name(server_dir: &Path) -> String {
    std::fs::read_to_string(server_dir.join("server.properties"))
        .ok()
        .and_then(|props| {
            props.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("level-name=")
                    .map(str::trim)
                    .map(str::to_owned)
            })
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "world".to_owned())
}

/// Whether a module should be running given its server's status: `Some`
/// to start or stop it, `None` to leave it as it is.
pub fn wanted(lifecycle: Lifecycle, server_status: &str) -> Option<bool> {
    match lifecycle {
        Lifecycle::Always => Some(true),
        Lifecycle::Manual => None,
        Lifecycle::WithServer => match server_status {
            "running" => Some(true),
            "stopped" | "crashed" | "sleeping" => Some(false),
            _ => None,
        },
    }
}

impl Orchestrator {
    fn module_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.module_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_owned())
            .or_default()
            .clone()
    }

    async fn load_module(&self, id: &str) -> Result<(Module, Server, &'static Template)> {
        let module = self
            .store
            .find_module(id)
            .await?
            .ok_or_else(|| OrchestratorError::InvalidInput("No such module.".into()))?;
        let server = self.load(&module.server_id).await?;
        let template = template::find(&module.template).ok_or_else(|| {
            OrchestratorError::InvalidState(format!(
                "This NetherNode doesn't know the {} module.",
                module.template
            ))
        })?;
        Ok((module, server, template))
    }

    /// The folder of module `name` on `server`, as NetherNode sees it.
    fn module_dir(&self, server: &Server, name: &str) -> Result<PathBuf> {
        if !template::valid_name(name) {
            return Err(OrchestratorError::InvalidInput(
                "invalid module name".into(),
            ));
        }
        Ok(self.server_dir(&server.slug)?.join(MODULES_DIR).join(name))
    }

    /// The same folder as the Docker host sees it, for bind mounts.
    fn host_module_root(&self, server: &Server) -> PathBuf {
        self.config
            .host_servers_dir
            .as_deref()
            .unwrap_or(&self.config.servers_dir)
            .join(&server.slug)
    }

    /// Adds a module to a server. It isn't started here.
    pub async fn add_module(
        &self,
        server_id: &str,
        template_id: &str,
        name: Option<&str>,
        settings: &Map<String, Value>,
        lifecycle: Option<Lifecycle>,
        memory_mb: Option<i64>,
    ) -> Result<Module> {
        let server = self.load(server_id).await?;
        if server.node_id != LOCAL_NODE {
            return Err(OrchestratorError::InvalidState(
                "Modules run on this machine only for now.".into(),
            ));
        }
        let template = template::find(template_id).ok_or_else(|| {
            OrchestratorError::InvalidInput(format!("There's no {template_id} module."))
        })?;
        let name = name
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&template.id);
        if !template::valid_name(name) {
            return Err(OrchestratorError::InvalidInput(
                "Module names are up to 32 lowercase letters, digits and dashes.".into(),
            ));
        }
        let settings =
            template::settings(template, settings).map_err(OrchestratorError::InvalidInput)?;
        let memory_mb = memory_mb.unwrap_or(template.memory_mb);
        if !MEMORY_RANGE.contains(&memory_mb) {
            return Err(OrchestratorError::InvalidInput(
                "A module's memory must be between 128 MB and 16 GB.".into(),
            ));
        }
        let existing = self.store.list_modules(Some(server_id)).await?;
        if existing.iter().any(|m| m.name == name) {
            return Err(OrchestratorError::InvalidInput(format!(
                "{} already has a module called {name}.",
                server.name
            )));
        }
        let dir = self.module_dir(&server, name)?;
        let settings_json = Value::Object(settings).to_string();
        let lifecycle = lifecycle.unwrap_or(template.lifecycle);
        let module = {
            let _guard = self.create_lock.lock().await;
            let web_port = match &template.web {
                None => None,
                Some(_) => {
                    let used = self.store.used_ports().await?;
                    let range = self.config.module_port_range.clone();
                    let ip = self.config.backend_ip;
                    let port = crate::ports::allocate(range.clone(), &used, 1, |p| {
                        crate::ports::is_free(ip, p)
                    })
                    .and_then(|p| p.first().copied())
                    .ok_or_else(|| {
                        OrchestratorError::NoPorts(format!("{}–{}", range.start(), range.end()))
                    })?;
                    Some(i64::from(port))
                }
            };
            self.store
                .insert_module(NewModule {
                    server_id,
                    template: &template.id,
                    name,
                    settings_json: &settings_json,
                    lifecycle: lifecycle.as_str(),
                    memory_mb,
                    web_port,
                    web_host: template.web.as_ref().and_then(|w| w.prefix.as_deref()),
                })
                .await?
        };
        std::fs::create_dir_all(&dir)?;
        tracing::info!(server = %server_id, module = %module.id, template = %template.id, "module added");
        Ok(module)
    }

    /// Changes a module's settings. A running module restarts with them.
    pub async fn update_module(
        &self,
        id: &str,
        settings: &Map<String, Value>,
        lifecycle: Lifecycle,
        memory_mb: i64,
    ) -> Result<Module> {
        let lock = self.module_lock(id);
        let _held = lock.lock().await;
        let (module, server, template) = self.load_module(id).await?;
        let settings =
            template::settings(template, settings).map_err(OrchestratorError::InvalidInput)?;
        if !MEMORY_RANGE.contains(&memory_mb) {
            return Err(OrchestratorError::InvalidInput(
                "A module's memory must be between 128 MB and 16 GB.".into(),
            ));
        }
        self.store
            .update_module(
                id,
                &Value::Object(settings).to_string(),
                lifecycle.as_str(),
                memory_mb,
            )
            .await?;
        let running = self.module_state(&server, &module).await.status == "running";
        if running {
            self.stop_module_locked(&server, &module).await?;
            let (module, server, template) = self.load_module(id).await?;
            self.start_module_locked(&module, &server, template).await?;
        }
        Ok(self.store.find_module(id).await?.unwrap_or(module))
    }

    /// Removes a module: its container, its record, and with `delete_files`
    /// its folder.
    pub async fn remove_module(&self, id: &str, delete_files: bool) -> Result<()> {
        let lock = self.module_lock(id);
        let _held = lock.lock().await;
        let module = self
            .store
            .find_module(id)
            .await?
            .ok_or_else(|| OrchestratorError::InvalidInput("No such module.".into()))?;
        let server = self.load(&module.server_id).await?;
        self.remove_module_container(&server, &module.name).await?;
        self.store.delete_module(id).await?;
        if delete_files {
            let dir = self.module_dir(&server, &module.name)?;
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
        }
        if self.store.list_modules(Some(&server.id)).await?.is_empty() {
            self.remove_network(&server).await;
        }
        self.module_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        tracing::info!(server = %server.id, module = %id, delete_files, "module removed");
        Ok(())
    }

    /// Starts a module (creating or updating its container first).
    pub async fn start_module(&self, id: &str) -> Result<()> {
        let lock = self.module_lock(id);
        let _held = lock.lock().await;
        let (module, server, template) = self.load_module(id).await?;
        self.start_module_locked(&module, &server, template).await
    }

    async fn start_module_locked(
        &self,
        module: &Module,
        server: &Server,
        template: &Template,
    ) -> Result<()> {
        if server.node_id != LOCAL_NODE {
            return Err(OrchestratorError::InvalidState(
                "Modules run on this machine only for now.".into(),
            ));
        }
        let given: Map<String, Value> =
            serde_json::from_str(&module.settings_json).unwrap_or_default();
        let settings = template::settings(template, &given).map_err(|e| {
            OrchestratorError::InvalidState(format!(
                "{} needs its settings fixed: {e}",
                module.name
            ))
        })?;
        let server_dir = self.server_dir(&server.slug)?;
        let world = level_name(&server_dir);
        let context = template::Context::new(server, &world, &module.name, &settings);
        let render = |text: &str| {
            template::render(text, &context.values).map_err(OrchestratorError::InvalidState)
        };

        let dir = self.module_dir(server, &module.name)?;
        let host_server = self.host_module_root(server);
        let host_dir = host_server.join(MODULES_DIR).join(&module.name);
        let mut binds = Vec::new();
        for mount in &template.mounts {
            let source = template::Source::parse(&render(&mount.source)?)
                .map_err(OrchestratorError::InvalidState)?;
            let (local, host) = match &source {
                template::Source::Server(path) => {
                    let local = server_dir.join(path.as_str());
                    if !local.exists() {
                        return Err(OrchestratorError::InvalidState(format!(
                            "{} needs {}'s {} folder. Start the server once so it's made.",
                            template.name,
                            server.name,
                            path.as_str()
                        )));
                    }
                    (local, host_server.join(path.as_str()))
                }
                template::Source::Module(path) => {
                    (dir.join(path.as_str()), host_dir.join(path.as_str()))
                }
                template::Source::Cache(path) => (
                    dir.join(CACHE_DIR).join(path.as_str()),
                    host_dir.join(CACHE_DIR).join(path.as_str()),
                ),
            };
            if !matches!(source, template::Source::Server(_)) {
                // Docker would make missing ones as root.
                std::fs::create_dir_all(&local)?;
            }
            let mode = if mount.read_only { ":ro" } else { "" };
            binds.push(format!("{}:{}{mode}", host.display(), mount.target));
        }
        for file in &template.files {
            let path = nn_files::RelPath::parse(&file.path)
                .map_err(|e| OrchestratorError::InvalidState(e.to_string()))?;
            let target = dir.join(path.as_str());
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, render(&file.content)?)?;
        }
        let mut env = BTreeMap::new();
        for (key, value) in &template.env {
            env.insert(key.clone(), render(value)?);
        }

        self.ensure_network(server).await?;
        let spec = build(&SpecInput {
            module,
            slug: &server.slug,
            template,
            env,
            binds,
            publish_ip: self.config.backend_ip,
            run_as: self.config.run_as,
        });
        let container = match self.docker.inspect_container(&spec.name, None).await {
            Ok(existing) => {
                let hash = existing
                    .config
                    .and_then(|c| c.labels)
                    .and_then(|l| l.get(LABEL_MODULE_HASH).cloned());
                let running = existing.state.and_then(|s| s.running).unwrap_or(false);
                if hash.as_deref() == Some(spec.hash.as_str()) {
                    if running {
                        return Ok(());
                    }
                    None
                } else {
                    tracing::info!(module = %module.id, "module settings changed; recreating its container");
                    self.remove_module_container(server, &module.name).await?;
                    Some(spec)
                }
            }
            Err(e) if is_not_found(&e) => Some(spec),
            Err(e) => return Err(e.into()),
        };
        let name = container_name(&server.slug, &module.name);
        if let Some(spec) = container {
            if self.docker.inspect_image(&template.image).await.is_err() {
                self.pull_module_image(&template.image).await?;
            }
            let options = CreateContainerOptionsBuilder::default()
                .name(&spec.name)
                .build();
            self.docker
                .create_container(Some(options), spec.body)
                .await?;
        }
        self.docker
            .start_container(&name, None::<StartContainerOptions>)
            .await?;
        tracing::info!(server = %server.id, module = %module.id, "module started");
        Ok(())
    }

    /// Stops a module.
    pub async fn stop_module(&self, id: &str) -> Result<()> {
        let lock = self.module_lock(id);
        let _held = lock.lock().await;
        let (module, server, _) = self.load_module(id).await?;
        self.stop_module_locked(&server, &module).await
    }

    async fn stop_module_locked(&self, server: &Server, module: &Module) -> Result<()> {
        let name = container_name(&server.slug, &module.name);
        let options = StopContainerOptionsBuilder::default()
            .t(STOP_SECONDS)
            .build();
        match self.docker.stop_container(&name, Some(options)).await {
            Ok(()) => {
                tracing::info!(server = %server.id, module = %module.id, "module stopped");
                Ok(())
            }
            // Not created yet, or already stopped (304).
            Err(e) if is_not_found(&e) || is_not_modified(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// One module's container state.
    async fn module_state(&self, server: &Server, module: &Module) -> ModuleState {
        let name = container_name(&server.slug, &module.name);
        match self.docker.inspect_container(&name, None).await {
            Ok(c) => {
                let state = c.state.unwrap_or_default();
                if state.running.unwrap_or(false) {
                    return ModuleState {
                        status: "running".into(),
                        message: None,
                    };
                }
                let code = state.exit_code.unwrap_or(0);
                let oom = state.oom_killed.unwrap_or(false);
                // 143 is SIGTERM from a stop.
                if code == 0 || code == 143 {
                    ModuleState {
                        status: "stopped".into(),
                        message: None,
                    }
                } else {
                    ModuleState {
                        status: "failed".into(),
                        message: Some(if oom {
                            "It ran out of memory. Give it more in its settings.".into()
                        } else {
                            match state.error.filter(|e| !e.is_empty()) {
                                Some(error) => error,
                                None => format!("It exited with code {code}. Its log says why."),
                            }
                        }),
                    }
                }
            }
            Err(e) if is_not_found(&e) => ModuleState {
                status: "absent".into(),
                message: None,
            },
            Err(e) => ModuleState {
                status: "failed".into(),
                message: Some(format!("Docker: {e}")),
            },
        }
    }

    /// A server's modules and what each is doing.
    pub async fn modules(&self, server_id: &str) -> Result<Vec<(Module, ModuleState)>> {
        let server = self.load(server_id).await?;
        let mut out = Vec::new();
        for module in self.store.list_modules(Some(server_id)).await? {
            let state = self.module_state(&server, &module).await;
            out.push((module, state));
        }
        Ok(out)
    }

    /// The last `lines` lines a module printed.
    pub async fn module_logs(&self, id: &str, lines: usize) -> Result<Vec<String>> {
        let (module, server, _) = self.load_module(id).await?;
        let name = container_name(&server.slug, &module.name);
        let tail = lines.clamp(1, 2000).to_string();
        let options = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .tail(&tail)
            .build();
        let mut stream = self.docker.logs(&name, Some(options));
        let mut text = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(output) => text.push_str(&output.to_string()),
                Err(e) if is_not_found(&e) => return Ok(Vec::new()),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(text.lines().map(str::to_owned).collect())
    }

    async fn remove_module_container(&self, server: &Server, name: &str) -> Result<()> {
        let options = RemoveContainerOptionsBuilder::default().force(true).build();
        match self
            .docker
            .remove_container(&container_name(&server.slug, name), Some(options))
            .await
        {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Makes the server's module network, and puts the server's container
    /// on it if there is one.
    async fn ensure_network(&self, server: &Server) -> Result<()> {
        let network = network_name(&server.slug);
        if let Err(e) = self.docker.inspect_network(&network, None).await {
            if !is_not_found(&e) {
                return Err(e.into());
            }
            let request = NetworkCreateRequest {
                name: network.clone(),
                driver: Some("bridge".into()),
                labels: Some(HashMap::from([(
                    LABEL_MODULE_SERVER.to_owned(),
                    server.id.clone(),
                )])),
                ..Default::default()
            };
            match self.docker.create_network(request).await {
                Ok(_) => {}
                // Made meanwhile by another module's start.
                Err(e) if is_conflict(&e) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(container) = &server.container_id {
            self.connect_server(&network, container).await;
        }
        Ok(())
    }

    async fn connect_server(&self, network: &str, container: &str) {
        let request = NetworkConnectRequest {
            container: container.to_owned(),
            endpoint_config: Some(EndpointSettings {
                aliases: Some(vec![template::SERVER_HOST.to_owned()]),
                ..Default::default()
            }),
        };
        match self.docker.connect_network(network, request).await {
            Ok(()) => {}
            // Already on it.
            Err(e) if is_forbidden_or_conflict(&e) => {}
            Err(e) => {
                tracing::warn!(%network, error = %e, "couldn't connect a server to its modules' network")
            }
        }
    }

    /// Puts a server's container on its modules' network, if it has
    /// modules. Called whenever the server's container is (re)created.
    pub(crate) async fn connect_modules(&self, server: &Server, container: &str) {
        match self.store.list_modules(Some(&server.id)).await {
            Ok(modules) if !modules.is_empty() => {}
            _ => return,
        }
        let mut server = server.clone();
        server.container_id = Some(container.to_owned());
        if let Err(e) = self.ensure_network(&server).await {
            tracing::warn!(server = %server.id, error = %e, "couldn't set up the modules' network");
        }
    }

    async fn remove_network(&self, server: &Server) {
        let network = network_name(&server.slug);
        if let Some(container) = &server.container_id {
            let request = NetworkDisconnectRequest {
                container: container.clone(),
                force: Some(true),
            };
            let _ = self.docker.disconnect_network(&network, request).await;
        }
        if let Err(e) = self.docker.remove_network(&network).await
            && !is_not_found(&e)
        {
            tracing::warn!(%network, error = %e, "couldn't remove the modules' network");
        }
    }

    /// Removes a server's module containers and network (its module
    /// records go with the server). Before the server is deleted.
    pub(crate) async fn remove_server_modules(&self, server: &Server) -> Result<()> {
        let modules = self.store.list_modules(Some(&server.id)).await?;
        for module in &modules {
            self.remove_module_container(server, &module.name).await?;
        }
        if !modules.is_empty() {
            self.remove_network(server).await;
        }
        Ok(())
    }

    async fn pull_module_image(&self, image: &str) -> Result<()> {
        let (repo, digest) = image.split_once('@').ok_or_else(|| {
            OrchestratorError::InvalidState(format!("module image {image} isn't pinned"))
        })?;
        tracing::info!(%image, "pulling module image");
        let options = CreateImageOptionsBuilder::default()
            .from_image(repo)
            .tag(digest)
            .build();
        let mut stream = self.docker.create_image(Some(options), None, None);
        while let Some(item) = stream.next().await {
            item?;
        }
        Ok(())
    }

    /// Starts or stops one server's modules to match its status.
    async fn follow_server(&self, server_id: &str, status: &str) {
        let Ok(modules) = self.store.list_modules(Some(server_id)).await else {
            return;
        };
        for module in modules {
            let Some(lifecycle) = Lifecycle::parse(&module.lifecycle) else {
                continue;
            };
            // Always-on modules are handled at startup and when started.
            if lifecycle != Lifecycle::WithServer {
                continue;
            }
            self.apply_wanted(&module, wanted(lifecycle, status)).await;
        }
    }

    async fn apply_wanted(&self, module: &Module, wanted: Option<bool>) {
        let result = match wanted {
            Some(true) => self.start_module(&module.id).await,
            Some(false) => self.stop_module(&module.id).await,
            None => return,
        };
        if let Err(e) = result {
            self.report_failure(
                &module.server_id,
                &format!("start the {} module", module.name),
                &e,
            );
        }
    }
}

/// Starts always-on modules, and modules whose server is running, then
/// keeps with-server modules in step with their servers.
pub fn spawn(orch: &Arc<Orchestrator>) {
    let orch = orch.clone();
    let mut events = orch.events.subscribe();
    tokio::spawn(async move {
        if let Ok(servers) = orch.store.list_local_servers().await {
            let status: HashMap<String, String> =
                servers.into_iter().map(|s| (s.id, s.status)).collect();
            for module in orch.store.list_modules(None).await.unwrap_or_default() {
                let (Some(lifecycle), Some(server_status)) = (
                    Lifecycle::parse(&module.lifecycle),
                    status.get(&module.server_id),
                ) else {
                    continue;
                };
                orch.apply_wanted(&module, wanted(lifecycle, server_status))
                    .await;
            }
        }
        loop {
            match events.recv().await {
                Ok(Event::ServerStatusChanged { server_id, status }) => {
                    let orch = orch.clone();
                    tokio::spawn(async move { orch.follow_server(&server_id, &status).await });
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

fn status_of(e: &bollard::errors::Error) -> Option<u16> {
    match e {
        bollard::errors::Error::DockerResponseServerError { status_code, .. } => Some(*status_code),
        _ => None,
    }
}

fn is_not_modified(e: &bollard::errors::Error) -> bool {
    status_of(e) == Some(304)
}

fn is_conflict(e: &bollard::errors::Error) -> bool {
    status_of(e) == Some(409)
}

fn is_forbidden_or_conflict(e: &bollard::errors::Error) -> bool {
    matches!(status_of(e), Some(403 | 409))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module() -> Module {
        Module {
            id: "01MODULE".into(),
            server_id: "01SERVER".into(),
            template: "bluemap".into(),
            name: "bluemap".into(),
            settings_json: "{}".into(),
            lifecycle: "always".into(),
            memory_mb: 2048,
            web_port: Some(8100),
            web_host: Some("map".into()),
            web_public: false,
            web_path: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn module_containers_are_kept_apart_from_servers() {
        let module = module();
        let template = template::find("bluemap").unwrap();
        let spec = build(&SpecInput {
            module: &module,
            slug: "smp",
            template,
            env: BTreeMap::from([("A".into(), "b".into())]),
            binds: vec!["/srv/servers/smp/world:/app/world:ro".into()],
            publish_ip: std::net::IpAddr::from([127, 0, 0, 1]),
            run_as: Some((1000, 1000)),
        });
        assert_eq!(spec.name, "nethernode-smp.bluemap");
        let labels = spec.body.labels.as_ref().unwrap();
        assert_eq!(labels[LABEL_MODULE], "01MODULE");
        assert_eq!(labels[LABEL_MODULE_SERVER], "01SERVER");
        assert!(
            !labels.contains_key(spec::LABEL_MANAGED),
            "servers' reconciliation must not see it"
        );
        assert!(!labels.contains_key(spec::LABEL_SERVER_ID));
        let host = spec.body.host_config.as_ref().unwrap();
        assert_eq!(host.network_mode.as_deref(), Some("nethernode-smp.modules"));
        assert_eq!(host.privileged, None);
        assert_eq!(
            host.security_opt.as_deref(),
            Some(&["no-new-privileges".to_owned()][..])
        );
        let web = &host.port_bindings.as_ref().unwrap()["8100/tcp"]
            .as_ref()
            .unwrap()[0];
        assert_eq!(
            (web.host_ip.as_deref(), web.host_port.as_deref()),
            (Some("127.0.0.1"), Some("8100"))
        );
        assert_eq!(spec.body.user.as_deref(), Some("1000:1000"));
        assert_eq!(host.memory, Some(2048 * 1024 * 1024));

        // The definition's hash follows its settings.
        let mut bigger = module.clone();
        bigger.memory_mb = 4096;
        let other = build(&SpecInput {
            module: &bigger,
            slug: "smp",
            template,
            env: BTreeMap::new(),
            binds: Vec::new(),
            publish_ip: std::net::IpAddr::from([127, 0, 0, 1]),
            run_as: None,
        });
        assert_ne!(spec.hash, other.hash);
    }

    #[test]
    fn modules_follow_their_lifecycle() {
        assert_eq!(wanted(Lifecycle::Always, "stopped"), Some(true));
        assert_eq!(wanted(Lifecycle::Manual, "running"), None);
        assert_eq!(wanted(Lifecycle::WithServer, "running"), Some(true));
        for down in ["stopped", "crashed", "sleeping"] {
            assert_eq!(wanted(Lifecycle::WithServer, down), Some(false), "{down}");
        }
        assert_eq!(wanted(Lifecycle::WithServer, "starting"), None);
    }

    #[test]
    fn world_folder_comes_from_server_properties() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(level_name(dir.path()), "world");
        std::fs::write(
            dir.path().join("server.properties"),
            "motd=x\nlevel-name=survival\n",
        )
        .unwrap();
        assert_eq!(level_name(dir.path()), "survival");
    }
}
