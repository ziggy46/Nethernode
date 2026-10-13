//! `nethernode`: the NetherNode server binary.

use std::fs;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nn_api::{ApiConfig, AppState};
use nn_auth::{Auth, AuthConfig};
use nn_events::EventBus;
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
    let auth = Arc::new(Auth::new(store, AuthConfig::default()));
    spawn_session_pruner(auth.clone());

    let state = AppState {
        auth: auth.clone(),
        events: EventBus::default(),
        config: Arc::new(ApiConfig {
            secure_cookies: config.http.secure_cookies.into(),
            trust_proxy_headers: config.http.trust_proxy_headers,
        }),
        metrics: Some(metrics),
    };
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

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    auth.store().close().await;
    tracing::info!("shut down cleanly");
    Ok(())
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
