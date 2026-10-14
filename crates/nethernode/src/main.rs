//! `nethernode`: the NetherNode server binary.

use std::fs;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nn_api::{ApiConfig, AppState, ProxyRuntime, ServerManager, StoreProxyHooks};
use nn_auth::{Auth, AuthConfig, SecretBox};
use nn_events::EventBus;
use nn_orchestrator::{Orchestrator, OrchestratorConfig};
use nn_proxy::{Limits, ListenerState, Proxy};
use nn_store::Store;
use rand::Rng;
use tracing_subscriber::EnvFilter;

mod config;

use config::Config;

#[derive(Debug, Parser)]
#[command(
    name = "nethernode",
    version,
    about = "Self-hosted Minecraft server manager"
)]
struct Cli {
    /// Path to the config file. Missing files are fine; defaults apply.
    #[arg(
        long,
        short,
        global = true,
        env = "NETHERNODE_CONFIG",
        default_value = "nethernode.toml"
    )]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the web panel and API (default).
    Serve,
    /// Apply database migrations and exit.
    Migrate,
    /// Set a new random password for an owner account and sign out its sessions.
    ResetAdmin {
        /// Account to reset. Defaults to the first owner.
        #[arg(long)]
        username: Option<String>,
    },
    /// Print version information.
    Version,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Command::Version) = cli.command {
        println!("nethernode {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let config = Config::load(&cli.config)
        .with_context(|| format!("invalid configuration in {}", cli.config.display()))?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        match cli.command.unwrap_or(Command::Serve) {
            Command::Serve => serve(config).await,
            Command::Migrate => migrate(config).await,
            Command::ResetAdmin { username } => reset_admin(config, username).await,
            Command::Version => unreachable!("handled above"),
        }
    })
}

async fn serve(config: Config) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn")),
        )
        .init();

    let metrics = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .context("installing metrics recorder")?;

    let store = open_store(&config).await?;
    let secrets = match SecretBox::load(&config.data.secret_key_path()) {
        Ok(secrets) => Some(Arc::new(secrets)),
        Err(e) => {
            tracing::warn!(error = %e, "the data key couldn't be loaded");
            None
        }
    };
    let auth =
        Arc::new(Auth::new(store.clone(), AuthConfig::default()).with_secrets(secrets.clone()));
    spawn_session_pruner(auth.clone());

    let events = EventBus::default();
    let hooks = StoreProxyHooks::new(store.clone());
    let proxy = start_proxy(&config, &store, &events, hooks.clone()).await?;
    let servers = start_orchestrator(&config, &store, &events, secrets.clone()).await;
    if let Some(orchestrator) = &servers.orchestrator {
        hooks.set_orchestrator(orchestrator.clone());
    }
    spawn_proxy_reloader(proxy.clone(), &events);

    let upload_settings = config.uploads.settings().map_err(|e| anyhow::anyhow!(e))?;
    let upload_dir = config.data.dir.join("tmp").join("uploads");
    let temp = nn_uploads::TempStore::open(&upload_dir)
        .with_context(|| format!("creating {}", upload_dir.display()))?;
    let uploads = nn_api::uploads::Uploads::new(upload_settings, temp);

    let content = Arc::new(nn_api::content::Content::new(
        nn_catalog::Catalog::new(
            Some(config.data.dir.join("cache").join("catalog")),
            Default::default(),
        ),
        secrets.clone(),
        Some(config.data.dir.join("cache")),
    ));
    let curseforge_key = content.load_curseforge_key(&store).await;
    if let Some(orchestrator) = &servers.orchestrator {
        orchestrator.set_curseforge_key(curseforge_key);
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let notifier = Arc::new(nn_api::notify::Notifier::default());
    notifier.load(&store, secrets.as_deref()).await;
    let state = AppState {
        auth: auth.clone(),
        proxy: proxy.clone(),
        events,
        config: Arc::new(ApiConfig {
            secure_cookies: config.http.secure_cookies.into(),
            trust_proxy_headers: config.http.trust_proxy_headers,
        }),
        metrics: Some(metrics),
        shutdown: shutdown_rx.clone(),
        servers,
        players: Arc::new(nn_api::players::PlayerManager::new(hooks.bans())),
        uploads: Arc::new(uploads),
        content: content.clone(),
        notifier: notifier.clone(),
    };
    nn_api::players::tasks::spawn(state.clone());
    nn_api::uploads::tasks::spawn(state.clone());
    nn_api::backups::tasks::recover(&state).await;
    nn_api::schedules::runner::spawn(state.clone());
    nn_api::notify::spawn(state.clone());
    let app = nn_api::router(state);

    let listener = tokio::net::TcpListener::bind(config.http.bind)
        .await
        .with_context(|| format!("binding {}", config.http.bind))?;
    let addr = listener.local_addr()?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "NetherNode panel listening on http://{addr}"
    );
    if auth.needs_setup().await? {
        tracing::info!(
            "No accounts yet. Open http://{} to create the owner account.",
            display_addr(addr)
        );
    }

    let mut stopping = shutdown_rx;
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });
    // Give in-flight requests a few seconds, then stop regardless.
    tokio::select! {
        result = server => result?,
        () = async {
            let _ = stopping.wait_for(|stop| *stop).await;
            tokio::time::sleep(Duration::from_secs(10)).await;
        } => tracing::warn!("requests still open after 10 s; shutting down anyway"),
    }
    proxy.proxy().shutdown().await;
    auth.store().close().await;
    tracing::info!("shut down cleanly");
    Ok(())
}

