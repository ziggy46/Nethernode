//! Discord webhook notifications: servers starting, stopping, and crashing,
//! players joining, backups failing, and modpack updates.
//!
//! Everything sent comes from events on the bus, and [`Toggles`] holds the
//! choice of which, so another way of being notified can listen the same
//! way.
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
    /// Newer modpack versions, and modpack updates finishing or failing.
    pub modpack_updates: bool,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            start_stop: true,
            crash: true,
            player_join: false,
            backup_failed: true,
            modpack_updates: true,
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

    /// Sends a test message now: that it works, and how messages look.
    pub(crate) async fn send_test(&self) -> Result<(), String> {
        let now = jiff::Timestamp::now().to_string();
        let mut out = vec![json!({
            "title": "✅ NetherNode notifications are working",
            "description": "The events chosen in Settings › Notifications are posted here. Below are examples of how they look; nothing happened to your servers.",
            "color": COLOR_OK,
            "timestamp": now,
        })];
        let example = About {
            name: "Survival".into(),
            software: "Paper 1.21.4".into(),
            max_players: 20,
            address: Some("survival.example.com".into()),
            restarts_on_crash: false,
            console: vec![
                "[12:01:44 INFO]: Steve fell from a high place".into(),
                "[12:02:10 ERROR]: Encountered an unexpected exception".into(),
                "java.lang.OutOfMemoryError: Java heap space".into(),
            ],
            ..About::default()
        };
        let mut batch = BTreeMap::new();
        batch.insert(
            "1".to_owned(),
            ServerBatch {
                statuses: vec!["running".into()],
                joined: vec![(
                    "Steve".into(),
                    Some("8667ba71-b85a-4004-af54-457a9734eed7".into()),
                )],
                ..ServerBatch::default()
            },
        );
        batch.insert(
            "2".to_owned(),
            ServerBatch {
                statuses: vec!["crashed".into()],
                ..ServerBatch::default()
            },
        );
        let examples = embeds(&batch, &|_| example.clone(), &now);
        out.extend(examples.into_iter().map(|mut e| {
            e["footer"] = json!({ "text": "Example" });
            e
        }));
        self.send(out).await
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
    let body = json!({
        "username": "NetherNode",
        "avatar_url": AVATAR,
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    });
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

/// The bot's picture: NetherNode's icon, from the public repository.
const AVATAR: &str =
    "https://raw.githubusercontent.com/ziggy46/Nethernode/main/web/public/icon-192.png";
/// Player heads by UUID, for joins.
const HEADS: &str = "https://mc-heads.net/avatar";

const COLOR_OK: u32 = 0x2f_b3_6b;
const COLOR_INFO: u32 = 0x5b_8d_ef;
const COLOR_MUTED: u32 = 0x80_80_80;
const COLOR_BAD: u32 = 0xe0_4b_3c;

/// Something about a server's modpack.
#[derive(Debug)]
enum PackNote {
    Available { pack: String, version: String },
    Updated(String),
    Failed(String),
}

/// What happened to one server during a batch.
#[derive(Debug, Default)]
struct ServerBatch {
    statuses: Vec<String>,
    /// Usernames, with UUIDs when known (for their heads).
    joined: Vec<(String, Option<String>)>,
    failures: Vec<String>,
    packs: Vec<PackNote>,
}

/// What messages say about a server, looked up when a batch goes out.
#[derive(Debug, Clone, Default)]
struct About {
    name: String,
    /// Like "Paper 1.21.4".
    software: String,
    max_players: i64,
    /// Where players join, when a route gives it one.
    address: Option<String>,
    /// Its machine, when it isn't the panel's own.
    machine: Option<String>,
    restarts_on_crash: bool,
    /// The end of its console, for crashes.
    console: Vec<String>,
}

fn field(name: &str, value: impl Into<String>) -> serde_json::Value {
    json!({ "name": name, "value": value.into(), "inline": true })
}

/// The facts shown when a server comes online.
fn server_fields(about: &About) -> Vec<serde_json::Value> {
    let mut fields = vec![
        field("Version", about.software.clone()),
        field("Players", format!("Up to {}", about.max_players)),
    ];
    if let Some(machine) = &about.machine {
        fields.push(field("Machine", machine.clone()));
    }
    fields
}

/// The end of a console as a code block that fits an embed.
fn console_block(lines: &[String]) -> Option<String> {
    let mut text = String::new();
    for line in lines.iter().rev() {
        // A fence in a line would end the block early.
        let line = line.replace("```", "`\u{200b}``");
        if text.len() + line.len() > 900 {
            break;
        }
        text = format!("{line}\n{text}");
    }
    let text = text.trim_end();
    (!text.is_empty()).then(|| format!("```\n{text}\n```"))
}

/// Turns a batch into Discord embeds, at most ten.
fn embeds(
    batch: &BTreeMap<String, ServerBatch>,
    about: &impl Fn(&str) -> About,
    now: &str,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for (server_id, b) in batch {
        let about = about(server_id);
        let name = &about.name;
        let crashed = b.statuses.iter().any(|s| s == "crashed");
        let stopped = b.statuses.iter().position(|s| s == "stopped");
        let running = b.statuses.iter().rposition(|s| s == "running");
        let join_line = about
            .address
            .as_ref()
            .map(|a| format!("Join at **{a}**"))
            .unwrap_or_else(|| "Players can join now.".into());
        if crashed {
            let what = if about.restarts_on_crash {
                "It stopped unexpectedly; NetherNode is starting it again."
            } else {
                "It stopped unexpectedly and wasn't restarted. Its console in the panel has the whole story."
            };
            let description = match console_block(&about.console) {
                Some(block) => format!("{what}\n\nThe last lines it printed:\n{block}"),
                None => what.to_owned(),
            };
            out.push(json!({ "title": format!("💥 {name} crashed"), "description": description, "color": COLOR_BAD }));
        } else {
            match (stopped, running) {
                (Some(s), Some(r)) if s < r => out.push(json!({
                    "title": format!("🔄 {name} restarted"),
                    "description": join_line,
                    "fields": server_fields(&about),
                    "color": COLOR_OK,
                })),
                (_, Some(_)) => out.push(json!({
                    "title": format!("🟢 {name} is online"),
                    "description": join_line,
                    "fields": server_fields(&about),
                    "color": COLOR_OK,
                })),
                (Some(_), None) => out.push(json!({
                    "title": format!("⏹️ {name} stopped"),
                    "color": COLOR_MUTED,
                })),
                (None, None) => {}
            }
        }
        if !b.joined.is_empty() {
            let mut players = b.joined.clone();
            players.dedup_by(|a, b| a.0 == b.0);
            let mut embed = if let [(player, uuid)] = players.as_slice() {
                let mut embed =
                    json!({ "title": format!("👋 {player} joined {name}"), "color": COLOR_INFO });
                if let Some(uuid) = uuid.as_deref().filter(|u| is_uuid(u)) {
                    embed["thumbnail"] =
                        json!({ "url": format!("{HEADS}/{}/64", uuid.replace('-', "")) });
                }
                embed
            } else {
                let names: Vec<&str> = players.iter().map(|(n, _)| n.as_str()).collect();
                let list = if names.len() > 15 {
                    format!("{} and {} more", names[..15].join(", "), names.len() - 15)
                } else {
                    names.join(", ")
                };
                json!({ "title": format!("👋 {} players joined {name}", names.len()), "description": list, "color": COLOR_INFO })
            };
            embed["timestamp"] = json!(now);
            out.push(embed);
        }
        for failure in &b.failures {
            out.push(json!({ "title": format!("💾 A backup of {name} failed"), "description": truncate(failure, 300), "color": COLOR_BAD }));
        }
        for note in &b.packs {
            out.push(match note {
                PackNote::Available { pack, version } => json!({ "title": format!("📦 {pack} {version} is out"), "description": format!("A newer version of {name}'s modpack. Update it from the server's Mods tab; a backup is made first."), "color": COLOR_INFO }),
                PackNote::Updated(message) => json!({ "title": format!("✅ {name}'s modpack was updated"), "description": truncate(message, 300), "color": COLOR_OK }),
                PackNote::Failed(message) => json!({ "title": format!("⚠️ {name}'s modpack update failed"), "description": truncate(message, 300), "color": COLOR_BAD }),
            });
        }
    }
    for embed in &mut out {
        if embed.get("timestamp").is_none() {
            embed["timestamp"] = json!(now);
        }
    }
    out.truncate(MAX_EMBEDS);
    out
}

fn is_uuid(s: &str) -> bool {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit())
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
                    let abouts = abouts(&state, &pending).await;
                    let lookup = |id: &str| {
                        abouts.get(id).cloned().unwrap_or_else(|| About {
                            name: "A server".to_owned(),
                            ..About::default()
                        })
                    };
                    let now = jiff::Timestamp::now().to_string();
                    let embeds = embeds(&pending, &lookup, &now);
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

/// What the batch's messages say about each of its servers.
async fn abouts(
    state: &AppState,
    batch: &BTreeMap<String, ServerBatch>,
) -> std::collections::HashMap<String, About> {
    let store = state.auth.store();
    let servers = store.list_servers().await.unwrap_or_default();
    let routes = store.list_routes().await.unwrap_or_default();
    let nodes = store.list_nodes().await.unwrap_or_default();
    let mut out = std::collections::HashMap::new();
    for server in servers.into_iter().filter(|s| batch.contains_key(&s.id)) {
        let exact = routes
            .iter()
            .find(|r| r.server_id.as_deref() == Some(&server.id) && !r.hostname.contains('*'))
            .map(|r| r.hostname.clone());
        let by_slug = routes
            .iter()
            .filter(|r| r.server_id.is_none())
            .find_map(|r| r.hostname.strip_prefix("*."))
            .map(|domain| format!("{}.{domain}", server.slug));
        let machine = (server.node_id != nn_store::LOCAL_NODE)
            .then(|| nodes.iter().find(|n| n.id == server.node_id))
            .flatten()
            .map(|n| n.name.clone());
        let crashed = batch[&server.id].statuses.iter().any(|s| s == "crashed");
        let console = match crashed {
            true => match state
                .nodes
                .for_server(&server)
                .subscribe_logs(&server.id)
                .await
            {
                Ok(logs) => {
                    let skip = logs.backlog.len().saturating_sub(12);
                    logs.backlog
                        .iter()
                        .skip(skip)
                        .map(|l| strip_ansi(l))
                        .collect()
                }
                Err(_) => Vec::new(),
            },
            false => Vec::new(),
        };
        out.insert(
            server.id.clone(),
            About {
                software: software(&server),
                max_players: server.max_players,
                address: exact.or(by_slug),
                machine,
                restarts_on_crash: server.restart_on_crash,
                console,
                name: server.name,
            },
        );
    }
    out
}

/// "Paper 1.21.4", or the modpack and its version.
fn software(server: &nn_store::Server) -> String {
    if let Some(pack) = crate::modpack_updates::pack_of(server) {
        return match pack.version_name {
            Some(v) => format!("{} {v}", pack.title),
            None => pack.title,
        };
    }
    let loader = match server.loader.as_str() {
        "neoforge" => "NeoForge".to_owned(),
        other => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    };
    format!("{loader} {}", server.mc_version)
}

/// A console line without its color codes.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
        } else if chars.peek() == Some(&'[') {
            // ESC [ … ends at a letter.
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
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
            uuid,
        } if toggles.player_join => {
            batch
                .entry(server_id.clone())
                .or_default()
                .joined
                .push((username.clone(), uuid.clone()));
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
        Event::FileJobFinished {
            server_id,
            kind,
            message,
            failed,
            ..
        } if kind == "modpack-update" && toggles.modpack_updates => {
            let note = if *failed {
                PackNote::Failed(message.clone())
            } else {
                PackNote::Updated(message.clone())
            };
            batch.entry(server_id.clone()).or_default().packs.push(note);
            true
        }
        Event::ModpackUpdateAvailable {
            server_id,
            pack,
            version,
        } if toggles.modpack_updates => {
            batch
                .entry(server_id.clone())
                .or_default()
                .packs
                .push(PackNote::Available {
                    pack: pack.clone(),
                    version: version.clone(),
                });
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
        let names = |id: &str| About {
            name: if id == "a" { "SMP" } else { "Creative" }.to_owned(),
            ..About::default()
        };
        let all = Toggles {
            player_join: true,
            ..Toggles::default()
        };
        let titles = |events: &[Event], toggles| {
            embeds(&batch_of(events, toggles), &names, "2026-10-20T12:00:00Z")
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
            ["🔄 SMP restarted"]
        );
        assert_eq!(
            titles(&[status("b", "stopped")], all),
            ["⏹️ Creative stopped"]
        );
        assert_eq!(
            titles(&[status("a", "running"), status("a", "crashed")], all),
            ["💥 SMP crashed"]
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
            ["👋 2 players joined SMP", "🟢 Creative is online"]
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
        assert_eq!(titles(&[failed], all), ["💾 A backup of SMP failed"]);

        let pack_job = |failed| Event::FileJobFinished {
            server_id: "a".into(),
            job_id: "j".into(),
            kind: "modpack-update".into(),
            message: "msg".into(),
            failed,
        };
        let available = Event::ModpackUpdateAvailable {
            server_id: "a".into(),
            pack: "Pack".into(),
            version: "2.0".into(),
        };
        assert_eq!(
            titles(&[available.clone(), pack_job(false), pack_job(true)], all),
            [
                "📦 Pack 2.0 is out",
                "✅ SMP's modpack was updated",
                "⚠️ SMP's modpack update failed"
            ]
        );
        let off = Toggles {
            modpack_updates: false,
            ..Toggles::default()
        };
        assert!(titles(&[available, pack_job(true)], off).is_empty());
    }

    #[test]
    fn messages_carry_the_details() {
        let about = |_: &str| About {
            name: "SMP".into(),
            software: "Paper 1.21.4".into(),
            max_players: 20,
            address: Some("smp.example.com".into()),
            machine: Some("Node two".into()),
            restarts_on_crash: false,
            console: vec![
                "\u{1b}[31mjava.lang.OutOfMemoryError\u{1b}[0m".into(),
                "```".into(),
            ],
        };
        let now = "2026-10-20T12:00:00Z";
        let online = &embeds(
            &batch_of(&[status("a", "running")], Toggles::default()),
            &about,
            now,
        )[0];
        assert_eq!(online["description"], "Join at **smp.example.com**");
        let fields: Vec<&str> = online["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["value"].as_str().unwrap())
            .collect();
        assert_eq!(fields, ["Paper 1.21.4", "Up to 20", "Node two"]);
        assert_eq!(online["timestamp"], now);

        let mut about_crash = about("a");
        about_crash.console = about_crash.console.iter().map(|l| strip_ansi(l)).collect();
        let crashed = &embeds(
            &batch_of(&[status("a", "crashed")], Toggles::default()),
            &|_| about_crash.clone(),
            now,
        )[0];
        let text = crashed["description"].as_str().unwrap();
        assert!(text.contains("java.lang.OutOfMemoryError\n"), "{text}");
        assert!(!text.contains('\u{1b}'), "color codes are gone");
        assert_eq!(
            text.matches("```").count(),
            2,
            "a fence in the console can't end the block: {text}"
        );

        let all = Toggles {
            player_join: true,
            ..Toggles::default()
        };
        let joined = Event::PlayerJoined {
            server_id: "a".into(),
            username: "Steve".into(),
            uuid: Some("8667ba71-b85a-4004-af54-457a9734eed7".into()),
        };
        let one = &embeds(&batch_of(&[joined], all), &about, now)[0];
        assert_eq!(one["title"], "👋 Steve joined SMP");
        assert_eq!(
            one["thumbnail"]["url"],
            "https://mc-heads.net/avatar/8667ba71b85a4004af54457a9734eed7/64"
        );
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
            "✅ NetherNode notifications are working"
        );
        assert_eq!(seen[0]["avatar_url"], AVATAR);
        let examples = seen[0]["embeds"].as_array().unwrap();
        assert!(examples.len() > 1, "examples follow");
        assert!(
            examples[1..]
                .iter()
                .all(|e| e["footer"]["text"] == "Example")
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
