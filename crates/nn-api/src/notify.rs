//! Discord webhook notifications: servers starting, stopping, and crashing,
//! players joining, and backups failing.
//!
//! Events are gathered for a few seconds and sent as one message, so a
//! restart or a burst of joins doesn't flood the channel (or Discord's rate
//! limit, which is honored when it's hit anyway).

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::Duration;

use connectrpc::ConnectError;
use nn_auth::SecretBox;
use nn_events::Event;
use nn_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;

const SETTINGS_KEY: &str = "notifications.discord";
/// How long events gather before a message goes out.
const BATCH: Duration = Duration::from_secs(4);
/// Discord allows ten embeds per message.
const MAX_EMBEDS: usize = 10;

/// Which events are sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Toggles {
    /// Servers starting, stopping, and restarting.
    pub start_stop: bool,
    /// Servers crashing.
    pub crash: bool,
    /// Players joining.
    pub player_join: bool,
    /// Backups failing.
    pub backup_failed: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            start_stop: true,
            crash: true,
            player_join: false,
            backup_failed: true,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// The webhook URL, sealed with the data key.
    webhook: Option<String>,
    #[serde(default)]
    toggles: Toggles,
}

/// What the settings screen shows.
#[derive(Debug, Clone, Default)]
pub struct Status {
    /// The end of the webhook URL, to recognize it by.
    pub webhook_hint: Option<String>,
    /// Which events are sent.
    pub toggles: Toggles,
    /// Why the last message didn't go through, if it didn't.
    pub last_error: Option<String>,
    /// When a message last went through, unix ms.
    pub last_sent_ms: Option<i64>,
}

#[derive(Debug, Default)]
struct Inner {
    webhook: Option<String>,
    toggles: Toggles,
    last_error: Option<String>,
    last_sent_ms: Option<i64>,
}

/// Sends notifications to a Discord webhook.
#[derive(Debug)]
pub struct Notifier {
    inner: RwLock<Inner>,
    client: reqwest::Client,
}

impl Default for Notifier {
    fn default() -> Self {
        Self {
            inner: RwLock::default(),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .expect("HTTP client"),
        }
    }
}

/// Checks that `url` is a Discord webhook URL.
pub fn validate_webhook(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| "That isn't a URL.".to_owned())?;
    let host = parsed.host_str().unwrap_or_default();
    let discord = matches!(
        host,
        "discord.com" | "discordapp.com" | "canary.discord.com" | "ptb.discord.com"
    );
    if parsed.scheme() != "https" || !discord || !parsed.path().starts_with("/api/webhooks/") {
        return Err(
            "Paste a Discord webhook URL (https://discord.com/api/webhooks/…). Find it under the channel's Integrations settings."
                .to_owned(),
        );
    }
    if parsed.port().is_some() || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("That webhook URL has unexpected parts.".to_owned());
    }
    Ok(())
}

impl Notifier {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Loads the saved webhook and choices at startup.
    pub async fn load(&self, store: &Store, secrets: Option<&SecretBox>) {
        let Ok(Some(saved)) = store.get_setting::<Saved>(SETTINGS_KEY).await else {
            return;
        };
        let webhook = match (saved.webhook, secrets) {
            (Some(sealed), Some(secrets)) => match secrets.open(&sealed) {
                Ok(url) => Some(url),
                Err(e) => {
                    tracing::warn!(error = %e, "couldn't read the saved Discord webhook");
                    None
                }
            },
            _ => None,
        };
        let mut inner = self.write();
        inner.webhook = webhook;
        inner.toggles = saved.toggles;
    }

    /// The settings screen's view.
    pub fn status(&self) -> Status {
        let inner = self.read();
        Status {
            webhook_hint: inner.webhook.as_deref().map(hint),
            toggles: inner.toggles,
            last_error: inner.last_error.clone(),
            last_sent_ms: inner.last_sent_ms,
        }
    }

