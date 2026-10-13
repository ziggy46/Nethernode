//! Client for Minecraft's server management protocol: JSON-RPC 2.0 over a
//! WebSocket, in Java Edition 1.21.9 and later. Authenticated with a
//! bearer secret; methods are positional (`"params": [[...]]`). The server
//! also pushes notifications when lists change or players come and go.
//!
//! Behavior measured on 26.3 (protocol 3.1.0): entries naming a UUID the
//! server can't verify (an offline UUID it has never seen) are dropped
//! without an error, so callers compare the returned list against what
//! they asked for. See <https://minecraft.wiki/w/Minecraft_Server_Management_Protocol>.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::profile::Profile;

/// Management protocol failures.
#[derive(Debug, thiserror::Error)]
pub enum MgmtError {
    /// Couldn't connect (wrong port, server too old, or not started).
    #[error("management connection failed: {0}")]
    Connect(String),
    /// The server rejected the secret.
    #[error("the management protocol rejected NetherNode's secret")]
    Unauthorized,
    /// The server returned a JSON-RPC error.
    #[error("the server refused: {message} ({code})")]
    Rpc {
        /// JSON-RPC error code.
        code: i64,
        /// Error text from the server.
        message: String,
    },
    /// No reply in time.
    #[error("the management protocol didn't answer in time")]
    Timeout,
    /// The connection dropped.
    #[error("the management connection closed")]
    Closed,
    /// A reply didn't have the expected shape.
    #[error("unexpected management reply: {0}")]
    Protocol(String),
}

type Result<T, E = MgmtError> = std::result::Result<T, E>;

/// A ban on an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserBan {
    /// Who.
    pub player: Profile,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Who placed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// ISO-8601 instant; `None` means permanent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

/// A ban on an IP address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpBan {
    /// The address.
    pub ip: String,
    /// Why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Who placed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// ISO-8601 instant; `None` means permanent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

/// An operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operator {
    /// Who.
    pub player: Profile,
    /// 1–4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_level: Option<u8>,
    /// May join when the server is full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypasses_player_limit: Option<bool>,
}

/// The server's state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServerState {
    /// Finished starting.
    pub started: bool,
    /// Online players.
    #[serde(default)]
    pub players: Vec<Profile>,
    /// Game version.
    pub version: Version,
}

/// Game version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Version {
    /// `26.3`.
    pub name: String,
    /// Protocol number.
    pub protocol: i64,
}

/// A push from the server, e.g. `minecraft:notification/players/joined`.
#[derive(Debug, Clone)]
pub struct Notification {
    /// Method name.
    pub method: String,
    /// Parameters as sent.
    pub params: Value,
}

