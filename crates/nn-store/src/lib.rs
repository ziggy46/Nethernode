//! SQLite storage for NetherNode.
//!
//! [`Store`] owns the connection pool. It is opened in WAL mode with
//! `busy_timeout=5000`, `foreign_keys=ON`, and `synchronous=NORMAL`. Queries
//! are compile-time checked with `sqlx` macros against the migrations in
//! `migrations/`. The offline query cache lives in `.sqlx/` at the workspace
//! root, and `just db-prepare` regenerates it.

use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

pub mod audit;
pub mod players;
pub mod proxy;
pub mod servers;
pub mod sessions;
pub mod settings;
pub mod users;

pub use audit::NewAuditEntry;
pub use players::{NewPlayerSession, PlayerSession};
pub use proxy::{ListenerInput, ProxyListener, Route, RouteInput};
pub use servers::{NewServer, Server, ServerSettings};
pub use sessions::{NewSession, Session};
pub use users::{NewUser, User};

/// Embedded migrations from `crates/nn-store/migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Errors returned by the storage layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database error.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    /// Applying migrations failed.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A stored JSON value could not be (de)serialized.
    #[error("invalid JSON value: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// Whether this is a UNIQUE constraint violation (e.g. a duplicate hostname).
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, Error::Database(sqlx::Error::Database(e)) if e.is_unique_violation())
    }
}

/// Result alias for storage operations.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Handle to the NetherNode database. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens (creating if needed) the SQLite database at `path`.
    ///
    /// Migrations are not applied; call [`Store::migrate`].
    pub async fn open(path: &Path) -> Result<Self> {
        let options = base_options(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Opens a private in-memory database. Intended for tests.
    pub async fn open_in_memory() -> Result<Self> {
        // An in-memory database lives as long as its connection, so the pool
        // is pinned to exactly one connection that never expires.
        let options = base_options(SqliteConnectOptions::from_str("sqlite::memory:")?);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Applies any pending migrations.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// The underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Closes all pooled connections.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn base_options(options: SqliteConnectOptions) -> SqliteConnectOptions {
    options
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_millis(5000))
        .foreign_keys(true)
}

/// Current time as unix milliseconds, the timestamp format used in every table.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Generates a new ULID primary key.
///
/// IDs are monotonic within the process, so sorting by ID matches insertion
/// order even for rows created in the same millisecond.
pub fn new_id() -> String {
    static GENERATOR: Mutex<ulid::Generator> = Mutex::new(ulid::Generator::new());
    let mut generator = GENERATOR.lock().unwrap_or_else(|e| e.into_inner());
    generator
        .generate()
        .unwrap_or_else(|overflow| overflow.commit_overflow_random())
        .to_string()
}

#[cfg(test)]
pub(crate) async fn test_store() -> Store {
    let store = Store::open_in_memory().await.expect("open in-memory store");
    store.migrate().await.expect("migrate");
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pragmas_are_applied_to_file_databases() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).await.unwrap();
        store.migrate().await.unwrap();

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let busy: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(busy, 5000);
        // NORMAL = 1
        let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(sync, 1);
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let store = test_store().await;
        store.migrate().await.unwrap();
    }
}