/// Seeds the default listener on first run, loads routes, and starts the
/// Smart Proxy and its status cache refresher.
async fn start_proxy(
    config: &Config,
    store: &Store,
    events: &EventBus,
    hooks: Arc<StoreProxyHooks>,
) -> Result<Arc<ProxyRuntime>> {
    let stale = store.close_stale_player_sessions().await?;
    if stale > 0 {
        tracing::info!(
            sessions = stale,
            "closed player sessions left open by the last shutdown"
        );
    }
    if config.proxy.enabled {
        let bind = config.proxy.default_listener;
        if store
            .ensure_default_listener(&bind.ip().to_string(), i64::from(bind.port()))
            .await?
        {
            tracing::info!(%bind, "created the default proxy listener");
        }
    } else {
        tracing::info!("Smart Proxy is disabled in the configuration");
    }

    let proxy = Proxy::new(hooks, events.clone(), Limits::default());
    let runtime = Arc::new(ProxyRuntime::new(
        proxy,
        store.clone(),
        config.proxy.enabled,
    ));
    for listener in runtime.reload().await? {
        if let ListenerState::Failed(error) = &listener.state {
            tracing::warn!(bind = %listener.spec.bind, %error, "proxy listener is not running; fix it in Proxy & routes");
        }
    }
    runtime.proxy().spawn_status_refresher();
    Ok(runtime)
}

/// Connects to Docker and starts the orchestrator. If Docker can't be
/// reached, the panel still runs; server management reports why it's off.
async fn start_orchestrator(
    config: &Config,
    store: &Store,
    events: &EventBus,
    secrets: Option<Arc<SecretBox>>,
) -> ServerManager {
    // Bind mounts need an absolute path.
    let servers_dir = match fs::canonicalize(&config.data.dir) {
        Ok(dir) => Some(dir.join("servers")),
        Err(e) => {
            tracing::warn!(error = %e, "data directory unavailable; server files can't be reached");
            None
        }
    };
    let unavailable = |reason: String| {
        tracing::warn!(socket = %config.docker.socket, %reason, "Docker isn't available; server management is disabled");
        ServerManager {
            orchestrator: None,
            unavailable_reason: Some(Arc::from(reason)),
            servers_dir: servers_dir.clone(),
        }
    };
    let Some(secrets) = secrets else {
        return unavailable("the data key couldn't be loaded".into());
    };
    let Some(dir) = servers_dir.clone() else {
        return unavailable("the data directory isn't available".into());
    };
    let mut orch_config = OrchestratorConfig::new(dir);
    orch_config.port_range = config.docker.port_range_start..=config.docker.port_range_end;
    orch_config.memory_overhead_percent = config.docker.memory_overhead_percent;
    orch_config.memory_overhead_min_mb = config.docker.memory_overhead_min_mb;
    match Orchestrator::connect(
        &config.docker.socket,
        store.clone(),
        events.clone(),
        secrets,
        orch_config,
    )
    .await
    {
        Ok(orchestrator) => {
            nn_orchestrator::spawn_background(&orchestrator);
            tracing::info!(socket = %config.docker.socket, "connected to Docker");
            ServerManager {
                orchestrator: Some(orchestrator),
                unavailable_reason: None,
                servers_dir,
            }
        }
        Err(e) => unavailable(e.to_string()),
    }
}