type Sink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;
type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// An authenticated management protocol connection.
pub struct MgmtClient {
    sink: Mutex<Sink>,
    pending: Pending,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
    notifications: broadcast::Sender<Notification>,
    timeout: Duration,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for MgmtClient {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl MgmtClient {
    /// Connects to `addr` with `secret`. `timeout` bounds the connect and
    /// every call.
    pub async fn connect(addr: SocketAddr, secret: &str, timeout: Duration) -> Result<Self> {
        let mut request = format!("ws://{addr}")
            .into_client_request()
            .map_err(|e| MgmtError::Connect(e.to_string()))?;
        let auth = format!("Bearer {secret}")
            .parse()
            .map_err(|_| MgmtError::Connect("the secret isn't a valid header value".into()))?;
        request.headers_mut().insert("authorization", auth);
        let (stream, _) = tokio::time::timeout(timeout, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| MgmtError::Timeout)?
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(response)
                    if response.status() == 401 || response.status() == 403 =>
                {
                    MgmtError::Unauthorized
                }
                other => MgmtError::Connect(other.to_string()),
            })?;
        let (sink, mut source) = stream.split();
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (notifications, _) = broadcast::channel(256);

        let reader = {
            let (pending, closed, notifications) =
                (pending.clone(), closed.clone(), notifications.clone());
            tokio::spawn(async move {
                while let Some(Ok(message)) = source.next().await {
                    let text = match message {
                        Message::Text(text) => text,
                        Message::Close(_) => break,
                        _ => continue,
                    };
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if let Some(id) = value.get("id").and_then(Value::as_u64) {
                        let waiter = pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&id);
                        if let Some(waiter) = waiter {
                            let _ = waiter.send(reply(value));
                        }
                    } else if let Some(method) = value.get("method").and_then(Value::as_str) {
                        let _ = notifications.send(Notification {
                            method: method.to_owned(),
                            params: value.get("params").cloned().unwrap_or(Value::Null),
                        });
                    }
                }
                closed.store(true, Ordering::Release);
                for (_, waiter) in pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
                    let _ = waiter.send(Err(MgmtError::Closed));
                }
            })
        };

        Ok(Self {
            sink: Mutex::new(sink),
            pending,
            next_id: AtomicU64::new(1),
            closed,
            notifications,
            timeout,
            reader,
        })
    }

    /// Whether the connection has dropped.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Pushes from the server.
    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications.subscribe()
    }

    /// Calls `method` with positional `params` and returns the result.
    pub async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        if self.is_closed() {
            return Err(MgmtError::Closed);
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let sent = self
            .sink
            .lock()
            .await
            .send(Message::Text(request.to_string().into()))
            .await;
        if sent.is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(MgmtError::Closed);
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(MgmtError::Closed),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(MgmtError::Timeout)
            }
        }
    }

    async fn typed<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<T> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|e| MgmtError::Protocol(format!("{method}: {e}")))
    }

    /// Whether the server finished starting, who's online, and its version.
    pub async fn status(&self) -> Result<ServerState> {
        self.typed("minecraft:server/status", vec![]).await
    }

    /// Online players.
    pub async fn players(&self) -> Result<Vec<Profile>> {
        self.typed("minecraft:players", vec![]).await
    }

    /// Kicks `player` with `message`. Returns who was kicked.
    pub async fn kick(&self, player: &Profile, message: &str) -> Result<Vec<Profile>> {
        let kick = json!([{ "player": player, "message": { "literal": message } }]);
        self.typed("minecraft:players/kick", vec![kick]).await
    }

    /// The allowlist.
    pub async fn allowlist(&self) -> Result<Vec<Profile>> {
        self.typed("minecraft:allowlist", vec![]).await
    }

    /// Adds to the allowlist; returns the new list.
    pub async fn allowlist_add(&self, players: &[Profile]) -> Result<Vec<Profile>> {
        self.typed("minecraft:allowlist/add", vec![json!(players)])
            .await
    }

    /// Removes from the allowlist; returns the new list.
    pub async fn allowlist_remove(&self, players: &[Profile]) -> Result<Vec<Profile>> {
        self.typed("minecraft:allowlist/remove", vec![json!(players)])
            .await
    }

    /// Whether the allowlist is enforced on join.
    pub async fn allowlist_enabled(&self) -> Result<bool> {
        self.typed("minecraft:serversettings/use_allowlist", vec![])
            .await
    }

    /// Turns the allowlist on or off.
    pub async fn set_allowlist_enabled(&self, enabled: bool) -> Result<bool> {
        self.typed(
            "minecraft:serversettings/use_allowlist/set",
            vec![json!(enabled)],
        )
        .await
    }

    /// Account bans.
    pub async fn bans(&self) -> Result<Vec<UserBan>> {
        self.typed("minecraft:bans", vec![]).await
    }

    /// Adds a ban; returns the new list.
    pub async fn ban(&self, ban: &UserBan) -> Result<Vec<UserBan>> {
        self.typed("minecraft:bans/add", vec![json!([ban])]).await
    }

    /// Lifts a ban; returns the new list.
    pub async fn pardon(&self, player: &Profile) -> Result<Vec<UserBan>> {
        self.typed("minecraft:bans/remove", vec![json!([player])])
            .await
    }

    /// IP bans.
    pub async fn ip_bans(&self) -> Result<Vec<IpBan>> {
        self.typed("minecraft:ip_bans", vec![]).await
    }

    /// Bans an IP; returns the new list.
    pub async fn ban_ip(&self, ban: &IpBan) -> Result<Vec<IpBan>> {
        self.typed("minecraft:ip_bans/add", vec![json!([ban])])
            .await
    }

    /// Lifts an IP ban; returns the new list.
    pub async fn pardon_ip(&self, ip: &str) -> Result<Vec<IpBan>> {
        self.typed("minecraft:ip_bans/remove", vec![json!([ip])])
            .await
    }

    /// Operators.
    pub async fn operators(&self) -> Result<Vec<Operator>> {
        self.typed("minecraft:operators", vec![]).await
    }

    /// Makes an operator; returns the new list.
    pub async fn op(&self, operator: &Operator) -> Result<Vec<Operator>> {
        self.typed("minecraft:operators/add", vec![json!([operator])])
            .await
    }

    /// Removes an operator; returns the new list.
    pub async fn deop(&self, player: &Profile) -> Result<Vec<Operator>> {
        self.typed("minecraft:operators/remove", vec![json!([player])])
            .await
    }
}

