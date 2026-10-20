//! The orchestrator and its lifecycle operations.
//!
//! State machine (persisted in `servers.status`):
//!
//! ```text
//! installing ─► stopped ─► starting ─► running ─► stopping ─► stopped
//!                  ▲  ▲         │          │  │
//!                  │  └─ crashed ◄─────────┘  └─► stopping ─► sleeping
//!                  └──────────────── (start / wake) ◄──────────────┘
//! ```
//!
//! `running` is recorded when a status ping succeeds, not when the
//! container starts. Docker events are the source of truth for exits;
//! NetherNode never relies on Docker's restart policy.

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bollard::Docker;
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, KillContainerOptionsBuilder,
    RemoveContainerOptionsBuilder, StartContainerOptions, StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use nn_auth::SecretBox;
use nn_events::{Event, EventBus};
use nn_store::{Server, ServerSettings, Store, now_ms};
use rand::Rng;

use crate::error::{OrchestratorError, Result};
use crate::runtime::{FolderUsage, Players, Runtime, StatSample};
use crate::spec::{self, SpecInput};
use crate::{java, ports};

/// Settings for the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// Absolute directory holding each server's data (`<data>/servers`).
    pub servers_dir: PathBuf,
    /// Host ports for game and RCON bindings (127.0.0.1 only).
    pub port_range: RangeInclusive<u16>,
    /// Container memory beyond the Java heap, in percent.
    pub memory_overhead_percent: u32,
    /// Container memory beyond the Java heap, at least, in MiB.
    pub memory_overhead_min_mb: u32,
    /// How long a graceful stop may take before Docker stops the container.
    pub stop_timeout: Duration,
    /// Minutes with no players before a sleep-enabled server sleeps, when
    /// the server doesn't set its own.
    pub default_idle_minutes: i64,
    /// How often running servers are pinged for player counts.
    pub player_poll: Duration,
    /// User and group servers run as. See [`default_run_as`].
    pub run_as: Option<(u32, u32)>,
    /// Where `servers_dir` is on the Docker host, for bind mounts. `None`
    /// finds it at connect time (see [`crate::host_path`]).
    pub host_servers_dir: Option<PathBuf>,
    /// Address server ports are published on, and dialed at: loopback when
    /// NetherNode shares the host's network, the Docker bridge gateway
    /// (e.g. 172.17.0.1) when it runs in a bridge-networked container.
    pub backend_ip: std::net::IpAddr,
}

impl OrchestratorConfig {
    /// Defaults from the build guide, with data under `servers_dir`.
    pub fn new(servers_dir: PathBuf) -> Self {
        Self {
            servers_dir,
            port_range: 30000..=30999,
            memory_overhead_percent: 25,
            memory_overhead_min_mb: 1024,
            stop_timeout: Duration::from_secs(60),
            default_idle_minutes: 10,
            player_poll: Duration::from_secs(15),
            run_as: default_run_as(),
            host_servers_dir: None,
            backend_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        }
    }
}

