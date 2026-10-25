//! The machines that run Minecraft servers.
//!
//! The panel never reaches Docker or a server's folder directly: it asks the
//! server's [`Node`]: this machine ([`LocalNode`]), or another one through
//! its agent ([`RemoteNode`]).
//!
//! The rule that keeps that possible: everything a [`Node`] method takes or
//! returns is plain data (server IDs, paths inside a server's folder, bytes,
//! records) that could just as well cross the network.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nn_events::EventBus;
use nn_files::{FileError, ServerFiles};

use nn_orchestrator::modules::{Lifecycle, ModuleState};
use nn_orchestrator::{
    BeforeStart, CreateServer, HostInfo, Orchestrator, OrchestratorError, Players, StatSample,
};
use nn_players::{MgmtClient, MgmtError};
use nn_store::{LOCAL_NODE, Module, Server, ServerSettings, Store};
use tokio::sync::broadcast;

use crate::protocol::Request;

pub mod agent;
mod backups;
mod folder;
pub mod jobs;
mod moving;
pub mod protocol;
mod remote;
pub mod update;

pub use backups::{Archive, Restored};
pub use folder::{FileReader, Folder};
pub use remote::{RemoteHost, RemoteNode};

/// Why a node operation failed.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// Docker isn't reachable on the node, so servers can't be managed.
    #[error(
        "Docker isn't available ({0}). Server management is disabled until NetherNode can reach Docker."
    )]
    Unavailable(Arc<str>),
    /// The operation itself failed.
    #[error(transparent)]
    Orchestrator(#[from] OrchestratorError),
    /// The node's data folder can't be reached.
    #[error("the data directory isn't available, so server files can't be reached")]
    NoDataFolder,
    /// A file operation failed.
    #[error(transparent)]
    File(#[from] FileError),
    /// The node's agent isn't connected.
    #[error("{0} is offline. Its servers can be managed again once its agent reconnects.")]
    Offline(String),
    /// The node's agent runs a NetherNode version the panel can't manage.
    #[error(
        "{0} runs a different NetherNode version than the panel. Update it to manage its servers."
    )]
    NeedsUpdate(String),
    /// The agent tried and failed.
    #[error(transparent)]
    Remote(#[from] protocol::RemoteError),
    /// Something servers on other machines can't do yet (the message says
    /// what).
    #[error("{0}")]
    NotYet(&'static str),
}

/// Result alias.
pub type Result<T, E = NodeError> = std::result::Result<T, E>;

/// A server's console: the lines so far, then each new line.
pub struct LogSubscription {
    /// Recent output, oldest first.
    pub backlog: Vec<Arc<str>>,
    /// New lines as they're printed.
    pub lines: broadcast::Receiver<Arc<str>>,
}

/// This machine: its Docker daemon (if reachable) and its data folder.
pub struct LocalNode {
    orchestrator: Option<Arc<Orchestrator>>,
    unavailable_reason: Arc<str>,
    servers_dir: Option<PathBuf>,
    events: EventBus,
}

impl LocalNode {
    /// A node with Docker. File jobs report progress on `events`.
    pub fn new(orchestrator: Arc<Orchestrator>, servers_dir: PathBuf, events: EventBus) -> Self {
        Self {
            orchestrator: Some(orchestrator),
            unavailable_reason: Arc::from("not connected"),
            servers_dir: Some(servers_dir),
            events,
        }
    }

    /// A node whose Docker can't be reached, for `reason`. Server files stay
    /// reachable when `servers_dir` is known.
    pub fn without_docker(
        reason: impl Into<Arc<str>>,
        servers_dir: Option<PathBuf>,
        events: EventBus,
    ) -> Self {
        Self {
            orchestrator: None,
            unavailable_reason: reason.into(),
            servers_dir,
            events,
        }
    }

    fn orchestrator(&self) -> Result<&Arc<Orchestrator>> {
        self.orchestrator
            .as_ref()
            .ok_or_else(|| NodeError::Unavailable(self.unavailable_reason.clone()))
    }

    async fn open_folder(&self, server: &Server) -> Result<Folder> {
        let dir = self.servers_dir.as_ref().ok_or(NodeError::NoDataFolder)?;
        // Slugs are checked when servers are created; check again since this
        // becomes a path.
        if nn_files::validate_name(&server.slug).is_err() || server.slug.starts_with('.') {
            return Err(FileError::Invalid("invalid server folder".into()).into());
        }
        let root = dir.join(&server.slug);
        let opened = tokio::task::spawn_blocking(move || match ServerFiles::open(&root) {
            Err(FileError::NotFound(_)) => Err(FileError::Invalid(
                "this server's folder doesn't exist yet; start the server once to create it".into(),
            )),
            other => other,
        })
        .await
        .map_err(|e| FileError::Io(std::io::Error::other(e.to_string())))??;
        Ok(Folder::local(
            server.id.clone(),
            self.events.clone(),
            opened,
        ))
    }

    /// Where uploads for this machine's servers are put together.
    pub(crate) fn upload_dir(&self) -> Option<PathBuf> {
        let dir = self
            .servers_dir
            .as_ref()?
            .parent()?
            .join("tmp")
            .join("uploads");
        std::fs::create_dir_all(&dir).ok()?;
        Some(dir)
    }

    /// Work to do just before any server on this machine starts.
    pub fn set_before_start(&self, hook: Arc<dyn BeforeStart>) {
        if let Some(orchestrator) = &self.orchestrator {
            orchestrator.set_before_start(hook);
        }
    }
}

const MODULES_NOT_YET: &str = "Modules run on the panel's own machine only for now.";

/// A machine that runs servers.
#[derive(Clone)]
pub enum Node {
    /// This machine.
    Local(Arc<LocalNode>),
    /// Another machine, through its agent.
    Remote(Arc<RemoteNode>),
}

impl Node {
    /// This machine, or `not_yet` for what other machines can't do yet.
    fn as_local(&self, not_yet: &'static str) -> Result<&LocalNode> {
        match self {
            Self::Local(node) => Ok(node),
            Self::Remote(_) => Err(NodeError::NotYet(not_yet)),
        }
    }

    /// Whether this is the panel's own machine.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Why servers can't be managed on this node right now, if they can't.
    pub fn unavailable(&self) -> Option<NodeError> {
        match self {
            Self::Local(node) => node.orchestrator().err(),
            Self::Remote(node) if !node.is_online() => Some(node.offline()),
            Self::Remote(node) => match node.host().map(|h| h.docker) {
                Some(Err(reason)) => Some(NodeError::Unavailable(reason.into())),
                _ => None,
            },
        }
    }

    // ----- lifecycle -----

    /// Creates a server and installs it in the background.
    pub async fn create(&self, input: CreateServer) -> Result<Server> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.create(input).await?),
            Self::Remote(node) => {
                node.call(Request::Create {
                    input: Box::new(input),
                })
                .await
            }
        }
    }

    /// Starts a stopped server.
    pub async fn start(&self, id: &str) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.start(id).await?),
            Self::Remote(node) => {
                node.before_start(id).await;
                node.call(Request::Start { id: id.into() }).await
            }
        }
    }

    /// Starts a sleeping server for a player who's joining.
    pub async fn wake(&self, id: &str) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.wake(id).await?),
            Self::Remote(node) => {
                node.before_start(id).await;
                node.call(Request::Wake { id: id.into() }).await
            }
        }
    }

    /// Stops a server, or puts it to sleep.
    pub async fn stop(&self, id: &str, sleep: bool) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.stop(id, sleep).await?),
            Self::Remote(node) => {
                node.call(Request::Stop {
                    id: id.into(),
                    sleep,
                })
                .await
            }
        }
    }

    /// Stops and starts a server.
    pub async fn restart(&self, id: &str) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.restart(id).await?),
            Self::Remote(node) => node.call(Request::Restart { id: id.into() }).await,
        }
    }

    /// Stops a server at once, without saving.
    pub async fn kill(&self, id: &str) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.kill(id).await?),
            Self::Remote(node) => node.call(Request::Kill { id: id.into() }).await,
        }
    }

    /// Changes a server's settings. Returns the server and whether it needs
    /// a restart for them to take effect.
    pub async fn update(&self, id: &str, settings: ServerSettings) -> Result<(Server, bool)> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.update(id, settings).await?),
            Self::Remote(node) => {
                node.call(Request::Update {
                    id: id.into(),
                    settings,
                })
                .await
            }
        }
    }

    /// Deletes a server, and with `delete_data` its files.
    pub async fn delete(&self, id: &str, delete_data: bool) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.delete(id, delete_data).await?),
            Self::Remote(node) => {
                node.call(Request::Delete {
                    id: id.into(),
                    delete_data,
                })
                .await
            }
        }
    }

    // ----- status -----

    /// Whether the server's container is running.
    pub async fn container_running(&self, id: &str) -> Result<bool> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.container_running(id).await?),
            Self::Remote(node) => node.call(Request::ContainerRunning { id: id.into() }).await,
        }
    }

    /// Whether the server's settings changed since its container was made.
    pub async fn restart_required(&self, server: &Server) -> bool {
        match self {
            Self::Local(node) => match node.orchestrator() {
                Ok(orchestrator) => orchestrator.restart_required(server).await,
                Err(_) => false,
            },
            Self::Remote(node) => node
                .call(Request::RestartRequired {
                    id: server.id.clone(),
                })
                .await
                .unwrap_or(false),
        }
    }

    /// Players from the last status check, while running.
    pub fn players(&self, id: &str) -> Option<Players> {
        match self {
            Self::Local(node) => node.orchestrator().ok()?.players(id),
            Self::Remote(node) => node.players(id),
        }
    }

    /// Up to an hour of resource samples, oldest first.
    pub async fn stats_history(&self, id: &str) -> Vec<StatSample> {
        match self {
            Self::Local(node) => node
                .orchestrator()
                .map(|o| o.stats_history(id))
                .unwrap_or_default(),
            Self::Remote(node) => match node.call(Request::StatsHistory { id: id.into() }).await {
                Ok(samples) => samples,
                Err(e) => {
                    tracing::warn!(server = %id, error = %e, "couldn't get a server's stats from its machine");
                    Vec::new()
                }
            },
        }
    }

    /// The newest resource sample.
    pub fn latest_stats(&self, id: &str) -> Option<StatSample> {
        match self {
            Self::Local(node) => node.orchestrator().ok()?.latest_stats(id),
            Self::Remote(node) => node.latest_stats(id),
        }
    }

    /// Whether the server paused itself because nobody's online, as of its
    /// newest sample (meaningful only while it runs).
    pub fn paused(&self, id: &str) -> bool {
        self.latest_stats(id).is_some_and(|s| s.paused)
    }

    /// How much space a server's folder and worlds take, if that can be
    /// measured.
    pub async fn folder_usage(&self, id: &str) -> Option<nn_orchestrator::FolderUsage> {
        match self {
            Self::Local(node) => node.orchestrator().ok()?.folder_usage(id).await.ok(),
            Self::Remote(node) => node
                .call::<Option<nn_orchestrator::FolderUsage>>(Request::FolderUsage {
                    id: id.into(),
                })
                .await
                .ok()
                .flatten(),
        }
    }

    /// Facts about the machine's Docker host.
    pub async fn host_info(&self) -> Result<HostInfo> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.host_info().await?),
            Self::Remote(node) => {
                if !node.is_online() {
                    return Err(node.offline());
                }
                match node.host().map(|h| h.docker) {
                    Some(Ok(info)) => Ok(info),
                    Some(Err(reason)) => Err(NodeError::Unavailable(reason.into())),
                    None => Err(NodeError::Unavailable("not reported yet".into())),
                }
            }
        }
    }

    /// Total and free bytes on the disk holding servers and backups.
    pub fn disk_space(&self) -> Option<(u64, u64)> {
        match self {
            Self::Local(node) => {
                let dir = node.servers_dir.as_deref()?;
                nn_uploads::disk_space(dir.parent().unwrap_or(dir))
            }
            Self::Remote(node) => node.host()?.disk,
        }
    }

    // ----- files -----

    /// The server's folder. Fails if it doesn't exist yet (servers create it
    /// on their first start).
    pub async fn folder(&self, server: &Server) -> Result<Folder> {
        match self {
            Self::Local(node) => node.open_folder(server).await,
            Self::Remote(node) => Ok(Folder::remote(server.id.clone(), node.clone())),
        }
    }

    // ----- console and protocols -----

    /// The server's console output.
    pub async fn subscribe_logs(&self, id: &str) -> Result<LogSubscription> {
        match self {
            Self::Local(node) => {
                let (backlog, lines) = node.orchestrator()?.subscribe_logs(id);
                Ok(LogSubscription { backlog, lines })
            }
            Self::Remote(node) => node.logs(id).await,
        }
    }

    /// Runs a console command (over RCON) and returns its output.
    pub async fn command(&self, id: &str, command: &str) -> Result<String> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.command(id, command).await?),
            Self::Remote(node) => {
                node.call(Request::Command {
                    id: id.into(),
                    command: command.into(),
                })
                .await
            }
        }
    }

    /// A connection to the server's management protocol, or `None` if it
    /// hasn't started with one (or, for now, runs on another machine:
    /// commands go over RCON instead).
    pub async fn management(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Option<std::result::Result<MgmtClient, MgmtError>> {
        let local = match self {
            Self::Remote(remote) => {
                // Through the agent, which dials the port on its machine.
                let opened = tokio::time::timeout(timeout, remote.management(id)).await;
                let (stream, secret) = opened.ok()?.ok()??;
                let stream: Box<dyn nn_players::MgmtStream> = Box::new(stream);
                return Some(MgmtClient::connect_over(stream, "127.0.0.1", &secret, timeout).await);
            }
            Self::Local(local) => local,
        };
        let orchestrator = local.orchestrator().ok()?;
        let (addr, secret) = orchestrator.management(id).await.ok().flatten()?;
        Some(MgmtClient::connect(addr, &secret, timeout).await)
    }

    /// A connection to the server's game port, for the proxy. An error of
    /// kind `NotConnected` means the node is offline.
    pub async fn connect(&self, id: &str) -> std::io::Result<Box<dyn nn_proxy::Tunnel>> {
        match self {
            // The proxy dials servers on this machine itself.
            Self::Local(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "servers on this machine are reached directly",
            )),
            Self::Remote(node) => node.connect(id).await,
        }
    }

    /// A connection to a Bedrock server's game port (UDP), for the proxy.
    /// An error of kind `NotConnected` means the node is offline.
    pub async fn connect_udp(&self, id: &str) -> std::io::Result<Box<dyn nn_proxy::Tunnel>> {
        match self {
            Self::Local(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "servers on this machine are reached directly",
            )),
            Self::Remote(node) => node.connect_udp(id).await,
        }
    }

    // ----- modules -----

    /// A server's modules and what each is doing.
    pub async fn modules(&self, server_id: &str) -> Result<Vec<(Module, ModuleState)>> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.modules(server_id).await?),
            Self::Remote(node) => {
                node.call(Request::Modules {
                    server_id: server_id.into(),
                })
                .await
            }
        }
    }

    /// Adds a module to a server (see [`Orchestrator::add_module`]).
    pub async fn add_module(
        &self,
        server_id: &str,
        template: &str,
        name: Option<&str>,
        settings: &serde_json::Map<String, serde_json::Value>,
        lifecycle: Option<Lifecycle>,
        memory_mb: Option<i64>,
    ) -> Result<Module> {
        match self {
            Self::Local(node) => Ok(node
                .orchestrator()?
                .add_module(server_id, template, name, settings, lifecycle, memory_mb)
                .await?),
            Self::Remote(node) => {
                node.call(Request::AddModule {
                    server_id: server_id.into(),
                    template: template.into(),
                    name: name.map(str::to_owned),
                    settings: settings.clone(),
                    lifecycle,
                    memory_mb,
                })
                .await
            }
        }
    }

    /// Changes a module's settings; a running one restarts with them.
    pub async fn update_module(
        &self,
        id: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
        lifecycle: Lifecycle,
        memory_mb: i64,
    ) -> Result<Module> {
        match self {
            Self::Local(node) => Ok(node
                .orchestrator()?
                .update_module(id, settings, lifecycle, memory_mb)
                .await?),
            Self::Remote(node) => {
                node.call(Request::UpdateModule {
                    id: id.into(),
                    settings: settings.clone(),
                    lifecycle,
                    memory_mb,
                })
                .await
            }
        }
    }

    /// Starts a module.
    pub async fn start_module(&self, id: &str) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.start_module(id).await?),
            Self::Remote(node) => node.call(Request::StartModule { id: id.into() }).await,
        }
    }

    /// Stops a module.
    pub async fn stop_module(&self, id: &str) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.stop_module(id).await?),
            Self::Remote(node) => node.call(Request::StopModule { id: id.into() }).await,
        }
    }

    /// Removes a module, and with `delete_files` its folder.
    pub async fn remove_module(&self, id: &str, delete_files: bool) -> Result<()> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.remove_module(id, delete_files).await?),
            Self::Remote(node) => {
                node.call(Request::RemoveModule {
                    id: id.into(),
                    delete_files,
                })
                .await
            }
        }
    }

    /// The last lines a module printed.
    pub async fn module_logs(&self, id: &str, lines: usize) -> Result<Vec<String>> {
        match self {
            Self::Local(node) => Ok(node.orchestrator()?.module_logs(id, lines).await?),
            Self::Remote(node) => {
                node.call(Request::ModuleLogs {
                    id: id.into(),
                    lines,
                })
                .await
            }
        }
    }

    /// A connection to a module's web page on another machine, through its
    /// agent (modules on this machine are reached at
    /// [`Self::module_address`]).
    pub async fn module_connect(&self, id: &str) -> Result<Box<dyn nn_proxy::Tunnel>> {
        match self {
            Self::Local(_) => Err(NodeError::NotYet(
                "modules on this machine are reached directly",
            )),
            Self::Remote(node) => node.module_connect(id).await,
        }
    }

    /// Runs the Cloudflare Tunnel connector (this machine only).
    pub async fn run_tunnel(&self, token: &str) -> Result<()> {
        let local = self.as_local("The Cloudflare Tunnel runs on the panel's own machine.")?;
        Ok(local.orchestrator()?.run_tunnel(token).await?)
    }

    /// Removes the Cloudflare Tunnel connector.
    pub async fn stop_tunnel(&self) -> Result<()> {
        let local = self.as_local("The Cloudflare Tunnel runs on the panel's own machine.")?;
        Ok(local.orchestrator()?.stop_tunnel().await?)
    }

    /// What the Cloudflare Tunnel connector is doing.
    pub async fn tunnel_state(&self) -> Result<ModuleState> {
        let local = self.as_local("The Cloudflare Tunnel runs on the panel's own machine.")?;
        Ok(local.orchestrator()?.tunnel_state().await)
    }

    /// Where a module's web page is published (`http://<ip>:<port>`).
    pub fn module_address(&self, port: i64) -> Option<String> {
        let local = self.as_local(MODULES_NOT_YET).ok()?;
        let ip = local.orchestrator().ok()?.backend_ip();
        Some(format!(
            "http://{}",
            std::net::SocketAddr::new(ip, u16::try_from(port).ok()?)
        ))
    }

    // ----- settings pushed from the panel -----

    /// The CurseForge API key, for installing CurseForge modpacks.
    pub fn set_curseforge_key(&self, key: Option<String>) {
        match self {
            Self::Local(node) => {
                if let Ok(orchestrator) = node.orchestrator() {
                    orchestrator.set_curseforge_key(key);
                }
            }
            Self::Remote(node) => {
                let node = node.clone();
                tokio::spawn(async move {
                    if let Err(e) = node.call::<()>(Request::SetCurseforgeKey { key }).await {
                        tracing::debug!(node = %node.id(), error = %e, "couldn't pass on the CurseForge key");
                    }
                });
            }
        }
    }
}

