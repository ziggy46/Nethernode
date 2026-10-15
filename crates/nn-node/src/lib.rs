//! The machines that run Minecraft servers.
//!
//! The panel never reaches Docker or a server's folder directly: it asks the
//! server's [`Node`]. Today every node is this machine ([`LocalNode`]); a
//! remote agent becomes another kind of node without callers changing.
//!
//! The rule that keeps that possible: everything a [`Node`] method takes or
//! returns is plain data (server IDs, paths inside a server's folder, bytes,
//! records) that could just as well cross the network.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nn_events::EventBus;
use nn_files::{FileError, ServerFiles};

use nn_orchestrator::{
    BeforeStart, CreateServer, HostInfo, Orchestrator, OrchestratorError, Players, StatSample,
};
use nn_players::{MgmtClient, MgmtError};
use nn_store::{Server, ServerSettings};
use tokio::sync::broadcast;

mod backups;
mod folder;
pub mod jobs;

pub use backups::{Archive, Restored};
pub use folder::{FileReader, Folder};

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

    /// Work to do just before any server on this machine starts.
    pub fn set_before_start(&self, hook: Arc<dyn BeforeStart>) {
        if let Some(orchestrator) = &self.orchestrator {
            orchestrator.set_before_start(hook);
        }
    }
}

/// A machine that runs servers.
#[derive(Clone)]
pub enum Node {
    /// This machine.
    Local(Arc<LocalNode>),
}

impl Node {
    fn local(&self) -> &LocalNode {
        match self {
            Self::Local(node) => node,
        }
    }

    /// Why servers can't be managed on this node right now, if they can't.
    pub fn unavailable(&self) -> Option<NodeError> {
        self.local().orchestrator().err()
    }

    // ----- lifecycle -----

    /// Creates a server and installs it in the background.
    pub async fn create(&self, input: CreateServer) -> Result<Server> {
        Ok(self.local().orchestrator()?.create(input).await?)
    }

    /// Starts a stopped server.
    pub async fn start(&self, id: &str) -> Result<()> {
        Ok(self.local().orchestrator()?.start(id).await?)
    }

    /// Starts a sleeping server for a player who's joining.
    pub async fn wake(&self, id: &str) -> Result<()> {
        Ok(self.local().orchestrator()?.wake(id).await?)
    }

    /// Stops a server, or puts it to sleep.
    pub async fn stop(&self, id: &str, sleep: bool) -> Result<()> {
        Ok(self.local().orchestrator()?.stop(id, sleep).await?)
    }

    /// Stops and starts a server.
    pub async fn restart(&self, id: &str) -> Result<()> {
        Ok(self.local().orchestrator()?.restart(id).await?)
    }

    /// Stops a server at once, without saving.
    pub async fn kill(&self, id: &str) -> Result<()> {
        Ok(self.local().orchestrator()?.kill(id).await?)
    }

    /// Changes a server's settings. Returns the server and whether it needs
    /// a restart for them to take effect.
    pub async fn update(&self, id: &str, settings: ServerSettings) -> Result<(Server, bool)> {
        Ok(self.local().orchestrator()?.update(id, settings).await?)
    }

    /// Deletes a server, and with `delete_data` its files.
    pub async fn delete(&self, id: &str, delete_data: bool) -> Result<()> {
        Ok(self.local().orchestrator()?.delete(id, delete_data).await?)
    }

    // ----- status -----

    /// Whether the server's container is running.
    pub async fn container_running(&self, id: &str) -> Result<bool> {
        Ok(self.local().orchestrator()?.container_running(id).await?)
    }

    /// Whether the server's settings changed since its container was made.
    pub async fn restart_required(&self, server: &Server) -> bool {
        match self.local().orchestrator() {
            Ok(orchestrator) => orchestrator.restart_required(server).await,
            Err(_) => false,
        }
    }

    /// Players from the last status check, while running.
    pub fn players(&self, id: &str) -> Option<Players> {
        self.local().orchestrator().ok()?.players(id)
    }

    /// Up to an hour of resource samples, oldest first.
    pub fn stats_history(&self, id: &str) -> Vec<StatSample> {
        self.local()
            .orchestrator()
            .map(|o| o.stats_history(id))
            .unwrap_or_default()
    }

    /// The newest resource sample.
    pub fn latest_stats(&self, id: &str) -> Option<StatSample> {
        self.local().orchestrator().ok()?.latest_stats(id)
    }

    /// Facts about the machine's Docker host.
    pub async fn host_info(&self) -> Result<HostInfo> {
        Ok(self.local().orchestrator()?.host_info().await?)
    }

    /// Total and free bytes on the disk holding servers and backups.
    pub fn disk_space(&self) -> Option<(u64, u64)> {
        let dir = self.local().servers_dir.as_deref()?;
        nn_uploads::disk_space(dir.parent().unwrap_or(dir))
    }

    // ----- files -----

    /// The server's folder. Fails if it doesn't exist yet (servers create it
    /// on their first start).
    pub async fn folder(&self, server: &Server) -> Result<Folder> {
        self.local().open_folder(server).await
    }

    // ----- console and protocols -----

    /// The server's console output.
    pub fn subscribe_logs(&self, id: &str) -> Result<LogSubscription> {
        let (backlog, lines) = self.local().orchestrator()?.subscribe_logs(id);
        Ok(LogSubscription { backlog, lines })
    }

    /// Runs a console command (over RCON) and returns its output.
    pub async fn command(&self, id: &str, command: &str) -> Result<String> {
        Ok(self.local().orchestrator()?.command(id, command).await?)
    }

    /// A connection to the server's management protocol, or `None` if it
    /// hasn't started with one.
    pub async fn management(
        &self,
        id: &str,
        timeout: Duration,
    ) -> Option<std::result::Result<MgmtClient, MgmtError>> {
        let orchestrator = self.local().orchestrator().ok()?;
        let (addr, secret) = orchestrator.management(id).await.ok().flatten()?;
        Some(MgmtClient::connect(addr, &secret, timeout).await)
    }

    // ----- settings pushed from the panel -----

    /// The CurseForge API key, for installing CurseForge modpacks.
    pub fn set_curseforge_key(&self, key: Option<String>) {
        if let Ok(orchestrator) = self.local().orchestrator() {
            orchestrator.set_curseforge_key(key);
        }
    }
}

/// Every node, and which one each server is on.
#[derive(Clone)]
pub struct Nodes {
    local: Node,
    local_node: Arc<LocalNode>,
}

impl Nodes {
    /// The panel's own machine as the only node.
    pub fn new(local: LocalNode) -> Self {
        let local_node = Arc::new(local);
        Self {
            local: Node::Local(local_node.clone()),
            local_node,
        }
    }

    /// This machine.
    pub fn local(&self) -> &Node {
        &self.local
    }

    /// This machine's own handle, for wiring at startup.
    pub fn local_node(&self) -> &Arc<LocalNode> {
        &self.local_node
    }

    /// The node `server` runs on.
    pub fn for_server(&self, _server: &Server) -> &Node {
        &self.local
    }

    /// Gives every node the CurseForge API key, for CurseForge modpacks.
    pub fn set_curseforge_key(&self, key: Option<String>) {
        self.local.set_curseforge_key(key);
    }

    /// The node the server with `id` runs on.
    pub fn for_server_id(&self, _id: &str) -> &Node {
        &self.local
    }
}

impl Default for Nodes {
    /// No Docker and no data folder: nothing works, and says why.
    fn default() -> Self {
        Self::new(LocalNode::without_docker(
            "not connected",
            None,
            EventBus::default(),
        ))
    }
}
