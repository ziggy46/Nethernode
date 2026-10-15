//! The opt-in "active installs" count.
//!
//! Off unless the configuration or an owner turns it on. Then, about once a
//! day, NetherNode sends a random install ID (made on this machine, from
//! nothing) and its version to the counter, and nothing else. Turning it
//! off deletes the ID.

use std::sync::RwLock;
use std::time::Duration;

use connectrpc::ConnectError;
use nn_store::Store;
use rand::{Rng, RngExt};
use serde_json::json;
use tokio::sync::Notify;

use crate::AppState;

/// Where reports go unless configured otherwise.
pub const DEFAULT_URL: &str = "https://nethernode-installs.SUBDOMAIN.workers.dev/ping";

const ENABLED_KEY: &str = "install_count.enabled";
const ID_KEY: &str = "install_count.id";
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// What's reported, for showing people exactly that.
pub fn payload(id: &str) -> serde_json::Value {
    json!({ "id": id, "version": env!("CARGO_PKG_VERSION") })
}

/// The settings screen's view.
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// Reporting is on.
    pub enabled: bool,
    /// Turned on by the configuration, so the switch can't turn it off.
    pub forced: bool,
    /// The install ID, while on.
    pub id: Option<String>,
    /// Where reports go.
    pub url: String,
    /// When a report last went through, unix ms.
    pub last_sent_ms: Option<i64>,
    /// Why the last report didn't go through.
    pub last_error: Option<String>,
}

#[derive(Debug, Default)]
struct Last {
    sent_ms: Option<i64>,
    error: Option<String>,
}

/// Sends the daily report when it's on.
#[derive(Debug)]
pub struct InstallCount {
    url: String,
    forced: bool,
    client: reqwest::Client,
    wake: Notify,
    last: RwLock<Last>,
}

impl Default for InstallCount {
    fn default() -> Self {
        Self::new(DEFAULT_URL.to_owned(), false)
    }
}

impl InstallCount {
    /// Reports to `url`; `forced` means the configuration turned it on.
    pub fn new(url: String, forced: bool) -> Self {
        Self {
            url,
            forced,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .user_agent(concat!("NetherNode/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("HTTP client"),
            wake: Notify::new(),
            last: RwLock::default(),
        }
    }

    async fn enabled(&self, store: &Store) -> bool {
        self.forced
            || store
                .get_setting::<bool>(ENABLED_KEY)
                .await
                .ok()
                .flatten()
                .unwrap_or(false)
    }

    /// The install ID, made the first time it's needed.
    async fn id(&self, store: &Store) -> Result<String, nn_store::Error> {
        if let Some(id) = store.get_setting::<String>(ID_KEY).await? {
            return Ok(id);
        }
        let mut bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut bytes);
        let id = uuid::Builder::from_random_bytes(bytes)
            .into_uuid()
            .to_string();
        store.set_setting(ID_KEY, &id).await?;
        Ok(id)
    }

    /// The settings screen's view.
    pub async fn status(&self, store: &Store) -> Status {
        let enabled = self.enabled(store).await;
        let id = if enabled {
            self.id(store).await.ok()
        } else {
            None
        };
        let last = self.last.read().unwrap_or_else(|e| e.into_inner());
        Status {
            enabled,
            forced: self.forced,
            id,
            url: self.url.clone(),
            last_sent_ms: last.sent_ms,
            last_error: last.error.clone(),
        }
    }

    /// Turns reporting on or off. Off deletes the install ID.
    pub(crate) async fn set(&self, store: &Store, on: bool) -> Result<(), ConnectError> {
        if self.forced && !on {
            return Err(ConnectError::failed_precondition(
                "it's turned on in NetherNode's configuration; remove the setting there to turn it off",
            ));
        }
        store
            .set_setting(ENABLED_KEY, &on)
            .await
            .map_err(crate::rpc::store_error)?;
        if on {
            self.id(store).await.map_err(crate::rpc::store_error)?;
            self.wake.notify_one();
        } else {
            store
                .delete_setting(ID_KEY)
                .await
                .map_err(crate::rpc::store_error)?;
            *self.last.write().unwrap_or_else(|e| e.into_inner()) = Last::default();
        }
        Ok(())
    }

    async fn report(&self, store: &Store) {
        let result = async {
            let id = self.id(store).await.map_err(|e| e.to_string())?;
            let response = self
                .client
                .post(&self.url)
                .json(&payload(&id))
                .send()
                .await
                .map_err(|e| format!("couldn't reach the counter: {}", e.without_url()))?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(format!(
                    "the counter answered {}",
                    response.status().as_u16()
                ))
            }
        }
        .await;
        let mut last = self.last.write().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(()) => {
                last.sent_ms = Some(nn_store::now_ms());
                last.error = None;
            }
            Err(e) => {
                tracing::debug!(error = %e, "install count report failed");
                last.error = Some(e);
            }
        }
    }
}

/// Reports once a day while it's on (soon after it's turned on, too).
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let counter = state.install_count.clone();
        let store = state.auth.store().clone();
        // Spread reports out, so restarts and many installs don't line up.
        let first = Duration::from_secs(rand::rng().random_range(60..=3600));
        let mut wait = first;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = counter.wake.notified() => {}
            }
            if counter.enabled(&store).await {
                counter.report(&store).await;
            }
            wait = DAY + Duration::from_secs(rand::rng().random_range(0..=3600));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_an_id_and_the_version() {
        let p = payload("3f1c2b9a-7d4e-4a6b-9c2d-1e0f5a7b8c9d");
        let keys: Vec<&String> = p.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["id", "version"]);
    }

    /// A stand-in counter that records what it receives.
    async fn counter() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        use axum::Json;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let app = axum::Router::new().route(
            "/ping",
            axum::routing::post(move |Json(body): Json<serde_json::Value>| {
                let seen = seen2.clone();
                async move {
                    seen.lock().unwrap().push(body);
                    http::StatusCode::NO_CONTENT
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/ping"), seen)
    }

    #[tokio::test]
    async fn off_by_default_and_off_forgets_the_id() {
        let store = Store::open_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let (url, seen) = counter().await;
        let count = InstallCount::new(url, false);
        let status = count.status(&store).await;
        assert!(!status.enabled && status.id.is_none());
        assert!(
            store.get_setting::<String>(ID_KEY).await.unwrap().is_none(),
            "no ID until it's on"
        );

        count.set(&store, true).await.unwrap();
        let id = count.status(&store).await.id.unwrap();
        assert!(uuid::Uuid::parse_str(&id).unwrap().get_version_num() == 4);
        count.report(&store).await;
        assert_eq!(seen.lock().unwrap().as_slice(), [payload(&id)]);
        assert!(count.status(&store).await.last_sent_ms.is_some());

        count.set(&store, false).await.unwrap();
        assert!(store.get_setting::<String>(ID_KEY).await.unwrap().is_none());
        count.set(&store, true).await.unwrap();
        assert_ne!(
            count.status(&store).await.id.unwrap(),
            id,
            "a fresh ID after turning it back on"
        );
    }

    #[tokio::test]
    async fn the_configuration_can_turn_it_on_for_good() {
        let store = Store::open_in_memory().await.unwrap();
        store.migrate().await.unwrap();
        let count = InstallCount::new("http://127.0.0.1:9/ping".into(), true);
        assert!(count.status(&store).await.enabled);
        assert!(count.set(&store, false).await.is_err());
        count.report(&store).await;
        assert!(
            count
                .status(&store)
                .await
                .last_error
                .unwrap()
                .contains("couldn't reach")
        );
    }
}
