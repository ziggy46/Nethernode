//! Agents on other machines: the panel's end of their links.
//!
//! An agent dials `/api/agent` (a WebSocket), proves it holds a joined
//! node's key, or spends a join code, and then keeps a control stream open,
//! sending a heartbeat every ten seconds. A node is online while its link
//! is up; three missed heartbeats close it.

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use http::StatusCode;
use nn_auth::SecretBox;
use nn_events::{Event, EventBus};
use nn_link::messages::{self, AgentMessage, HostReport};
use nn_link::{Handshaken, Keypair, PROTOCOL, Session};
use nn_store::{NewAuditEntry, Node, Store};
use serde_json::json;
use tokio::sync::Semaphore;

use crate::{AppState, RequestMeta};

/// Where the panel's key pair is kept, sealed with the data key.
const KEY_SETTING: &str = "nodes.panel_key";
/// How long an agent has to finish the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Handshakes in progress at once; more are turned away.
const MAX_HANDSHAKES: usize = 16;
/// Silence after which a link is closed (three missed heartbeats).
const SILENCE: Duration = Duration::from_secs(30);

/// A connected agent.
#[derive(Clone)]
pub struct Live {
    /// The agent's version.
    pub version: String,
    /// Its machine, from the latest heartbeat.
    pub host: Option<HostReport>,
    session: Session,
    generation: u64,
}

/// The panel's links to agents.
pub struct Agents {
    store: Option<Store>,
    events: EventBus,
    key: Option<Keypair>,
    unavailable: Option<String>,
    links: Mutex<HashMap<String, Live>>,
    generation: std::sync::atomic::AtomicU64,
    handshakes: Semaphore,
}

impl Default for Agents {
    /// Agents can't connect (no data key to keep the panel's key with).
    fn default() -> Self {
        Self::unavailable(EventBus::default(), "the data key couldn't be loaded")
    }
}

impl Agents {
    fn unavailable(events: EventBus, reason: &str) -> Self {
        Self {
            store: None,
            events,
            key: None,
            unavailable: Some(reason.to_owned()),
            links: Mutex::default(),
            generation: Default::default(),
            handshakes: Semaphore::new(MAX_HANDSHAKES),
        }
    }