    /// Saves the choices, and the webhook when `webhook` is `Some` (empty
    /// removes it).
    pub(crate) async fn save(
        &self,
        store: &Store,
        secrets: Option<&SecretBox>,
        webhook: Option<&str>,
        toggles: Toggles,
    ) -> Result<(), ConnectError> {
        let current = store
            .get_setting::<Saved>(SETTINGS_KEY)
            .await
            .map_err(crate::rpc::store_error)?
            .unwrap_or_default();
        let (sealed, plain) = match webhook.map(str::trim) {
            None => (current.webhook, self.read().webhook.clone()),
            Some("") => (None, None),
            Some(url) => {
                validate_webhook(url).map_err(ConnectError::invalid_argument)?;
                let secrets = secrets.ok_or_else(|| {
                    ConnectError::unavailable(
                        "the data key isn't available, so the webhook can't be saved",
                    )
                })?;
                (Some(secrets.seal(url)), Some(url.to_owned()))
            }
        };
        store
            .set_setting(
                SETTINGS_KEY,
                &Saved {
                    webhook: sealed,
                    toggles,
                },
            )
            .await
            .map_err(crate::rpc::store_error)?;
        let mut inner = self.write();
        if inner.webhook != plain {
            inner.last_error = None;
            inner.last_sent_ms = None;
        }
        inner.webhook = plain;
        inner.toggles = toggles;
        Ok(())
    }

    /// Sends a test message now.
    pub(crate) async fn send_test(&self) -> Result<(), String> {
        let embed = json!({
            "title": "NetherNode notifications are working",
            "description": "Server events will be posted to this channel.",
            "color": COLOR_OK,
        });
        self.send(vec![embed]).await
    }

    async fn send(&self, embeds: Vec<serde_json::Value>) -> Result<(), String> {
        let Some(url) = self.read().webhook.clone() else {
            return Err("No webhook is set.".to_owned());
        };
        let result = post(&self.client, &url, &embeds).await;
        let mut inner = self.write();
        match &result {
            Ok(()) => {
                inner.last_error = None;
                inner.last_sent_ms = Some(nn_store::now_ms());
            }
            Err(e) => inner.last_error = Some(e.clone()),
        }
        result
    }
}