/// A JSON-RPC response's result, or its error.
fn reply(value: Value) -> Result<Value> {
    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        return Err(MgmtError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_owned(),
        });
    }
    Ok(value.get("result").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    use super::*;

    /// A fake management server with an allowlist that, like 26.3, drops
    /// entries whose UUID it can't verify, and announces changes.
    // The handshake callback's error type is fixed by tungstenite.
    #[allow(clippy::result_large_err)]
    async fn fake_server(secret: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let check =
                        |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                            let ok = req
                                .headers()
                                .get("authorization")
                                .is_some_and(|v| v == format!("Bearer {secret}").as_str());
                            if ok {
                                Ok(resp)
                            } else {
                                let mut e = ErrorResponse::new(None);
                                *e.status_mut() =
                                    tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                                Err(e)
                            }
                        };
                    let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await else {
                        return;
                    };
                    let (mut tx, mut rx) = ws.split();
                    let mut allowlist: Vec<Value> = Vec::new();
                    while let Some(Ok(Message::Text(text))) = rx.next().await {
                        let req: Value = serde_json::from_str(&text).unwrap();
                        let id = req["id"].clone();
                        let reply = match req["method"].as_str().unwrap() {
                            "minecraft:allowlist/add" => {
                                for p in req["params"][0].as_array().unwrap() {
                                    if p["id"].as_str().is_some_and(|id| id.starts_with("0000")) {
                                        continue; // unverifiable: dropped silently
                                    }
                                    allowlist.push(p.clone());
                                }
                                let note = json!({ "jsonrpc": "2.0", "method": "minecraft:notification/allowlist/added", "params": [req["params"][0][0]] });
                                tx.send(Message::Text(note.to_string().into()))
                                    .await
                                    .unwrap();
                                json!({ "jsonrpc": "2.0", "id": id, "result": allowlist })
                            }
                            "minecraft:server/status" => {
                                json!({ "jsonrpc": "2.0", "id": id, "result": {
                                "started": true, "version": { "name": "26.3", "protocol": 777 } } })
                            }
                            "slow" => continue,
                            _ => {
                                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })
                            }
                        };
                        tx.send(Message::Text(reply.to_string().into()))
                            .await
                            .unwrap();
                    }
                });
            }
        });
        addr
    }

    const T: Duration = Duration::from_secs(2);

    fn jeb() -> Profile {
        Profile {
            uuid: "853c80ef-3c37-49fd-aa49-938b674adae6".into(),
            name: "jeb_".into(),
        }
    }

    #[tokio::test]
    async fn calls_and_notifications() {
        let addr = fake_server("s3cret").await;
        let client = MgmtClient::connect(addr, "s3cret", T).await.unwrap();
        let mut notes = client.notifications();
        let state = client.status().await.unwrap();
        assert_eq!((state.started, state.version.name.as_str()), (true, "26.3"));
        assert_eq!(client.allowlist_add(&[jeb()]).await.unwrap(), vec![jeb()]);
        let note = notes.recv().await.unwrap();
        assert_eq!(note.method, "minecraft:notification/allowlist/added");
        let unverifiable = Profile {
            uuid: "00000000-0000-3000-8000-000000000001".into(),
            name: "Ghost".into(),
        };
        let list = client
            .allowlist_add(std::slice::from_ref(&unverifiable))
            .await
            .unwrap();
        assert!(
            !list.contains(&unverifiable),
            "dropped entries are visible to callers"
        );
        assert!(matches!(
            client.call("minecraft:nope", vec![]).await,
            Err(MgmtError::Rpc { code: -32601, .. })
        ));
    }

    #[tokio::test]
    async fn wrong_secret_and_timeouts() {
        let addr = fake_server("s3cret").await;
        assert!(matches!(
            MgmtClient::connect(addr, "wrong", T).await,
            Err(MgmtError::Unauthorized)
        ));
        let client = MgmtClient::connect(addr, "s3cret", Duration::from_millis(200))
            .await
            .unwrap();
        assert!(matches!(
            client.call("slow", vec![]).await,
            Err(MgmtError::Timeout)
        ));
        let closed = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        assert!(matches!(
            MgmtClient::connect(closed, "s3cret", T).await,
            Err(MgmtError::Connect(_))
        ));
    }
}