    /// Loads the panel's key pair, making one the first time.
    pub async fn load(store: Store, events: EventBus, secrets: Option<&SecretBox>) -> Self {
        let Some(secrets) = secrets else {
            return Self::default();
        };
        let key = match panel_key(&store, secrets).await {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!(error = %e, "the panel's node key couldn't be loaded; agents can't connect");
                return Self::unavailable(events, "the panel's node key couldn't be loaded");
            }
        };
        Self {
            store: Some(store),
            events,
            key: Some(key),
            unavailable: None,
            links: Mutex::default(),
            generation: Default::default(),
            handshakes: Semaphore::new(MAX_HANDSHAKES),
        }
    }

    /// Why agents can't connect, if they can't.
    pub fn unavailable_reason(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    /// The panel's public key, for join tokens.
    pub fn public_key(&self) -> Option<[u8; 32]> {
        self.key.as_ref().map(|k| k.public)
    }

    /// The node's agent, while it's connected.
    pub fn live(&self, node_id: &str) -> Option<Live> {
        self.lock().get(node_id).cloned()
    }

    /// Disconnects a node's agent (the node was removed).
    pub fn disconnect(&self, node_id: &str) {
        if let Some(live) = self.lock().remove(node_id) {
            live.session.close();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Live>> {
        self.links.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs one agent connection to its end.
    async fn serve(self: Arc<Self>, frames: nn_link::Frames, ip: Option<String>) {
        let (Some(store), Some(key)) = (self.store.clone(), self.key.as_ref()) else {
            return;
        };
        let Ok(permit) = self.handshakes.try_acquire() else {
            tracing::warn!(?ip, "too many agents connecting at once; turned one away");
            return;
        };
        let shaken =
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, nn_link::accept(frames, key)).await {
                Ok(Ok(shaken)) => shaken,
                Ok(Err(e)) => {
                    tracing::info!(?ip, error = %e, "an agent's handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::info!(?ip, "an agent's handshake timed out");
                    return;
                }
            };
        let Some(node) = self.identify(&store, shaken, ip.as_deref()).await else {
            return;
        };
        drop(permit);
        self.run(&store, node).await;
    }

    /// Finds the agent's node, or lets it join with its code. Rejects it
    /// (and returns `None`) otherwise.
    async fn identify(
        &self,
        store: &Store,
        shaken: Handshaken,
        ip: Option<&str>,
    ) -> Option<Admitted> {
        if shaken.hello.protocol != PROTOCOL {
            shaken
                .reject("this agent's NetherNode version can't talk to this panel; run the same version as the panel")
                .await;
            return None;
        }
        let agent_key = hex::encode(shaken.agent_key);
        let known = match store.find_node_by_key(&agent_key).await {
            Ok(found) => found,
            Err(e) => {
                tracing::error!(error = %e, "couldn't look up an agent");
                shaken
                    .reject("the panel couldn't check this agent; try again")
                    .await;
                return None;
            }
        };
        let node = match (known, &shaken.hello.join) {
            (Some(node), _) => node,
            (None, Some(proof)) => {
                let hash = nn_link::hash_secret(&proof.secret);
                let now = nn_store::now_ms();
                match store
                    .claim_join_code(&proof.code_id, &hash, &agent_key, now)
                    .await
                {
                    Ok(Some(node)) => {
                        audit(store, "node.join", &node.id, json!({ "ip": ip })).await;
                        tracing::info!(node = %node.id, name = %node.name, "an agent joined");
                        node
                    }
                    other => {
                        if let Err(e) = other {
                            tracing::error!(error = %e, "couldn't check a join code");
                        }
                        audit(
                            store,
                            "node.join_failed",
                            &proof.code_id,
                            json!({ "ip": ip }),
                        )
                        .await;
                        shaken
                            .reject("the join code is wrong, expired, or already used; make a new one on the panel's Nodes page")
                            .await;
                        return None;
                    }
                }
            }
            (None, None) => {
                shaken
                    .reject("this panel doesn't know this agent (it may have been removed); add it again from the Nodes page")
                    .await;
                return None;
            }
        };
        let version = shaken.hello.version.clone();
        match shaken.admit(&node.id, &node.name).await {
            Ok((session, inbound)) => Some(Admitted {
                node,
                version,
                session,
                inbound,
            }),
            Err(e) => {
                tracing::info!(error = %e, "an agent left while being let in");
                None
            }
        }
    }

    /// Serves an admitted agent until its link ends.
    async fn run(&self, store: &Store, admitted: Admitted) {
        let Admitted {
            node,
            version,
            session,
            mut inbound,
        } = admitted;
        let generation = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let live = Live {
            version: version.clone(),
            host: None,
            session: session.clone(),
            generation,
        };
        // A newer connection replaces an older one.
        if let Some(old) = self.lock().insert(node.id.clone(), live) {
            old.session.close();
        }
        let _ = store
            .node_seen(&node.id, &version, None, nn_store::now_ms())
            .await;
        self.publish(&node.id, true);
        tracing::info!(node = %node.id, %version, "an agent connected");

        if let Ok(Some(mut control)) = tokio::time::timeout(SILENCE, inbound.next()).await {
            loop {
                let message =
                    tokio::time::timeout(SILENCE, messages::recv::<_, AgentMessage>(&mut control))
                        .await;
                match message {
                    Ok(Ok(Some(AgentMessage::Heartbeat { host }))) => {
                        let json = serde_json::to_string(&host).ok();
                        if let Some(live) = self.lock().get_mut(&node.id)
                            && live.generation == generation
                        {
                            live.host = Some(host);
                        }
                        let _ = store
                            .node_seen(&node.id, &version, json.as_deref(), nn_store::now_ms())
                            .await;
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(e)) => {
                        tracing::info!(node = %node.id, error = %e, "an agent's link broke");
                        break;
                    }
                    Err(_) => {
                        tracing::info!(node = %node.id, "an agent went quiet; closing its link");
                        break;
                    }
                }
            }
        }
        session.close();
        let mut links = self.lock();
        if links
            .get(&node.id)
            .is_some_and(|l| l.generation == generation)
        {
            links.remove(&node.id);
            drop(links);
            self.publish(&node.id, false);
            tracing::info!(node = %node.id, "an agent disconnected");
        }
    }

    fn publish(&self, node_id: &str, online: bool) {
        self.events.publish(Event::NodeChanged {
            node_id: node_id.to_owned(),
            online,
        });
    }
}

struct Admitted {
    node: Node,
    version: String,
    session: Session,
    inbound: nn_link::Inbound,
}

async fn audit(store: &Store, action: &str, target: &str, details: serde_json::Value) {
    let entry = NewAuditEntry {
        user_id: None,
        action,
        target_type: Some("node"),
        target_id: Some(target),
        details: Some(details),
    };
    if let Err(e) = store.insert_audit(entry).await {
        tracing::warn!(error = %e, action, "failed to write audit log entry");
    }
}

async fn panel_key(store: &Store, secrets: &SecretBox) -> Result<Keypair, String> {
    if let Some(sealed) = store
        .get_setting::<String>(KEY_SETTING)
        .await
        .map_err(|e| e.to_string())?
    {
        let json = secrets.open(&sealed).map_err(|e| e.to_string())?;
        return serde_json::from_str(&json).map_err(|e| e.to_string());
    }
    let key = Keypair::generate();
    let json = serde_json::to_string(&key).map_err(|e| e.to_string())?;
    store
        .set_setting(KEY_SETTING, &secrets.seal(&json))
        .await
        .map_err(|e| e.to_string())?;
    Ok(key)
}

/// `GET /api/agent`: where agents connect.
pub(crate) async fn connect(
    State(state): State<AppState>,
    meta: Option<Extension<RequestMeta>>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(reason) = state.agents.unavailable_reason() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("agents can't connect: {reason}"),
        )
            .into_response();
    }
    let ip = meta.and_then(|Extension(m)| m.ip);
    let agents = state.agents.clone();
    ws.max_message_size(70_000)
        .on_upgrade(move |socket| agents.serve(Box::new(WsFrames(socket)), ip))
}

/// An axum WebSocket as link frames: binary messages only.
struct WsFrames(WebSocket);

impl Stream for WsFrames {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.0.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(bytes))) => Poll::Ready(Some(Ok(bytes))),
                Some(Ok(Message::Close(_))) | None => Poll::Ready(None),
                // Pings are answered by axum; nothing else is expected.
                Some(Ok(_)) => continue,
                Some(Err(e)) => Poll::Ready(Some(Err(io::Error::other(e)))),
            };
        }
    }
}

impl Sink<Bytes> for WsFrames {
    type Error = io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.poll_ready_unpin(cx).map_err(io::Error::other)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Bytes) -> io::Result<()> {
        self.0
            .start_send_unpin(Message::Binary(item))
            .map_err(io::Error::other)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.poll_flush_unpin(cx).map_err(io::Error::other)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.poll_close_unpin(cx).map_err(io::Error::other)
    }
}

/// Whether an agent on `agent` can run servers for a panel on `panel`:
/// the same major and minor version.
pub(crate) fn compatible(panel: &str, agent: &str) -> bool {
    let minor = |v: &str| {
        let mut parts = v.split('.');
        (
            parts.next().map(str::to_owned),
            parts.next().map(str::to_owned),
        )
    };
    minor(panel) == minor(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_match_on_major_and_minor() {
        assert!(compatible("0.2.0", "0.2.5"));
        assert!(!compatible("0.2.0", "0.3.0"));
        assert!(!compatible("1.2.0", "0.2.0"));
    }
}