/// Rebuilds proxy routes when servers change state, so the proxy always
/// knows which servers are running, sleeping, or gone.
fn spawn_proxy_reloader(proxy: Arc<ProxyRuntime>, events: &EventBus) {
    let mut rx = events.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(nn_events::Event::ServerStatusChanged { .. }) => {}
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
            // Coalesce bursts (e.g. reconciliation at startup).
            tokio::time::sleep(Duration::from_millis(100)).await;
            while let Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) =
                rx.try_recv()
            {}
            if let Err(error) = proxy.reload().await {
                tracing::warn!(%error, "failed to reload proxy routes");
            }
        }
    });
}

async fn migrate(config: Config) -> Result<()> {
    let store = open_store(&config).await?;
    store.close().await;
    println!(
        "Database is up to date: {}",
        config.data.database_path().display()
    );
    Ok(())
}

async fn reset_admin(config: Config, username: Option<String>) -> Result<()> {
    let store = open_store(&config).await?;
    let auth = Auth::new(store, AuthConfig::default());
    let (user, password) = auth
        .reset_owner_password(username.as_deref())
        .await
        .context("could not reset the password")?;
    auth.store().close().await;
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "Password reset for '{}' ({}).",
        user.username, user.role
    )?;
    writeln!(out, "New password: {password}")?;
    writeln!(out, "All of this account's sessions have been signed out.")?;
    if user.totp_enabled {
        writeln!(
            out,
            "Two-factor sign-in has been turned off; set it up again after signing in."
        )?;
    }
    writeln!(
        out,
        "You'll be asked to choose a new password when you sign in."
    )?;
    Ok(())
}

/// Creates the data directory and secret key, then opens and migrates the database.
async fn open_store(config: &Config) -> Result<Store> {
    fs::create_dir_all(&config.data.dir)
        .with_context(|| format!("creating data directory {}", config.data.dir.display()))?;
    ensure_secret_key(&config.data.secret_key_path())?;
    let store = Store::open(&config.data.database_path())
        .await
        .context("opening database")?;
    store.migrate().await.context("applying migrations")?;
    Ok(store)
}

/// Creates the 256-bit at-rest encryption key (mode 0600) if it doesn't exist.
/// Later phases use it to encrypt RCON passwords.
fn ensure_secret_key(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(hex::encode(key).as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn spawn_session_pruner(auth: Arc<Auth>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            interval.tick().await;
            match auth.prune_expired_sessions().await {
                Ok(0) => {}
                Ok(n) => tracing::debug!(removed = n, "pruned expired sessions"),
                Err(e) => tracing::warn!(error = %e, "failed to prune sessions"),
            }
        }
    });
}

/// `0.0.0.0` isn't browsable; suggest localhost instead.
fn display_addr(addr: SocketAddr) -> String {
    if addr.ip().is_unspecified() {
        format!("localhost:{}", addr.port())
    } else {
        addr.to_string()
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_key_is_created_once_with_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.key");
        ensure_secret_key(&path).unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(first.len(), 64);
        ensure_secret_key(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn cli_parses_subcommands() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
        let cli = Cli::parse_from(["nethernode", "reset-admin", "--username", "alice"]);
        assert!(
            matches!(cli.command, Some(Command::ResetAdmin { username: Some(ref u) }) if u == "alice")
        );
        assert!(Cli::parse_from(["nethernode"]).command.is_none());
    }
}