/// The server image chowns its data directory to the user it runs as
/// (uid 1000 by default). On Linux that has to be NetherNode's own user, or
/// NetherNode can read server files but not change or delete them. Root can
/// manage any files, and Minecraft shouldn't run as root, so a root
/// NetherNode keeps the image default. Docker Desktop and OrbStack on macOS
/// map bind-mount ownership to the host user, so nothing is needed there.
pub fn default_run_as() -> Option<(u32, u32)> {
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{getegid, geteuid};
        let uid = geteuid();
        (!uid.is_root()).then(|| (uid.as_raw(), getegid().as_raw()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Fields for creating a server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CreateServer {
    /// Display name.
    pub name: String,
    /// Loader id (`vanilla`, `paper`, `forge`, …).
    pub loader: String,
    /// Minecraft version or `LATEST`.
    pub mc_version: String,
    /// Loader/build pin.
    pub loader_version: Option<String>,
    /// Java heap in MiB.
    pub memory_mb: i64,
    /// Card artwork.
    pub card_theme: String,
    /// The admin ticked "I accept the Minecraft EULA".
    pub eula_accepted: bool,
    /// Sleep when empty, wake on connect.
    pub sleep_when_empty: bool,
    /// Start as soon as installation finishes.
    pub start_after_create: bool,
    /// The modpack to install, for the `modrinth` and `curseforge` loaders.
    pub modpack: Option<spec::Modpack>,
    /// The slug to use, when the panel chose it (servers on other machines
    /// get slugs unique across the panel). Otherwise one is made from the
    /// name.
    #[serde(default)]
    pub slug: Option<String>,
    /// Ports not to use, because servers on other machines have them (the
    /// panel keeps ports unique across machines).
    #[serde(default)]
    pub avoid_ports: Vec<i64>,
}

/// Docker host facts for the UI.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct HostInfo {
    /// Docker host name.
    pub hostname: String,
    /// Host OS description.
    pub operating_system: String,
    /// Docker engine version.
    pub docker_version: String,
    /// Logical CPUs.
    pub cpus: u32,
    /// Total memory in bytes.
    pub memory_bytes: u64,
}

/// Manages Minecraft containers. Share it as `Arc<Orchestrator>`.
pub struct Orchestrator {
    pub(crate) docker: Docker,
    pub(crate) store: Store,
    pub(crate) events: EventBus,
    pub(crate) secrets: Arc<SecretBox>,
    pub(crate) config: OrchestratorConfig,
    runtimes: Mutex<HashMap<String, Arc<Runtime>>>,
    create_lock: tokio::sync::Mutex<()>,
    before_start: std::sync::OnceLock<Arc<dyn BeforeStart>>,
    /// The admin's CurseForge API key, for CurseForge modpacks.
    curseforge_key: std::sync::RwLock<Option<String>>,
}

/// Work done just before a server's container starts, while the server is
/// still stopped, so files written here are what it loads (bans that apply
/// to every server, for example).
pub trait BeforeStart: Send + Sync + 'static {
    /// Runs before `server` starts. Failures are the hook's to report.
    fn before_start<'a>(
        &'a self,
        server: &'a Server,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

const NAME_MAX: usize = 40;
const MEMORY_RANGE: RangeInclusive<i64> = 512..=65_536;
const THEMES: &[&str] = &["nether", "overworld", "end", "sky"];

impl Orchestrator {
    /// Connects to Docker at `socket` (e.g. `unix:///var/run/docker.sock`)
    /// and checks that it answers.
    pub async fn connect(
        socket: &str,
        store: Store,
        events: EventBus,
        secrets: Arc<SecretBox>,
        mut config: OrchestratorConfig,
    ) -> Result<Arc<Self>> {
        let docker = Docker::connect_with_socket(socket, 120, bollard::API_DEFAULT_VERSION)?;
        docker.ping().await?;
        std::fs::create_dir_all(&config.servers_dir)?;
        if config.host_servers_dir.is_none() {
            match crate::host_path::detect(&docker, &config.servers_dir).await {
                crate::host_path::HostPath::Same => {}
                crate::host_path::HostPath::Translated(host) => {
                    tracing::info!(
                        inside = %config.servers_dir.display(),
                        host = %host.display(),
                        "running in a container; server folders are mounted from the host path"
                    );
                    config.host_servers_dir = Some(host);
                }
                crate::host_path::HostPath::Unknown(why) => tracing::warn!(
                    reason = %why,
                    path = %config.servers_dir.display(),
                    "running in a container but couldn't find the data folder's host path; servers \
                     will only start if the host path is the same (set docker.host_data_dir to fix)"
                ),
            }
        }
        Ok(Arc::new(Self {
            docker,
            store,
            events,
            secrets,
            config,
            runtimes: Mutex::new(HashMap::new()),
            create_lock: tokio::sync::Mutex::new(()),
            before_start: std::sync::OnceLock::new(),
            curseforge_key: std::sync::RwLock::new(None),
        }))
    }

    /// Sets (or clears) the CurseForge API key given to CurseForge modpack
    /// servers. Takes effect when each one next starts.
    pub fn set_curseforge_key(&self, key: Option<String>) {
        *self
            .curseforge_key
            .write()
            .unwrap_or_else(|e| e.into_inner()) = key.filter(|k| !k.is_empty());
    }

    /// Runs `work` while holding the server's lifecycle lock: nothing can
    /// start, stop, or wake it meanwhile (a player connecting waits). `work`
    /// gets the server as it is once the lock is held. Console commands
    /// still work, so a running server can be told to save first.
    pub async fn locked<T>(&self, id: &str, work: impl AsyncFnOnce(Server) -> T) -> Result<T> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        let server = self.load(id).await?;
        Ok(work(server).await)
    }

    /// The Docker connection.
    pub fn docker(&self) -> &Docker {
        &self.docker
    }

    /// Where servers' folders are.
    pub fn servers_dir(&self) -> &std::path::Path {
        &self.config.servers_dir
    }

    /// Where server ports are published and dialed.
    pub fn backend_ip(&self) -> std::net::IpAddr {
        self.config.backend_ip
    }

    pub(crate) fn runtime(&self, id: &str) -> Arc<Runtime> {
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Runtime::new()))
            .clone()
    }

    pub(crate) async fn load(&self, id: &str) -> Result<Server> {
        self.store
            .find_server(id)
            .await?
            .ok_or(OrchestratorError::NotFound)
    }

    /// Records a status change and announces it.
    pub(crate) async fn set_status(&self, id: &str, status: &str) {
        match self.store.set_server_status(id, status).await {
            Ok(true) => {
                tracing::info!(server = %id, %status, "server status changed");
                self.events.publish(Event::ServerStatusChanged {
                    server_id: id.to_owned(),
                    status: status.to_owned(),
                });
            }
            Ok(false) => {}
            Err(error) => tracing::error!(server = %id, %error, "failed to record server status"),
        }
    }

    pub(crate) fn report_failure(&self, id: &str, operation: &str, error: &OrchestratorError) {
        tracing::warn!(server = %id, operation, %error, "server operation failed");
        self.events.publish(Event::ServerOperationFailed {
            server_id: id.to_owned(),
            operation: operation.to_owned(),
            message: error.to_string(),
        });
    }

    // ----- queries -----

    /// One of this machine's servers.
    pub async fn server(&self, id: &str) -> Result<Server> {
        self.load(id).await
    }

    /// This machine's servers.
    pub async fn servers(&self) -> Result<Vec<Server>> {
        Ok(self.store.list_local_servers().await?)
    }

    /// Players from the last status ping, while running.
    pub fn players(&self, id: &str) -> Option<Players> {
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .and_then(|rt| rt.state().players)
    }

    /// How much space a server's folder and worlds take. Measured at most
    /// every few minutes, since big folders take a while to walk.
    pub async fn folder_usage(&self, id: &str) -> Result<FolderUsage> {
        const FRESH: Duration = Duration::from_secs(5 * 60);
        let rt = self.runtime(id);
        if let Some((at, usage)) = *rt.usage.lock().unwrap_or_else(|e| e.into_inner())
            && at.elapsed() < FRESH
        {
            return Ok(usage);
        }
        let server = self.load(id).await?;
        let dir = self.server_dir(&server.slug)?;
        let usage = tokio::task::spawn_blocking(move || measure_folder(&dir))
            .await
            .map_err(std::io::Error::other)?;
        *rt.usage.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((std::time::Instant::now(), usage));
        Ok(usage)
    }

    /// Up to an hour of resource samples, oldest first.
    pub fn stats_history(&self, id: &str) -> Vec<StatSample> {
        let rt = self
            .runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned();
        rt.map(|rt| {
            rt.stats
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .snapshot()
        })
        .unwrap_or_default()
    }

    /// The newest resource sample.
    pub fn latest_stats(&self, id: &str) -> Option<StatSample> {
        let rt = self
            .runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()?;
        let stats = rt.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.last().copied()
    }

    /// Console backlog plus a live receiver.
    pub fn subscribe_logs(
        &self,
        id: &str,
    ) -> (Vec<Arc<str>>, tokio::sync::broadcast::Receiver<Arc<str>>) {
        self.runtime(id).logs.subscribe()
    }

    /// Facts about the Docker host.
    pub async fn host_info(&self) -> Result<HostInfo> {
        let info = self.docker.info().await?;
        Ok(HostInfo {
            hostname: info.name.unwrap_or_default(),
            operating_system: info.operating_system.unwrap_or_default(),
            docker_version: info.server_version.unwrap_or_default(),
            cpus: info.ncpu.and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
            memory_bytes: info
                .mem_total
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
        })
    }

    /// Whether the server's container definition differs from what its
    /// current settings would produce (so the next start recreates it).
    pub async fn restart_required(&self, server: &Server) -> bool {
        let Some(container_id) = &server.container_id else {
            return false;
        };
        let Ok(spec) = self.spec(server) else {
            return false;
        };
        match self.docker.inspect_container(container_id, None).await {
            Ok(c) => {
                let running = c.state.and_then(|s| s.running).unwrap_or(false);
                let hash = c
                    .config
                    .and_then(|c| c.labels)
                    .and_then(|l| l.get(spec::LABEL_SPEC_HASH).cloned());
                running && hash.as_deref() != Some(spec.hash.as_str())
            }
            Err(_) => false,
        }
    }

    // ----- create -----

    /// Validates input, allocates ports, records the server as
    /// `installing`, and finishes installation (image pull, container
    /// creation) in the background. Returns the new server immediately.
    pub async fn create(self: &Arc<Self>, input: CreateServer) -> Result<Server> {
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > NAME_MAX {
            return Err(OrchestratorError::InvalidInput(format!(
                "Name must be 1–{NAME_MAX} characters."
            )));
        }
        if !input.eula_accepted {
            return Err(OrchestratorError::InvalidInput(
                "You must accept the Minecraft EULA to create a server.".into(),
            ));
        }
        let settings = self.validated_settings(ServerSettings {
            name: name.to_owned(),
            mc_version: input.mc_version.trim().to_owned(),
            loader: input.loader.trim().to_ascii_lowercase(),
            loader_version: input
                .loader_version
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
            image: String::new(),
            java_version: None,
            memory_mb: input.memory_mb,
            cpu_limit: None,
            auto_start: false,
            sleep_when_empty: input.sleep_when_empty,
            auto_stop_idle_minutes: None,
            restart_on_crash: true,
            use_aikar_flags: true,
            motd: Some(format!("{name} · NetherNode")),
            max_players: 20,
            card_theme: input.card_theme,
            modpack_json: input
                .modpack
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| OrchestratorError::InvalidInput(e.to_string()))?,
        })?;

        let server = {
            let _guard = self.create_lock.lock().await;
            let slug = match input.slug {
                Some(slug) => {
                    if slugify(&slug) != slug || self.store.slug_exists(&slug).await? {
                        return Err(OrchestratorError::InvalidInput(format!(
                            "the slug {slug} is taken or invalid"
                        )));
                    }
                    slug
                }
                None => self.unique_slug(name).await?,
            };
            let mut used = self.store.used_ports().await?;
            used.extend(input.avoid_ports.iter().copied());
            let range = self.config.port_range.clone();
            let ip = self.config.backend_ip;
            let ports = ports::allocate(range.clone(), &used, 2, |p| ports::is_free(ip, p))
                .ok_or_else(|| {
                    OrchestratorError::NoPorts(format!("{}–{}", range.start(), range.end()))
                })?;
            let password = random_password();
            let sealed = self.secrets.seal(&password);
            self.store
                .insert_managed_server(
                    &slug,
                    "installing",
                    &settings,
                    i64::from(ports[0]),
                    i64::from(ports[1]),
                    &sealed,
                    now_ms(),
                )
                .await?
        };
        self.events.publish(Event::ServerStatusChanged {
            server_id: server.id.clone(),
            status: "installing".into(),
        });

        let this = self.clone();
        let id = server.id.clone();
        let start_after = input.start_after_create;
        tokio::spawn(async move {
            let rt = this.runtime(&id);
            let _op = rt.op.lock().await;
            match this.install(&id).await {
                Ok(()) => {
                    this.set_status(&id, "stopped").await;
                    drop(_op);
                    if start_after && let Err(e) = this.start(&id).await {
                        this.report_failure(&id, "start", &e);
                    }
                }
                Err(e) => {
                    this.report_failure(&id, "create", &e);
                    this.set_status(&id, "stopped").await;
                }
            }
        });
        Ok(server)
    }

    async fn install(&self, id: &str) -> Result<()> {
        let server = self.load(id).await?;
        self.ensure_container(&server).await?;
        Ok(())
    }

    /// Normalizes and validates settings, filling in the image.
    pub(crate) fn validated_settings(&self, mut s: ServerSettings) -> Result<ServerSettings> {
        if spec::loader_type(&s.loader).is_none() {
            return Err(OrchestratorError::InvalidInput(format!(
                "Unsupported server software: {}.",
                s.loader
            )));
        }
        let is_modpack = spec::MODPACK_LOADERS.contains(&s.loader.as_str());
        match (is_modpack, s.modpack_json.as_deref()) {
            (true, Some(json)) => {
                let pack: spec::Modpack = serde_json::from_str(json).map_err(|_| {
                    OrchestratorError::InvalidInput("The modpack details are invalid.".into())
                })?;
                if pack.slug.is_empty() && pack.project_id.is_empty() {
                    return Err(OrchestratorError::InvalidInput("Choose a modpack.".into()));
                }
            }
            (true, None) => {
                return Err(OrchestratorError::InvalidInput("Choose a modpack.".into()));
            }
            (false, _) => s.modpack_json = None,
        }
        if !java::is_valid_version(&s.mc_version) {
            return Err(OrchestratorError::InvalidInput(
                "Enter a Minecraft version like 1.21.4, or LATEST.".into(),
            ));
        }
        if java::is_floating(&s.mc_version) {
            s.mc_version = s.mc_version.to_ascii_uppercase();
        }
        if !MEMORY_RANGE.contains(&s.memory_mb) {
            return Err(OrchestratorError::InvalidInput(
                "Memory must be between 512 MB and 64 GB.".into(),
            ));
        }
        if !THEMES.contains(&s.card_theme.as_str()) {
            s.card_theme = "nether".into();
        }
        if !(1..=10_000).contains(&s.max_players) {
            return Err(OrchestratorError::InvalidInput(
                "Max players must be between 1 and 10,000.".into(),
            ));
        }
        if let Some(minutes) = s.auto_stop_idle_minutes
            && !(1..=1440).contains(&minutes)
        {
            return Err(OrchestratorError::InvalidInput(
                "Idle time must be between 1 and 1,440 minutes.".into(),
            ));
        }
        if s.cpu_limit.is_some_and(|c| !(0.1..=256.0).contains(&c)) {
            return Err(OrchestratorError::InvalidInput(
                "CPU limit must be between 0.1 and 256 cores.".into(),
            ));
        }
        if s.motd.as_ref().is_some_and(|m| m.chars().count() > 120) {
            return Err(OrchestratorError::InvalidInput(
                "The message of the day is limited to 120 characters.".into(),
            ));
        }
        // Custom images are kept; the default image follows the Java version.
        if s.image.is_empty() || s.image.starts_with(java::DEFAULT_IMAGE) {
            let java = s.java_version.and_then(|j| u32::try_from(j).ok());
            s.image = java::default_image(&s.mc_version, &s.loader, java);
        }
        Ok(s)
    }

    async fn unique_slug(&self, name: &str) -> Result<String> {
        let base = slugify(name);
        let mut slug = base.clone();
        let mut n = 2;
        while self.store.slug_exists(&slug).await? {
            slug = format!("{base}-{n}");
            n += 1;
        }
        Ok(slug)
    }

    // ----- containers -----

    pub(crate) fn spec(&self, server: &Server) -> Result<spec::ContainerSpec> {
        let password = match &server.rcon_password_enc {
            Some(sealed) => self.secrets.open(sealed)?,
            None => String::new(),
        };
        let extra_env: BTreeMap<String, String> =
            serde_json::from_str(&server.env_json).unwrap_or_default();
        let modpack: Option<spec::Modpack> = server
            .modpack_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok());
        let curseforge_key = self
            .curseforge_key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        Ok(spec::build(&SpecInput {
            server_id: &server.id,
            slug: &server.slug,
            image: &server.image,
            loader: &server.loader,
            loader_version: server.loader_version.as_deref(),
            mc_version: &server.mc_version,
            memory_mb: server.memory_mb,
            memory_overhead_percent: self.config.memory_overhead_percent,
            memory_overhead_min_mb: self.config.memory_overhead_min_mb,
            cpu_limit: server.cpu_limit,
            game_port: u16::try_from(server.internal_port).unwrap_or(0),
            rcon_port: server
                .rcon_port
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(0),
            mgmt_port: server.mgmt_port.and_then(|p| u16::try_from(p).ok()),
            rcon_password: &password,
            eula_accepted: server.eula_accepted_at.is_some(),
            use_aikar_flags: server.use_aikar_flags,
            motd: server.motd.as_deref(),
            max_players: server.max_players,
            extra_env: &extra_env,
            run_as: self.config.run_as,
            servers_dir: self
                .config
                .host_servers_dir
                .as_deref()
                .unwrap_or(&self.config.servers_dir),
            publish_ip: self.config.backend_ip,
            modpack: modpack.as_ref(),
            curseforge_api_key: curseforge_key.as_deref(),
        }))
    }

    /// Makes sure the server's container exists and matches its settings,
    /// pulling the image and (re)creating the container as needed.
    pub(crate) async fn ensure_container(&self, server: &Server) -> Result<String> {
        let server = &self.ensure_management(server).await?;
        let spec = self.spec(server)?;
        std::fs::create_dir_all(self.config.servers_dir.join(&server.slug))?;
        if server.loader == "curseforge" {
            // The image only looks in a downloads folder that exists.
            std::fs::create_dir_all(
                self.config
                    .servers_dir
                    .join(&server.slug)
                    .join(spec::CURSEFORGE_DOWNLOADS_DIR),
            )?;
        }

        if let Ok(existing) = self.docker.inspect_container(&spec.name, None).await {
            let hash = existing
                .config
                .and_then(|c| c.labels)
                .and_then(|l| l.get(spec::LABEL_SPEC_HASH).cloned());
            let id = existing.id.unwrap_or_default();
            if hash.as_deref() == Some(spec.hash.as_str()) {
                if server.container_id.as_deref() != Some(id.as_str()) {
                    self.store.set_container_id(&server.id, Some(&id)).await?;
                }
                return Ok(id);
            }
            tracing::info!(server = %server.id, "settings changed; recreating container");
            let options = RemoveContainerOptionsBuilder::default().force(true).build();
            self.docker
                .remove_container(&spec.name, Some(options))
                .await?;
        }

        if self.docker.inspect_image(&server.image).await.is_err() {
            self.pull_image(&server.id, &server.image).await?;
        }
        let options = CreateContainerOptionsBuilder::default()
            .name(&spec.name)
            .build();
        let created = self
            .docker
            .create_container(Some(options), spec.body)
            .await?;
        self.store
            .set_container_id(&server.id, Some(&created.id))
            .await?;
        Ok(created.id)
    }

    /// Gives the server a management protocol port and secret if it has
    /// none (servers created before player management, or new ones).
    async fn ensure_management(&self, server: &Server) -> Result<Server> {
        if server.mgmt_port.is_some() && server.mgmt_secret_enc.is_some() {
            return Ok(server.clone());
        }
        let _guard = self.create_lock.lock().await;
        let used = self.store.used_ports().await?;
        let range = self.config.port_range.clone();
        let ip = self.config.backend_ip;
        let port = ports::allocate(range.clone(), &used, 1, |p| ports::is_free(ip, p))
            .and_then(|p| p.first().copied())
            .ok_or_else(|| {
                OrchestratorError::NoPorts(format!("{}–{}", range.start(), range.end()))
            })?;
        let sealed = self.secrets.seal(&management_secret());
        self.store
            .set_management(&server.id, i64::from(port), &sealed)
            .await?;
        self.load(&server.id).await
    }

    /// Turns on the management protocol in `server.properties` before a
    /// start. The image keeps keys it doesn't manage, and versions before
    /// 1.21.9 ignore them.
    async fn write_management_properties(&self, server: &Server) -> Result<()> {
        let Some(sealed) = &server.mgmt_secret_enc else {
            return Ok(());
        };
        let secret = self.secrets.open(sealed)?;
        let dir = self.server_dir(&server.slug)?;
        tokio::task::spawn_blocking(move || -> std::result::Result<(), nn_files::FileError> {
            let files = nn_files::ServerFiles::open(&dir)?;
            let path = nn_files::RelPath::parse("server.properties")?;
            let mut props = match files.read(&path, nn_files::EDITOR_LIMIT) {
                Ok(content) => nn_files::Properties::parse(&String::from_utf8_lossy(&content.data)),
                Err(nn_files::FileError::NotFound(_)) => nn_files::Properties::default(),
                Err(e) => return Err(e),
            };
            let port = spec::MGMT_PORT.to_string();
            for (key, value) in [
                ("management-server-enabled", "true"),
                ("management-server-host", "0.0.0.0"),
                ("management-server-port", port.as_str()),
                ("management-server-secret", secret.as_str()),
                ("management-server-tls-enabled", "false"),
            ] {
                props.set(key, value);
            }
            files.write(&path, props.to_string().as_bytes(), nn_files::Expect::Any)?;
            Ok(())
        })
        .await
        .map_err(|e| OrchestratorError::Io(std::io::Error::other(e)))?
        .map_err(|e| OrchestratorError::Io(std::io::Error::other(e.to_string())))
    }

    /// Sets the work to do before each start. Only the first call counts.
    pub fn set_before_start(&self, hook: Arc<dyn BeforeStart>) {
        let _ = self.before_start.set(hook);
    }

    /// Whether the server's container is running right now, asked of
    /// Docker rather than the recorded status.
    pub async fn container_running(&self, id: &str) -> Result<bool> {
        let server = self.load(id).await?;
        match self
            .docker
            .inspect_container(&spec::container_name(&server.slug), None)
            .await
        {
            Ok(c) => Ok(c.state.and_then(|s| s.running).unwrap_or(false)),
            Err(e) if is_not_found(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Where to reach a running server's management protocol: its address
    /// and secret. `None` until the server has started with one.
    pub async fn management(&self, id: &str) -> Result<Option<(std::net::SocketAddr, String)>> {
        let server = self.load(id).await?;
        let (Some(port), Some(sealed)) = (server.mgmt_port, &server.mgmt_secret_enc) else {
            return Ok(None);
        };
        let port =
            u16::try_from(port).map_err(|_| OrchestratorError::InvalidInput("bad port".into()))?;
        Ok(Some((
            std::net::SocketAddr::new(self.config.backend_ip, port),
            self.secrets.open(sealed)?,
        )))
    }

    /// Pulls an image, publishing progress at most twice a second.
    async fn pull_image(&self, server_id: &str, image: &str) -> Result<()> {
        let (repo, tag) = match image.rsplit_once(':') {
            Some((repo, tag)) if !tag.contains('/') => (repo, tag),
            _ => (image, "latest"),
        };
        tracing::info!(%image, "pulling image");
        let options = CreateImageOptionsBuilder::default()
            .from_image(repo)
            .tag(tag)
            .build();
        let mut stream = self.docker.create_image(Some(options), None, None);
        let mut layers: HashMap<String, (i64, i64)> = HashMap::new();
        let mut last_sent = std::time::Instant::now() - Duration::from_secs(1);
        let publish = |status: &str, percent: Option<u8>| {
            self.events.publish(Event::ImagePullProgress {
                server_id: server_id.to_owned(),
                image: image.to_owned(),
                percent,
                status: status.to_owned(),
            });
        };
        publish("Starting download", None);
        while let Some(item) = stream.next().await {
            let info = item?;
            if let (Some(id), Some(detail)) = (info.id.clone(), info.progress_detail.as_ref())
                && let (Some(current), Some(total)) = (detail.current, detail.total)
                && total > 0
            {
                layers.insert(id, (current, total));
            }
            if last_sent.elapsed() >= Duration::from_millis(500) {
                last_sent = std::time::Instant::now();
                let (done, total) = layers
                    .values()
                    .fold((0, 0), |(d, t), (c, n)| (d + c, t + n));
                let percent = (total > 0).then(|| u8::try_from(done * 100 / total).unwrap_or(100));
                publish(info.status.as_deref().unwrap_or("Downloading"), percent);
            }
        }
        publish("Download complete", Some(100));
        Ok(())
    }

    // ----- lifecycle -----

    /// Starts a stopped, sleeping, or crashed server. `running` follows once
    /// the server answers a status ping.
    pub async fn start(self: &Arc<Self>, id: &str) -> Result<()> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        let server = self.load(id).await?;
        match server.status.as_str() {
            "running" | "starting" => return Ok(()),
            "installing" => {
                return Err(OrchestratorError::InvalidState(
                    "The server is still installing.".into(),
                ));
            }
            "stopping" => {
                return Err(OrchestratorError::InvalidState(
                    "The server is stopping. Try again in a moment.".into(),
                ));
            }
            _ => {}
        }
        if server.eula_accepted_at.is_none() {
            return Err(OrchestratorError::InvalidState(
                "Accept the Minecraft EULA before starting this server.".into(),
            ));
        }
        self.set_status(id, "starting").await;
        let result = async {
            let container = self.ensure_container(&server).await?;
            let server = self.load(id).await?;
            self.write_management_properties(&server).await?;
            if let Some(hook) = self.before_start.get() {
                hook.before_start(&server).await;
            }
            {
                let mut state = rt.state();
                state.expected_exit = None;
                state.launching = true;
            }
            let started = self
                .docker
                .start_container(&container, None::<StartContainerOptions>)
                .await;
            if started.is_err() {
                rt.state().launching = false;
            }
            started?;
            Ok::<_, OrchestratorError>(container)
        }
        .await;
        match result {
            Ok(container) => {
                crate::monitor::spawn(self, id, &container, &rt, false);
                Ok(())
            }
            Err(e) => {
                self.set_status(id, &server.status).await;
                Err(e)
            }
        }
    }

    /// Wakes a sleeping server (used by the proxy on login).
    pub async fn wake(self: &Arc<Self>, id: &str) -> Result<()> {
        self.start(id).await
    }

    /// Stops a server gracefully: RCON `stop`, wait up to the stop timeout,
    /// then Docker stop. With `sleep`, the server ends up `sleeping`.
    pub async fn stop(self: &Arc<Self>, id: &str, sleep: bool) -> Result<()> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        let server = self.load(id).await?;
        let target = if sleep { "sleeping" } else { "stopped" };
        if !matches!(server.status.as_str(), "running" | "starting") {
            // Already down: just record the intent (e.g. stop → sleep).
            if matches!(server.status.as_str(), "stopped" | "sleeping" | "crashed")
                && server.status != target
            {
                self.set_status(id, target).await;
            }
            return Ok(());
        }
        let Some(container) = server.container_id.clone() else {
            self.set_status(id, target).await;
            return Ok(());
        };
        self.set_status(id, "stopping").await;
        rt.state().expected_exit = Some(target);

        if let Err(error) = self.rcon_stop(&server).await {
            tracing::info!(server = %id, %error, "RCON stop unavailable; asking Docker to stop");
        }
        if !self
            .wait_for_exit(&container, self.config.stop_timeout)
            .await
        {
            tracing::warn!(server = %id, "graceful stop timed out; stopping the container");
            let options = StopContainerOptionsBuilder::default().t(10).build();
            if let Err(e) = self.docker.stop_container(&container, Some(options)).await {
                tracing::warn!(server = %id, error = %e, "docker stop failed");
            }
            self.wait_for_exit(&container, Duration::from_secs(15))
                .await;
        }
        self.finish_exit(id, &rt, target).await;
        Ok(())
    }

    /// Stops then starts.
    pub async fn restart(self: &Arc<Self>, id: &str) -> Result<()> {
        self.stop(id, false).await?;
        self.start(id).await
    }

    /// Kills the container immediately (players are disconnected without a save).
    pub async fn kill(self: &Arc<Self>, id: &str) -> Result<()> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        let server = self.load(id).await?;
        let Some(container) = server.container_id.clone() else {
            return Ok(());
        };
        rt.state().expected_exit = Some("stopped");
        let options = KillContainerOptionsBuilder::default()
            .signal("SIGKILL")
            .build();
        if let Err(e) = self.docker.kill_container(&container, Some(options)).await
            && !is_not_running(&e)
        {
            rt.state().expected_exit = None;
            return Err(e.into());
        }
        self.wait_for_exit(&container, Duration::from_secs(10))
            .await;
        self.finish_exit(id, &rt, "stopped").await;
        Ok(())
    }

    /// Records the end of a deliberate stop. Safe to call more than once.
    async fn finish_exit(&self, id: &str, rt: &Runtime, target: &'static str) {
        {
            let mut state = rt.state();
            state.stop_tasks();
            state.expected_exit = None;
            state.players = None;
        }
        self.set_status(id, target).await;
    }

    /// Changes settings. Returns the server and whether a restart is needed
    /// for the changes to take effect (the container is recreated on start).
    pub async fn update(
        self: &Arc<Self>,
        id: &str,
        settings: ServerSettings,
    ) -> Result<(Server, bool)> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        self.load(id).await?;
        // An empty image is the default one for the Java version.
        let settings = self.validated_settings(settings)?;
        self.store.update_server_settings(id, &settings).await?;
        let server = self.load(id).await?;
        let restart = self.restart_required(&server).await;
        Ok((server, restart))
    }

    /// Deletes a server: kills its container, removes it, and deletes the
    /// record. With `delete_data`, also deletes its world and files.
    pub async fn delete(self: &Arc<Self>, id: &str, delete_data: bool) -> Result<()> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        let server = self.load(id).await?;
        rt.state().expected_exit = Some("stopped");
        rt.state().stop_tasks();
        let name = spec::container_name(&server.slug);
        let options = RemoveContainerOptionsBuilder::default().force(true).build();
        if let Err(e) = self.docker.remove_container(&name, Some(options)).await
            && !is_not_found(&e)
        {
            return Err(e.into());
        }
        self.store.delete_server(id).await?;
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if delete_data {
            let dir = self.server_dir(&server.slug)?;
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
        }
        self.events.publish(Event::ServerStatusChanged {
            server_id: id.to_owned(),
            status: "deleted".into(),
        });
        tracing::info!(server = %id, delete_data, "server deleted");
        Ok(())
    }

    /// Takes over a server moved here from another machine: records it as
    /// this machine's, keeping its ID, slug and settings, with a fresh RCON
    /// password. It keeps its ports unless they're taken here or in
    /// `avoid_ports` (other machines' servers). Its status is `installing`
    /// until [`Self::finish_import`], once the caller has filled its folder.
    pub async fn import(&self, server: &Server, avoid_ports: &[i64]) -> Result<Server> {
        self.server_dir(&server.slug)?;
        let _guard = self.create_lock.lock().await;
        if let Some(owner) = self.store.slug_owner(&server.slug).await?
            && owner != server.id
        {
            return Err(OrchestratorError::InvalidInput(format!(
                "another server on this machine is already called {}",
                server.slug
            )));
        }
        let mut used = self.store.ports_used_by_others(&server.id).await?;
        used.extend(avoid_ports.iter().copied());
        let ip = self.config.backend_ip;
        let usable = |port: i64| {
            !used.contains(&port) && u16::try_from(port).is_ok_and(|p| ports::is_free(ip, p))
        };
        let (game, rcon) = match server.rcon_port {
            Some(rcon) if usable(server.internal_port) && usable(rcon) => {
                (server.internal_port, rcon)
            }
            _ => {
                let range = self.config.port_range.clone();
                let ports = ports::allocate(range.clone(), &used, 2, |p| ports::is_free(ip, p))
                    .ok_or_else(|| {
                        OrchestratorError::NoPorts(format!("{}–{}", range.start(), range.end()))
                    })?;
                (i64::from(ports[0]), i64::from(ports[1]))
            }
        };
        let row = Server {
            status: "installing".into(),
            internal_port: game,
            rcon_port: Some(rcon),
            public_port: None,
            container_id: None,
            rcon_password_enc: None,
            mgmt_port: None,
            mgmt_secret_enc: None,
            node_id: nn_store::LOCAL_NODE.into(),
            updated_at: now_ms(),
            ..server.clone()
        };
        // Written like a mirrored row (no secrets), then given its own.
        self.store
            .upsert_server_mirror(&row, nn_store::LOCAL_NODE)
            .await?;
        self.store
            .set_rcon_secret(&row.id, &self.secrets.seal(&random_password()))
            .await?;
        self.events.publish(Event::ServerStatusChanged {
            server_id: row.id.clone(),
            status: "installing".into(),
        });
        self.load(&row.id).await
    }

    /// Sets up the container of a server [`Self::import`]ed here, once its
    /// folder is in place. It ends up stopped.
    pub async fn finish_import(self: &Arc<Self>, id: &str) -> Result<Server> {
        let rt = self.runtime(id);
        let _op = rt.op.lock().await;
        self.install(id).await?;
        self.set_status(id, "stopped").await;
        self.load(id).await
    }

    /// Lets go of a server that now lives on another machine (or never
    /// finished moving here): removes its container and folder on this
    /// machine. Its record is left to the caller.
    pub async fn release(self: &Arc<Self>, id: &str, slug: &str) -> Result<()> {
        let dir = self.server_dir(slug)?;
        let rt = self.runtime(id);
        {
            let _op = rt.op.lock().await;
            rt.state().expected_exit = Some("stopped");
            rt.state().stop_tasks();
            let options = RemoveContainerOptionsBuilder::default().force(true).build();
            if let Err(e) = self
                .docker
                .remove_container(&spec::container_name(slug), Some(options))
                .await
                && !is_not_found(&e)
            {
                return Err(e.into());
            }
        }
        self.runtimes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
        if dir.exists() {
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir))
                .await
                .map_err(std::io::Error::other)??;
        }
        tracing::info!(server = %id, "released a server that moved to another machine");
        Ok(())
    }

    /// A server's data directory, refusing anything outside `servers_dir`.
    fn server_dir(&self, slug: &str) -> Result<PathBuf> {
        if slug.is_empty()
            || !slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(OrchestratorError::InvalidInput(
                "invalid server slug".into(),
            ));
        }
        Ok(self.config.servers_dir.join(slug))
    }

    async fn rcon_stop(&self, server: &Server) -> std::result::Result<(), String> {
        let mut rcon = self.rcon_connect(server).await?;
        rcon.send_only("stop").await.map_err(|e| e.to_string())
    }

    async fn rcon_connect(&self, server: &Server) -> std::result::Result<nn_players::Rcon, String> {
        let port = server
            .rcon_port
            .and_then(|p| u16::try_from(p).ok())
            .ok_or("no RCON port")?;
        let sealed = server
            .rcon_password_enc
            .as_deref()
            .ok_or("no RCON password")?;
        let password = self.secrets.open(sealed).map_err(|e| e.to_string())?;
        nn_players::Rcon::connect(
            std::net::SocketAddr::new(self.config.backend_ip, port),
            &password,
            Duration::from_secs(5),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Runs a console command over RCON and returns its output (often
    /// empty; the server log shows the effect). One connection per server
    /// stays open: Minecraft logs every RCON connect and disconnect, so
    /// reconnecting per command would flood the console.
    pub async fn command(&self, id: &str, command: &str) -> Result<String> {
        let command = command.trim();
        let command = command.strip_prefix('/').unwrap_or(command);
        if command.is_empty() {
            return Err(OrchestratorError::InvalidInput("type a command".into()));
        }
        let server = self.load(id).await?;
        if server.status != "running" {
            return Err(OrchestratorError::InvalidState(
                "commands can be sent once the server is running".into(),
            ));
        }
        let rt = self.runtime(id);
        let mut conn = rt.rcon.lock().await;
        // A kept connection may be stale (the server restarted); retry once
        // on a fresh one.
        for fresh in [conn.is_none(), true] {
            if fresh || conn.is_none() {
                *conn = Some(
                    self.rcon_connect(&server)
                        .await
                        .map_err(OrchestratorError::Command)?,
                );
            }
            let Some(rcon) = conn.as_mut() else { continue };
            match rcon.command(command).await {
                Ok(output) => return Ok(output),
                Err(nn_players::RconError::CommandTooLong(_)) => {
                    return Err(OrchestratorError::InvalidInput(
                        "that command is too long for RCON".into(),
                    ));
                }
                Err(e) => {
                    *conn = None;
                    if fresh {
                        return Err(OrchestratorError::Command(e.to_string()));
                    }
                }
            }
        }
        Err(OrchestratorError::Command("RCON isn't responding".into()))
    }

    /// Waits until the container isn't running. Returns `false` on timeout.
    pub(crate) async fn wait_for_exit(&self, container: &str, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.docker.inspect_container(container, None).await {
                Ok(c) if c.state.as_ref().and_then(|s| s.running).unwrap_or(false) => {}
                _ => return true,
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    pub(crate) fn record_stats(&self, id: &str, sample: StatSample) {
        let rt = self.runtime(id);
        rt.stats
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(sample);
    }
}

/// Sizes a server folder: everything, and the worlds named by
/// `level-name` in `server.properties` (with Bukkit-style `_nether` and
/// `_the_end` folders beside it).
fn measure_folder(dir: &std::path::Path) -> FolderUsage {
    let level = std::fs::read_to_string(dir.join("server.properties"))
        .ok()
        .and_then(|props| {
            props.lines().find_map(|line| {
                line.strip_prefix("level-name=")
                    .map(|v| v.trim().to_owned())
            })
        })
        .filter(|level| !level.is_empty() && !level.contains(['/', '\\']) && level != "..")
        .unwrap_or_else(|| "world".to_owned());
    let world_bytes = [
        level.clone(),
        format!("{level}_nether"),
        format!("{level}_the_end"),
    ]
    .iter()
    .map(|name| folder_size(&dir.join(name)))
    .sum();
    FolderUsage {
        total_bytes: folder_size(dir),
        world_bytes,
    }
}

/// Bytes in the files under `path`, not following links.
fn folder_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![path.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// Lowercase letters, digits, and single dashes; at most 32 characters.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 32 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.is_empty() {
        "server".to_owned()
    } else {
        slug
    }
}

/// 40 alphanumeric characters, the format Minecraft requires.
fn management_secret() -> String {
    use rand::distr::{Alphanumeric, SampleString};
    Alphanumeric.sample_string(&mut rand::rng(), 40)
}

fn random_password() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn is_not_found(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn is_not_running(e: &bollard::errors::Error) -> bool {
    matches!(
        e,
        bollard::errors::Error::DockerResponseServerError {
            status_code: 409,
            ..
        }
    ) || is_not_found(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_dns_safe() {
        assert_eq!(slugify("Nether SMP"), "nether-smp");
        assert_eq!(slugify("  Better MC!! (Forge) "), "better-mc-forge");
        assert_eq!(slugify("Ünïcødé"), "n-c-d");
        assert_eq!(slugify("!!!"), "server");
        assert_eq!(slugify(&"a".repeat(50)).len(), 32);
    }

    #[test]
    fn passwords_are_random() {
        assert_ne!(random_password(), random_password());
        assert_eq!(random_password().len(), 32);
    }
}