/// Every node, and which one each server is on.
#[derive(Clone)]
pub struct Nodes {
    local: Node,
    local_node: Arc<LocalNode>,
    remotes: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<RemoteNode>>>>,
    store: Option<Store>,
    moving: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    before_start: Arc<std::sync::OnceLock<StartHook>>,
}

/// Work the panel does before a server on another machine starts.
pub(crate) struct StartHook {
    pub(crate) store: Store,
    pub(crate) hook: Arc<dyn BeforeStart>,
}

/// A server being moved between machines; the move ends when this drops.
pub struct Moving {
    id: String,
    moving: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl Drop for Moving {
    fn drop(&mut self) {
        self.moving
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

impl Nodes {
    /// This machine, with no others yet. `store` says which node each
    /// server is on.
    pub fn new(local: LocalNode, store: Option<Store>) -> Self {
        let local_node = Arc::new(local);
        Self {
            local: Node::Local(local_node.clone()),
            local_node,
            remotes: Arc::default(),
            store,
            moving: Arc::default(),
            before_start: Arc::default(),
        }
    }

    /// Sets the work to do before each server starts, on every machine:
    /// this one's orchestrator runs it itself; for other machines the panel
    /// runs it (over the link) before asking their agent to start the
    /// server. Only the first call counts.
    pub fn set_before_start(&self, hook: Arc<dyn BeforeStart>) {
        self.local_node.set_before_start(hook.clone());
        if let Some(store) = &self.store {
            let _ = self.before_start.set(StartHook {
                store: store.clone(),
                hook,
            });
        }
    }

    /// Marks a server as moving between machines, unless it already is.
    /// Meanwhile nothing else may change it, and agents' reports about it
    /// are ignored.
    pub fn begin_move(&self, id: &str) -> Option<Moving> {
        let mut moving = self.moving.lock().unwrap_or_else(|e| e.into_inner());
        moving.insert(id.to_owned()).then(|| Moving {
            id: id.to_owned(),
            moving: self.moving.clone(),
        })
    }

    /// Whether a server is moving between machines.
    pub fn is_moving(&self, id: &str) -> bool {
        self.moving
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(id)
    }

    /// This machine.
    pub fn local(&self) -> &Node {
        &self.local
    }

    /// This machine's own handle, for wiring at startup.
    pub fn local_node(&self) -> &Arc<LocalNode> {
        &self.local_node
    }

    /// Another machine, by node ID.
    pub fn remote(&self, node_id: &str) -> Arc<RemoteNode> {
        self.remotes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(node_id.to_owned())
            .or_insert_with(|| Arc::new(RemoteNode::new(node_id, self.before_start.clone())))
            .clone()
    }

    /// Forgets a removed machine.
    pub fn forget(&self, node_id: &str) {
        let removed = self
            .remotes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(node_id);
        if let Some(node) = removed {
            node.detach();
        }
    }

    /// The node with this ID.
    pub fn get(&self, node_id: &str) -> Node {
        if node_id == LOCAL_NODE {
            self.local.clone()
        } else {
            Node::Remote(self.remote(node_id))
        }
    }

    /// The node `server` runs on.
    pub fn for_server(&self, server: &Server) -> Node {
        self.get(&server.node_id)
    }

    /// The node the server with `id` runs on (this machine if it's gone).
    pub async fn for_server_id(&self, id: &str) -> Node {
        let node_id = match &self.store {
            Some(store) => store
                .find_server(id)
                .await
                .ok()
                .flatten()
                .map(|s| s.node_id),
            None => None,
        };
        self.get(node_id.as_deref().unwrap_or(LOCAL_NODE))
    }

    /// Gives every node the CurseForge API key, for CurseForge modpacks.
    pub fn set_curseforge_key(&self, key: Option<String>) {
        self.local.set_curseforge_key(key.clone());
        let remotes: Vec<_> = self
            .remotes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|n| n.is_online())
            .cloned()
            .collect();
        for node in remotes {
            Node::Remote(node).set_curseforge_key(key.clone());
        }
    }
}

impl Default for Nodes {
    /// No Docker and no data folder: nothing works, and says why.
    fn default() -> Self {
        Self::new(
            LocalNode::without_docker("not connected", None, EventBus::default()),
            None,
        )
    }
}