/// `…/1234/ab12` from a webhook URL: enough to recognize, not to use.
fn hint(url: &str) -> String {
    let tail: String = url
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

/// Posts one message, waiting out rate limits and retrying a server error once.
async fn post(
    client: &reqwest::Client,
    url: &str,
    embeds: &[serde_json::Value],
) -> Result<(), String> {
    let body =
        json!({ "username": "NetherNode", "embeds": embeds, "allowed_mentions": { "parse": [] } });
    let mut server_errors = 0;
    for _ in 0..4 {
        let response = client
            .post(url)
            .json(&body)
            .send()
            .await
            // Without the URL: its last part is the webhook's password.
            .map_err(|e| format!("Couldn't reach Discord: {}", e.without_url()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let header = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<f64>().ok());
            let body: serde_json::Value = response.json().await.unwrap_or_default();
            let wait = body
                .get("retry_after")
                .and_then(serde_json::Value::as_f64)
                .or(header)
                .unwrap_or(2.0)
                .clamp(0.1, 60.0);
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            continue;
        }
        if status.is_server_error() && server_errors == 0 {
            server_errors += 1;
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        return Err(match status.as_u16() {
            401 | 403 | 404 => {
                "Discord rejected the webhook. It may have been deleted; paste a new one."
                    .to_owned()
            }
            code => format!("Discord answered {code}."),
        });
    }
    Err("Discord kept asking to slow down; some notifications were dropped.".to_owned())
}

const COLOR_OK: u32 = 0x2f_b3_6b;
const COLOR_INFO: u32 = 0x5b_8d_ef;
const COLOR_MUTED: u32 = 0x80_80_80;
const COLOR_BAD: u32 = 0xe0_4b_3c;

/// What happened to one server during a batch.
#[derive(Debug, Default)]
struct ServerBatch {
    statuses: Vec<String>,
    joined: Vec<String>,
    failures: Vec<String>,
}

/// Turns a batch into Discord embeds, at most ten.
fn embeds(
    batch: &BTreeMap<String, ServerBatch>,
    names: &impl Fn(&str) -> String,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for (server_id, b) in batch {
        let name = names(server_id);
        let crashed = b.statuses.iter().any(|s| s == "crashed");
        let stopped = b.statuses.iter().position(|s| s == "stopped");
        let running = b.statuses.iter().rposition(|s| s == "running");
        if crashed {
            out.push(json!({ "title": format!("{name} crashed"), "description": "It stopped unexpectedly. Check its console for the cause.", "color": COLOR_BAD }));
        } else {
            match (stopped, running) {
                (Some(s), Some(r)) if s < r => {
                    out.push(json!({ "title": format!("{name} restarted"), "color": COLOR_OK }))
                }
                (_, Some(_)) => {
                    out.push(json!({ "title": format!("{name} is online"), "color": COLOR_OK }))
                }
                (Some(_), None) => {
                    out.push(json!({ "title": format!("{name} stopped"), "color": COLOR_MUTED }))
                }
                (None, None) => {}
            }
        }
        if !b.joined.is_empty() {
            let mut players = b.joined.clone();
            players.dedup();
            let list = if players.len() > 10 {
                format!(
                    "{} and {} others",
                    players[..10].join(", "),
                    players.len() - 10
                )
            } else {
                players.join(", ")
            };
            out.push(json!({ "title": format!("{list} joined {name}"), "color": COLOR_INFO }));
        }
        for failure in &b.failures {
            out.push(json!({ "title": format!("{name} backup failed"), "description": truncate(failure, 300), "color": COLOR_BAD }));
        }
    }
    out.truncate(MAX_EMBEDS);
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Starts sending notifications for bus events.
pub fn spawn(state: AppState) {
    let mut rx = state.events.subscribe();
    tokio::spawn(async move {
        let mut batch: BTreeMap<String, ServerBatch> = BTreeMap::new();
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let event = match deadline {
                Some(at) => tokio::select! {
                    e = rx.recv() => Some(e),
                    () = tokio::time::sleep_until(at) => None,
                },
                None => Some(rx.recv().await),
            };
            match event {
                Some(Ok(event)) => {
                    let toggles = state.notifier.read().toggles;
                    if state.notifier.read().webhook.is_some()
                        && note(&mut batch, &event, toggles)
                        && deadline.is_none()
                    {
                        deadline = Some(tokio::time::Instant::now() + BATCH);
                    }
                }
                Some(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Some(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return,
                None => {
                    deadline = None;
                    let pending = std::mem::take(&mut batch);
                    let names = server_names(&state).await;
                    let lookup = |id: &str| {
                        names
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| "A server".to_owned())
                    };
                    let embeds = embeds(&pending, &lookup);
                    if !embeds.is_empty()
                        && let Err(e) = state.notifier.send(embeds).await
                    {
                        tracing::warn!(error = %e, "couldn't send a Discord notification");
                    }
                }
            }
        }
    });
}

async fn server_names(state: &AppState) -> std::collections::HashMap<String, String> {
    state
        .auth
        .store()
        .list_servers()
        .await
        .map(|servers| servers.into_iter().map(|s| (s.id, s.name)).collect())
        .unwrap_or_default()
}

/// Adds `event` to the batch if it's wanted. Returns whether it was.
fn note(batch: &mut BTreeMap<String, ServerBatch>, event: &Event, toggles: Toggles) -> bool {
    match event {
        Event::ServerStatusChanged { server_id, status } => {
            let wanted = match status.as_str() {
                "running" | "stopped" => toggles.start_stop,
                "crashed" => toggles.crash,
                _ => false,
            };
            if wanted {
                batch
                    .entry(server_id.clone())
                    .or_default()
                    .statuses
                    .push(status.clone());
            }
            wanted
        }
        Event::PlayerJoined {
            server_id,
            username,
            ..
        } if toggles.player_join => {
            batch
                .entry(server_id.clone())
                .or_default()
                .joined
                .push(username.clone());
            true
        }
        Event::FileJobFinished {
            server_id,
            kind,
            message,
            failed: true,
            ..
        } if kind == "backup" && toggles.backup_failed => {
            batch
                .entry(server_id.clone())
                .or_default()
                .failures
                .push(message.clone());
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_urls_must_be_discord() {
        assert!(validate_webhook("https://discord.com/api/webhooks/123/abc").is_ok());
        assert!(validate_webhook("https://canary.discord.com/api/webhooks/123/abc").is_ok());
        for bad in [
            "http://discord.com/api/webhooks/123/abc",
            "https://discord.com.evil.example/api/webhooks/1/a",
            "https://evil.example/api/webhooks/1/a",
            "https://discord.com/api/users/1",
            "https://discord.com:8443/api/webhooks/1/a",
            "https://user@discord.com/api/webhooks/1/a",
            "not a url",
        ] {
            assert!(validate_webhook(bad).is_err(), "{bad}");
        }
    }

    fn batch_of(events: &[Event], toggles: Toggles) -> BTreeMap<String, ServerBatch> {
        let mut batch = BTreeMap::new();
        for e in events {
            note(&mut batch, e, toggles);
        }
        batch
    }

    fn status(id: &str, status: &str) -> Event {
        Event::ServerStatusChanged {
            server_id: id.into(),
            status: status.into(),
        }
    }

    fn join(id: &str, name: &str) -> Event {
        Event::PlayerJoined {
            server_id: id.into(),
            username: name.into(),
            uuid: None,
        }
    }

    #[test]
    fn batches_read_naturally() {
        let names = |id: &str| {
            if id == "a" {
                "SMP".to_owned()
            } else {
                "Creative".to_owned()
            }
        };
        let all = Toggles {
            player_join: true,
            ..Toggles::default()
        };
        let titles = |events: &[Event], toggles| {
            embeds(&batch_of(events, toggles), &names)
                .into_iter()
                .map(|e| e["title"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(
                &[
                    status("a", "stopping"),
                    status("a", "stopped"),
                    status("a", "starting"),
                    status("a", "running")
                ],
                all
            ),
            ["SMP restarted"]
        );
        assert_eq!(titles(&[status("b", "stopped")], all), ["Creative stopped"]);
        assert_eq!(
            titles(&[status("a", "running"), status("a", "crashed")], all),
            ["SMP crashed"]
        );
        assert_eq!(
            titles(
                &[
                    join("a", "Steve"),
                    join("a", "Alex"),
                    status("b", "running")
                ],
                all
            ),
            ["Steve, Alex joined SMP", "Creative is online"]
        );
        // Turned-off events don't appear.
        assert!(titles(&[join("a", "Steve")], Toggles::default()).is_empty());
        let off = Toggles {
            start_stop: false,
            ..Toggles::default()
        };
        assert!(titles(&[status("a", "running")], off).is_empty());
        let failed = Event::FileJobFinished {
            server_id: "a".into(),
            job_id: "j".into(),
            kind: "backup".into(),
            message: "disk full".into(),
            failed: true,
        };
        assert_eq!(titles(&[failed], all), ["SMP backup failed"]);
    }

    /// A stand-in for Discord that rate-limits the first request.
    async fn stub() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    ) {
        use axum::Json;
        use axum::routing::post;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (seen2, calls2) = (seen.clone(), calls.clone());
        let app = axum::Router::new().route(
            "/api/webhooks/1/x",
            post(move |Json(body): Json<serde_json::Value>| {
                let (seen, calls) = (seen2.clone(), calls2.clone());
                async move {
                    if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        return (
                            http::StatusCode::TOO_MANY_REQUESTS,
                            Json(json!({ "retry_after": 0.2 })),
                        );
                    }
                    seen.lock().unwrap().push(body);
                    (http::StatusCode::NO_CONTENT, Json(json!({})))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/api/webhooks/1/x"), seen)
    }

    #[tokio::test]
    async fn rate_limits_are_waited_out() {
        let (url, seen) = stub().await;
        let notifier = Notifier::default();
        notifier.write().webhook = Some(url);
        let started = std::time::Instant::now();
        notifier.send_test().await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            seen[0]["embeds"][0]["title"],
            "NetherNode notifications are working"
        );
        assert_eq!(seen[0]["allowed_mentions"]["parse"], json!([]));
        assert!(notifier.status().last_sent_ms.is_some());
    }

    #[tokio::test]
    async fn rejected_webhooks_are_reported() {
        let notifier = Notifier::default();
        assert!(notifier.send_test().await.is_err());
        // A closed port: the request fails and the error is kept for the UI.
        notifier.write().webhook = Some("http://127.0.0.1:9/api/webhooks/1/x".into());
        assert!(notifier.send_test().await.is_err());
        assert!(
            notifier
                .status()
                .last_error
                .unwrap()
                .contains("Couldn't reach Discord")
        );
    }
}
