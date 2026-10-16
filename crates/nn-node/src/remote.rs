//! A machine reached through its agent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use nn_events::Event;
use nn_link::messages;
use nn_link::{LinkStream, Session};
use nn_orchestrator::{HostInfo, Players, StatSample};
use serde::de::DeserializeOwned;
use tokio::sync::broadcast;

use crate::protocol::{ErrorKind, FolderOp, FolderReply, LogLine, RemoteError, Reply, Request};
use crate::{LogSubscription, NodeError, Result};

/// Bytes sent after a folder request.
pub(crate) enum Body<'a> {
    None,
    Bytes(&'a [u8]),
    /// A file on this machine, of this length.
    File(&'a std::path::Path, u64),
}

/// What the agent last reported about its machine.
#[derive(Debug, Clone)]
pub struct RemoteHost {
    /// Docker host facts, or why Docker isn't reachable there.
    pub docker: std::result::Result<HostInfo, String>,
    /// Total and free bytes on its data disk.
    pub disk: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Default)]
struct Cached {
    players: Option<Players>,
    stats: Option<StatSample>,
}

/// A node on another machine. It exists while the node does, connected or
/// not; while its agent is away, every request fails as offline.
pub struct RemoteNode {
    id: String,
    name: Mutex<String>,
    session: Mutex<Option<Session>>,
    host: Mutex<Option<RemoteHost>>,
    cache: Mutex<HashMap<String, Cached>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl RemoteNode {
    pub(crate) fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: Mutex::new(id.to_owned()),
            session: Mutex::default(),
            host: Mutex::default(),
            cache: Mutex::default(),
        }
    }

    /// The node's ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The node's name, for messages.
    pub fn name(&self) -> String {
        lock(&self.name).clone()
    }

    /// Updates the node's name.
    pub fn set_name(&self, name: &str) {
        *lock(&self.name) = name.to_owned();
    }

    /// The agent connected.
    pub fn attach(&self, session: Session) {
        *lock(&self.session) = Some(session);
    }

    /// The agent left (if `session` is still the current one).
    pub fn detach(&self) {
        *lock(&self.session) = None;
        lock(&self.cache).clear();
    }

    /// Whether the agent is connected.
    pub fn is_online(&self) -> bool {
        lock(&self.session).as_ref().is_some_and(|s| !s.is_closed())
    }

    /// The agent's latest report about its machine.
    pub fn set_host(&self, host: RemoteHost) {
        *lock(&self.host) = Some(host);
    }

    pub(crate) fn host(&self) -> Option<RemoteHost> {
        lock(&self.host).clone()
    }

    /// Keeps player counts and stats from the agent's events, for views that
    /// list many servers at once.
    pub fn apply(&self, event: &Event) {
        let mut cache = lock(&self.cache);
        match event {
            Event::ServerPlayers {
                server_id,
                online,
                max,
            } => {
                cache.entry(server_id.clone()).or_default().players = Some(Players {
                    online: *online,
                    max: *max,
                });
            }
            Event::ServerStats {
                server_id,
                cpu_percent,
                memory_bytes,
                memory_limit_bytes,
                net_rx_bytes,
                net_tx_bytes,
            } => {
                cache.entry(server_id.clone()).or_default().stats = Some(StatSample {
                    at_ms: nn_store::now_ms(),
                    cpu_percent: *cpu_percent,
                    memory_bytes: *memory_bytes,
                    memory_limit_bytes: *memory_limit_bytes,
                    net_rx_bytes: *net_rx_bytes,
                    net_tx_bytes: *net_tx_bytes,
                });
            }
            Event::ServerStatusChanged { server_id, status } if status != "running" => {
                cache.remove(server_id);
            }
            _ => {}
        }
    }

    pub(crate) fn players(&self, id: &str) -> Option<Players> {
        lock(&self.cache).get(id).and_then(|c| c.players)
    }

    pub(crate) fn latest_stats(&self, id: &str) -> Option<StatSample> {
        lock(&self.cache).get(id).and_then(|c| c.stats)
    }

    fn offline(&self) -> NodeError {
        NodeError::Offline(self.name())
    }

    async fn open(&self) -> Result<LinkStream> {
        let session = lock(&self.session).clone().ok_or_else(|| self.offline())?;
        session.open().await.map_err(|_| self.offline())
    }

    /// Sends a request and reads its reply.
    pub(crate) async fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T> {
        let mut stream = self.open().await?;
        let reply = self.exchange(&mut stream, &request).await?;
        serde_json::from_value(reply).map_err(|e| {
            NodeError::Remote(RemoteError {
                kind: ErrorKind::Internal,
                message: format!("the agent's reply didn't make sense: {e}"),
            })
        })
    }

    async fn exchange(
        &self,
        stream: &mut LinkStream,
        request: &Request,
    ) -> Result<serde_json::Value> {
        messages::send(stream, request)
            .await
            .map_err(|_| self.offline())?;
        let reply: Reply = messages::recv(stream)
            .await
            .map_err(|_| self.offline())?
            .ok_or_else(|| self.offline())?;
        reply.map_err(NodeError::Remote)
    }

    /// Opens a tunnel to a server's game port.
    pub(crate) async fn connect(&self, id: &str) -> std::io::Result<Box<dyn nn_proxy::Tunnel>> {
        let offline = |_| std::io::Error::new(std::io::ErrorKind::NotConnected, "offline");
        let mut stream = self.open().await.map_err(offline)?;
        self.exchange(&mut stream, &Request::Connect { id: id.to_owned() })
            .await
            .map_err(|e| match e {
                NodeError::Offline(_) => {
                    std::io::Error::new(std::io::ErrorKind::NotConnected, e.to_string())
                }
                other => std::io::Error::other(other.to_string()),
            })?;
        Ok(Box::new(stream))
    }

    /// Runs a folder operation, sending `body` after the request. Returns
    /// the reply and the stream, for operations that send bytes back.
    pub(crate) async fn folder(
        &self,
        id: &str,
        call: FolderOp,
        body: Body<'_>,
    ) -> Result<(LinkStream, serde_json::Value), nn_files::FileError> {
        use tokio::io::AsyncWriteExt;
        let offline =
            |_| nn_files::FileError::Io(std::io::Error::other(self.offline().to_string()));
        let mut stream = self.open().await.map_err(offline)?;
        let request = Request::Folder {
            id: id.to_owned(),
            call,
        };
        messages::send(&mut stream, &request).await?;
        match body {
            Body::None => {}
            Body::Bytes(bytes) => stream.write_all(bytes).await?,
            Body::File(path, len) => {
                let file = tokio::fs::File::open(path).await?;
                let sent =
                    tokio::io::copy(&mut tokio::io::AsyncReadExt::take(file, len), &mut stream)
                        .await?;
                if sent != len {
                    return Err(nn_files::FileError::Io(std::io::Error::other(
                        "the upload changed while it was being sent",
                    )));
                }
            }
        }
        stream.flush().await?;
        let reply: FolderReply = messages::recv(&mut stream).await?.ok_or_else(|| {
            nn_files::FileError::Io(std::io::Error::other(self.offline().to_string()))
        })?;
        let value = reply.map_err(nn_files::FileError::from)?;
        Ok((stream, value))
    }

    /// Follows a server's console.
    pub(crate) async fn logs(&self, id: &str) -> Result<LogSubscription> {
        let mut stream = self.open().await?;
        let backlog: Vec<String> = serde_json::from_value(
            self.exchange(&mut stream, &Request::Logs { id: id.to_owned() })
                .await?,
        )
        .unwrap_or_default();
        let (tx, lines) = broadcast::channel(1024);
        tokio::spawn(async move {
            loop {
                // Nobody's watching any more: stop following.
                if tx.receiver_count() == 0 {
                    break;
                }
                match messages::recv::<_, LogLine>(&mut stream).await {
                    Ok(Some(LogLine { line })) => {
                        let _ = tx.send(Arc::from(line));
                    }
                    _ => break,
                }
            }
        });
        Ok(LogSubscription {
            backlog: backlog.into_iter().map(Arc::from).collect(),
            lines,
        })
    }
}
