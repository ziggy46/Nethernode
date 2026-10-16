//! The agent's end of the link to its panel.
//!
//! The first run joins with a code from the panel and saves what it needs
//! to come back (`agent.json`: the panel's address and key, the agent's own
//! key pair, its node). Every run after that reconnects on its own and
//! keeps the link up, sending a heartbeat every ten seconds.

use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nn_events::Event;
use nn_link::messages::{self, AgentMessage, Hello, HostReport, JoinProof, StreamKind, Welcome};
use nn_link::{Inbound, JoinToken, Keypair, LinkError, LinkStream, PROTOCOL, Session};
use nn_store::Server;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::Node;
use crate::protocol::{
    AgentEvent, FileFailure, FileHeader, FolderOp, FolderReply, LogLine, RemoteError, Reply,
    Request,
};
use nn_files::FileError;

const STATE_FILE: &str = "agent.json";
/// Wait after the panel turns a joined agent away before asking again.
const REJECTED_WAIT: Duration = Duration::from_secs(600);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A link up this long counts as healthy: the next drop retries at once.
const STABLE: Duration = Duration::from_secs(60);

/// What a joined agent remembers.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// The panel's address (`https://…`).
    pub panel_url: String,
    /// The panel's public key, pinned at join.
    #[serde(with = "hex32")]
    pub panel_key: [u8; 32],
    /// This agent's key pair.
    pub key: Keypair,
    /// The node this agent is.
    pub node_id: String,
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(d)?;
        hex::decode(&text)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| serde::de::Error::custom("expected 32 hex-encoded bytes"))
    }
}

impl AgentState {
    /// The saved state in `dir`, if this agent has joined.
    pub fn load(dir: &Path) -> io::Result<Option<Self>> {
        match std::fs::read(dir.join(STATE_FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Saves the state in `dir`, readable only by this user.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let tmp = dir.join(format!("{STATE_FILE}.tmp"));
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        io::Write::write_all(&mut file, &json)?;
        file.sync_all()?;
        std::fs::rename(&tmp, dir.join(STATE_FILE))
    }
}

/// How the agent was started.
pub struct AgentArgs {
    /// Where `agent.json` lives (the agent's data folder).
    pub dir: PathBuf,
    /// `--join`: the panel's address, for the first run.
    pub panel_url: Option<String>,
    /// `--code`: the join token, for the first run.
    pub code: Option<String>,
    /// This NetherNode's version.
    pub version: String,
}

/// Why the agent gave up.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// It can't start without being told where to join.
    #[error("{0}")]
    Usage(String),
    /// Joining failed in a way retrying won't fix.
    #[error("{0}")]
    Join(String),
    /// Its saved state can't be read or written.
    #[error("the agent's state in {dir}: {error}")]
    State {
        /// The data folder.
        dir: String,
        /// What went wrong.
        error: io::Error,
    },
}

/// Runs the agent: joins if it hasn't, then keeps its link to the panel up
/// for good, reporting `node`'s machine.
pub async fn run(args: AgentArgs, node: Node) -> Result<(), AgentError> {
    let state_error = |error| AgentError::State {
        dir: args.dir.display().to_string(),
        error,
    };
    let mut state = AgentState::load(&args.dir).map_err(state_error)?;
    let mut joining = None;
    match (&state, &args.panel_url, &args.code) {
        (Some(saved), Some(url), _) if normalize(url) != normalize(&saved.panel_url) => {
            return Err(AgentError::Usage(format!(
                "this agent already joined {}; to join another panel, start it with an empty data folder",
                saved.panel_url
            )));
        }
        (Some(saved), Some(_), _) | (Some(saved), _, Some(_)) => {
            tracing::info!(panel = %saved.panel_url, "already joined; ignoring --join and --code");
        }
        (Some(_), None, None) => {}
        (None, Some(url), Some(code)) => {
            let token = JoinToken::parse(code).ok_or_else(|| {
                AgentError::Usage(
                    "--code isn't a join code; copy it from the panel's Nodes page".into(),
                )
            })?;
            joining = Some((normalize(url), token, Keypair::generate()));
        }
        (None, _, _) => {
            return Err(AgentError::Usage(
                "this agent hasn't joined a panel yet: start it with --join <panel address> --code <join code> from the panel's Nodes page".into(),
            ));
        }
    }

    let mut backoff = Duration::from_secs(1);
    loop {
        let (url, panel_key, key, join) = match (&state, &joining) {
            (Some(s), _) => (s.panel_url.clone(), s.panel_key, s.key.clone(), None),
            (None, Some((url, token, key))) => (
                url.clone(),
                token.panel_key,
                key.clone(),
                Some(JoinProof {
                    code_id: token.code_id.clone(),
                    secret: token.secret_hex(),
                }),
            ),
            (None, None) => unreachable!("checked above"),
        };
        let hello = Hello {
            protocol: PROTOCOL,
            version: args.version.clone(),
            join,
        };
        match connect(&url, &key, &panel_key, &hello).await {
            Ok((node_id, name, session, inbound)) => {
                if state.is_none() {
                    let joined = AgentState {
                        panel_url: url.clone(),
                        panel_key,
                        key,
                        node_id: node_id.clone(),
                    };
                    joined.save(&args.dir).map_err(state_error)?;
                    tracing::info!(panel = %url, node = %name, "joined the panel");
                    state = Some(joined);
                    joining = None;
                }
                tracing::info!(panel = %url, node = %name, "connected to the panel");
                let started = Instant::now();
                serve(&session, inbound, &node).await;
                tracing::warn!(panel = %url, "lost the connection to the panel; reconnecting");
                if started.elapsed() >= STABLE {
                    backoff = Duration::from_secs(1);
                }
            }
            Err(LinkError::Rejected(reason)) if state.is_none() => {
                return Err(AgentError::Join(format!(
                    "the panel refused to let this agent join: {reason}"
                )));
            }
            Err(LinkError::WrongPanel) if state.is_none() => {
                return Err(AgentError::Join(format!(
                    "{url} isn't the panel this join code came from (its key doesn't match)"
                )));
            }
            Err(e @ (LinkError::Rejected(_) | LinkError::WrongPanel)) => {
                tracing::error!(panel = %url, error = %e, "the panel won't take this agent; asking again in 10 minutes");
                tokio::time::sleep(REJECTED_WAIT).await;
                continue;
            }
            Err(e) => {
                tracing::warn!(panel = %url, error = %e, retry_in = ?backoff, "couldn't reach the panel");
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// `https://Panel.Example.com/` → `https://panel.example.com`.
fn normalize(url: &str) -> String {
    url.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Opens the link: WebSocket to `<panel>/api/agent`, then the handshake.
pub async fn connect(
    panel_url: &str,
    key: &Keypair,
    panel_key: &[u8; 32],
    hello: &Hello,
) -> Result<(String, String, Session, Inbound), LinkError> {
    let timed_out =
        |what: &str| io::Error::new(io::ErrorKind::TimedOut, format!("{what} timed out"));
    // A dead network would otherwise wait out the system's TCP timeout.
    let frames = tokio::time::timeout(Duration::from_secs(20), dial(panel_url))
        .await
        .map_err(|_| timed_out("connecting to the panel"))??;
    let (welcome, session, inbound) = tokio::time::timeout(
        Duration::from_secs(20),
        nn_link::connect(frames, key, panel_key, hello),
    )
    .await
    .map_err(|_| timed_out("the handshake"))??;
    match welcome {
        Welcome::Accepted { node_id, name } => Ok((node_id, name, session, inbound)),
        Welcome::Rejected { reason } => Err(LinkError::Rejected(reason)),
    }
}

/// Serves the panel until the link drops: heartbeats on the control
/// stream, the panel's requests, and this machine's events.
async fn serve(session: &Session, mut inbound: Inbound, node: &Node) {
    let mut control = match open(session, StreamKind::Control).await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't open the control stream");
            return;
        }
    };
    let events = tokio::spawn(forward_events(session.clone(), node.clone()));
    let requests = {
        let node = node.clone();
        tokio::spawn(async move {
            while let Some(stream) = inbound.next().await {
                tokio::spawn(handle(stream, node.clone()));
            }
        })
    };
    let mut tick = tokio::time::interval(messages::HEARTBEAT);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let heartbeat = AgentMessage::Heartbeat { host: host_report(node).await };
                if let Err(e) = messages::send(&mut control, &heartbeat).await {
                    tracing::debug!(error = %e, "heartbeat failed");
                    break;
                }
            }
            () = session.closed() => break,
        }
    }
    session.close();
    events.abort();
    requests.abort();
}

/// Opens a stream to the panel, saying what it's for.
async fn open(session: &Session, kind: StreamKind) -> io::Result<LinkStream> {
    let mut stream = session.open().await?;
    messages::send(&mut stream, &kind).await?;
    Ok(stream)
}

/// This machine's servers.
async fn servers(node: &Node) -> crate::Result<Vec<Server>> {
    let Node::Local(local) = node else {
        return Ok(Vec::new());
    };
    Ok(local.orchestrator()?.servers().await?)
}

/// Tells the panel about every change to this machine's servers, and passes
/// on this machine's events.
async fn forward_events(session: Session, node: Node) {
    let Node::Local(local) = &node else {
        return;
    };
    // Subscribe first, so nothing between the snapshot and the stream is lost.
    let mut events = local.events.subscribe();
    let Ok(mut stream) = open(&session, StreamKind::Events).await else {
        return;
    };
    let snapshot = |servers: crate::Result<Vec<Server>>| AgentEvent::Servers {
        servers: servers.unwrap_or_default(),
    };
    if messages::send(&mut stream, &snapshot(servers(&node).await))
        .await
        .is_err()
    {
        return;
    }
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                if messages::send(&mut stream, &snapshot(servers(&node).await))
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        if let Event::ServerStatusChanged { server_id, status } = &event {
            let change = if status == "deleted" {
                Some(AgentEvent::Removed {
                    id: server_id.clone(),
                })
            } else {
                match local.orchestrator() {
                    Ok(o) => o
                        .server(server_id)
                        .await
                        .ok()
                        .map(|server| AgentEvent::Server {
                            server: Box::new(server),
                        }),
                    Err(_) => None,
                }
            };
            if let Some(change) = change
                && messages::send(&mut stream, &change).await.is_err()
            {
                return;
            }
        }
        if messages::send(&mut stream, &AgentEvent::Event { event })
            .await
            .is_err()
        {
            return;
        }
    }
}

/// Answers one request from the panel.
async fn handle(mut stream: LinkStream, node: Node) {
    let request = match messages::recv::<_, Request>(&mut stream).await {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(e) => {
            tracing::debug!(error = %e, "a request from the panel didn't make sense");
            return;
        }
    };
    if let Request::Logs { id } = &request {
        return follow_logs(stream, &node, id).await;
    }
    if let Request::Connect { id } = &request {
        return relay(stream, &node, id).await;
    }
    if let Request::OpenBackup { name, offset } = &request {
        return send_backup(stream, &node, name, *offset).await;
    }
    if let Request::ReceiveBackup { name, len } = &request {
        return receive_archive(stream, &node, name, *len).await;
    }
    let request = match request {
        Request::Folder { id, call } => return folder(stream, &node, &id, call).await,
        other => other,
    };
    let reply = answer(&node, request).await;
    let _ = messages::send(&mut stream, &reply).await;
}

fn reply<T: Serialize>(result: crate::Result<T>) -> Reply {
    result
        .map(|value| serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
        .map_err(|e| RemoteError::from(&e))
}

async fn answer(node: &Node, request: Request) -> Reply {
    match request {
        Request::ListServers => reply(servers(node).await),
        Request::Create { input } => reply(node.create(input).await),
        Request::Start { id } => reply(node.start(&id).await),
        Request::Wake { id } => reply(node.wake(&id).await),
        Request::Stop { id, sleep } => reply(node.stop(&id, sleep).await),
        Request::Restart { id } => reply(node.restart(&id).await),
        Request::Kill { id } => reply(node.kill(&id).await),
        Request::Update { id, settings } => reply(node.update(&id, settings).await),
        Request::Delete { id, delete_data } => reply(node.delete(&id, delete_data).await),
        Request::ContainerRunning { id } => reply(node.container_running(&id).await),
        Request::RestartRequired { id } => {
            let required = match node {
                Node::Local(local) => match local.orchestrator() {
                    Ok(o) => match o.server(&id).await {
                        Ok(server) => o.restart_required(&server).await,
                        Err(_) => false,
                    },
                    Err(_) => false,
                },
                Node::Remote(_) => false,
            };
            reply(Ok(required))
        }
        Request::StatsHistory { id } => reply(Ok(node.stats_history(&id).await)),
        Request::Command { id, command } => reply(node.command(&id, &command).await),
        Request::SetCurseforgeKey { key } => {
            node.set_curseforge_key(key);
            reply(Ok(()))
        }
        Request::BackupsFreeSpace => said(node.backups_free_space().await),
        Request::Backup { id, name, job_id } => match server_row(node, &id).await {
            Ok(server) => said(node.backup(&server, &name, &job_id).await),
            Err(e) => reply::<()>(Err(e)),
        },
        Request::Restore {
            id,
            source,
            pre_restore,
            job_id,
        } => match server_row(node, &id).await {
            Ok(server) => said(node.restore(&server, &source, &pre_restore, &job_id).await),
            Err(e) => reply::<()>(Err(e)),
        },
        Request::BackupSize { name } => {
            let size = node.open_backup(&name).await.map(|reader| reader.size());
            reply(Ok(size))
        }
        Request::DeleteBackup { name } => said(node.delete_backup(&name).await),
        Request::Adopt {
            server,
            archive,
            avoid_ports,
            job_id,
        } => {
            let adopted = node.adopt(&server, &archive, &avoid_ports, &job_id).await;
            if adopted.is_err() {
                // Leave nothing half set up.
                let _ = node.delete(&server.id, true).await;
            }
            said(adopted)
        }
        Request::Logs { .. }
        | Request::Connect { .. }
        | Request::Folder { .. }
        | Request::OpenBackup { .. }
        | Request::ReceiveBackup { .. } => {
            unreachable!("handled by follow_logs, relay, folder, send_backup, and receive_archive")
        }
    }
}

/// Connects the stream to a server's game port on this machine and passes
/// bytes both ways until either side closes.
async fn relay(mut stream: LinkStream, node: &Node, id: &str) {
    let target = match node {
        Node::Local(local) => match local.orchestrator() {
            Ok(o) => match o.server(id).await {
                Ok(server) => u16::try_from(server.internal_port)
                    .ok()
                    .map(|port| std::net::SocketAddr::new(o.backend_ip(), port)),
                Err(_) => None,
            },
            Err(_) => None,
        },
        Node::Remote(_) => None,
    };
    let Some(addr) = target else {
        let missing = crate::NodeError::Orchestrator(nn_orchestrator::OrchestratorError::NotFound);
        let _ = messages::send(&mut stream, &reply::<()>(Err(missing))).await;
        return;
    };
    let mut server = match TcpStream::connect(addr).await {
        Ok(server) => server,
        Err(e) => {
            let failed =
                crate::NodeError::Orchestrator(nn_orchestrator::OrchestratorError::InvalidState(
                    format!("the server isn't accepting connections: {e}"),
                ));
            let _ = messages::send(&mut stream, &reply::<()>(Err(failed))).await;
            return;
        }
    };
    let _ = server.set_nodelay(true);
    if messages::send(&mut stream, &reply(Ok(()))).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut server).await;
}

/// A reply from an operation that fails with a message.
fn said<T: Serialize>(result: Result<T, String>) -> Reply {
    reply(result.map_err(|message| {
        crate::NodeError::Remote(RemoteError {
            kind: crate::protocol::ErrorKind::Internal,
            message,
        })
    }))
}

/// This machine's record of a server.
async fn server_row(node: &Node, id: &str) -> crate::Result<Server> {
    match node {
        Node::Local(local) => Ok(local.orchestrator()?.server(id).await?),
        Node::Remote(_) => Err(crate::NodeError::NotYet(
            "agents only serve their own servers",
        )),
    }
}

/// Sends a backup archive from `offset`.
async fn send_backup(mut stream: LinkStream, node: &Node, name: &str, offset: u64) {
    let Some(reader) = node.open_backup(name).await else {
        let missing = crate::NodeError::Remote(RemoteError {
            kind: crate::protocol::ErrorKind::NotFound,
            message: "the backup's file is missing".into(),
        });
        let _ = messages::send(&mut stream, &reply::<()>(Err(missing))).await;
        return;
    };
    let size = reader.size();
    match reader.from_offset(offset).await {
        Ok(mut bytes) => {
            if messages::send(&mut stream, &reply(Ok(size))).await.is_ok() {
                let _ = tokio::io::copy(&mut bytes, &mut stream).await;
                let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
            }
        }
        Err(e) => {
            let failed = crate::NodeError::File(FileError::Io(e));
            let _ = messages::send(&mut stream, &reply::<()>(Err(failed))).await;
        }
    }
}

/// Receives an archive for a server moving here.
async fn receive_archive(mut stream: LinkStream, node: &Node, name: &str, len: u64) {
    let Node::Local(local) = node else {
        return;
    };
    let result = local.receive_archive(name, &mut stream, len).await;
    let _ = messages::send(&mut stream, &said(result)).await;
}

/// Largest file accepted in one write (a big mod, say).
const MAX_WRITE: u64 = 512 * 1024 * 1024;

/// Answers a folder request against this machine's copy of the server.
async fn folder(mut stream: LinkStream, node: &Node, id: &str, call: FolderOp) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fail = |e: &FileError| -> FolderReply { Err(FileFailure::from(e)) };
    let Node::Local(local) = node else {
        return;
    };
    let opened = match local.orchestrator() {
        Ok(o) => match o.server(id).await {
            Ok(server) => local.open_folder(&server).await,
            Err(e) => Err(e.into()),
        },
        Err(e) => Err(e),
    };
    let folder = match opened {
        Ok(folder) => folder,
        Err(e) => {
            let e = match e {
                crate::NodeError::File(f) => f,
                other => FileError::Invalid(other.to_string()),
            };
            let _ = messages::send(&mut stream, &fail(&e)).await;
            return;
        }
    };
    fn ok<T: Serialize>(result: Result<T, FileError>) -> FolderReply {
        result
            .map(|v| serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            .map_err(|e| FileFailure::from(&e))
    }
    let reply = match call {
        FolderOp::List { path } => ok(folder.list(&path).await),
        FolderOp::Stat { path } => ok(folder.stat(&path).await),
        FolderOp::Read { path, limit } => match folder.read(&path, limit).await {
            Ok(content) => {
                let header = FileHeader {
                    len: content.data.len() as u64,
                    is_text: content.is_text,
                    version: content.version,
                    modified_ms: content.modified_ms,
                };
                if messages::send(&mut stream, &ok(Ok(header))).await.is_ok() {
                    let _ = stream.write_all(&content.data).await;
                    let _ = stream.flush().await;
                }
                return;
            }
            Err(e) => fail(&e),
        },
        FolderOp::Write { path, expect, len } => {
            if len > MAX_WRITE {
                fail(&FileError::TooLarge {
                    what: path.to_string(),
                    limit: "512 MB".into(),
                })
            } else {
                let mut data = vec![0u8; usize::try_from(len).unwrap_or(0)];
                if stream.read_exact(&mut data).await.is_err() {
                    return;
                }
                ok(folder.write(&path, data, expect).await)
            }
        }
        FolderOp::Mkdir { path } => ok(folder.mkdir(&path).await),
        FolderOp::Rename { from, to } => ok(folder.rename(&from, &to).await),
        FolderOp::Copy { from, to } => ok(folder.copy(&from, &to).await),
        FolderOp::Delete {
            paths,
            retention_secs,
        } => ok(folder
            .delete(paths, Duration::from_secs(retention_secs))
            .await),
        FolderOp::Trash { retention_secs } => {
            ok(folder.trash(Duration::from_secs(retention_secs)).await)
        }
        FolderOp::Restore { id } => ok(folder.restore(&id).await),
        FolderOp::Purge { id } => ok(folder.purge(id.as_deref()).await),
        FolderOp::Sha1 { path } => ok(folder.sha1(&path).await),
        FolderOp::ModInfo { path } => ok(folder.mod_info(&path).await),
        FolderOp::ModIcon { path } => match folder.mod_icon(&path).await {
            Ok(Some(icon)) => {
                if messages::send(&mut stream, &ok(Ok(Some(icon.len() as u64))))
                    .await
                    .is_ok()
                {
                    let _ = stream.write_all(&icon).await;
                    let _ = stream.flush().await;
                }
                return;
            }
            Ok(None) => ok(Ok(None::<u64>)),
            Err(e) => fail(&e),
        },
        FolderOp::Extract {
            archive,
            dest,
            overwrite,
        } => ok(folder.extract(archive, dest, overwrite).await),
        FolderOp::Compress { sources, dest } => ok(folder.compress(sources, dest).await),
        FolderOp::OpenRead { path, offset } => match folder.open_read(&path).await {
            Ok(reader) => {
                let size = reader.size();
                match reader.from_offset(offset).await {
                    Ok(mut bytes) => {
                        if messages::send(&mut stream, &ok(Ok(size))).await.is_ok() {
                            let _ = tokio::io::copy(&mut bytes, &mut stream).await;
                            let _ = stream.shutdown().await;
                        }
                        return;
                    }
                    Err(e) => fail(&FileError::Io(e)),
                }
            }
            Err(e) => fail(&e),
        },
        FolderOp::Adopt {
            dir,
            name,
            on_conflict,
            len,
        } => {
            // Received into this machine's upload folder, then placed.
            let tmp = match local.upload_dir() {
                Some(dir) => dir.join(format!("{}.part", nn_store::new_id())),
                None => {
                    let _ = messages::send(
                        &mut stream,
                        &fail(&FileError::Invalid(
                            "the data folder isn't available".into(),
                        )),
                    )
                    .await;
                    return;
                }
            };
            let received = async {
                let mut file = tokio::fs::File::create(&tmp).await?;
                let copied = tokio::io::copy(&mut (&mut stream).take(len), &mut file).await?;
                file.sync_all().await?;
                if copied == len {
                    Ok(())
                } else {
                    Err(std::io::Error::other("the upload was cut short"))
                }
            }
            .await;
            let reply = match received {
                Ok(()) => ok(folder.adopt(tmp.clone(), &dir, &name, on_conflict).await),
                Err(e) => fail(&FileError::Io(e)),
            };
            let _ = tokio::fs::remove_file(&tmp).await;
            reply
        }
    };
    let _ = messages::send(&mut stream, &reply).await;
}

/// Sends a server's console backlog, then each new line.
async fn follow_logs(mut stream: LinkStream, node: &Node, id: &str) {
    let logs = match node.subscribe_logs(id).await {
        Ok(logs) => logs,
        Err(e) => {
            let _ = messages::send(&mut stream, &reply::<()>(Err(e))).await;
            return;
        }
    };
    let backlog: Vec<&str> = logs.backlog.iter().map(|l| &**l).collect();
    if messages::send(&mut stream, &reply(Ok(backlog)))
        .await
        .is_err()
    {
        return;
    }
    let mut lines = logs.lines;
    loop {
        match lines.recv().await {
            Ok(line) => {
                let line = LogLine {
                    line: line.to_string(),
                };
                if messages::send(&mut stream, &line).await.is_err() {
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// This machine, for the panel.
pub async fn host_report(node: &Node) -> HostReport {
    let mut report = match node.host_info().await {
        Ok(info) => HostReport {
            docker_available: true,
            hostname: info.hostname,
            operating_system: info.operating_system,
            docker_version: info.docker_version,
            cpus: info.cpus,
            memory_bytes: info.memory_bytes,
            ..Default::default()
        },
        Err(crate::NodeError::Unavailable(reason)) => HostReport {
            docker_error: reason.to_string(),
            ..Default::default()
        },
        Err(e) => HostReport {
            docker_error: e.to_string(),
            ..Default::default()
        },
    };
    if let Some((total, free)) = node.disk_space() {
        report.disk_total_bytes = Some(total);
        report.disk_free_bytes = Some(free);
    }
    report
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Opens a WebSocket to the panel's agent endpoint.
async fn dial(panel_url: &str) -> Result<nn_link::Frames, LinkError> {
    let bad =
        |why: &str| LinkError::Io(io::Error::new(io::ErrorKind::InvalidInput, why.to_owned()));
    let uri: http::Uri = panel_url
        .parse()
        .map_err(|_| bad("the panel address isn't a URL"))?;
    let tls = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err(bad("the panel address must start with http:// or https://")),
    };
    let host = uri
        .host()
        .ok_or_else(|| bad("the panel address has no host"))?
        .to_owned();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let base = uri.path().trim_end_matches('/');
    let ws_url = format!(
        "{}://{}{base}/api/agent",
        if tls { "wss" } else { "ws" },
        uri.authority().map(|a| a.as_str()).unwrap_or(&host),
    );
    let request = ws_url
        .as_str()
        .into_client_request()
        .map_err(|e| bad(&e.to_string()))?;

    let tcp = TcpStream::connect((host.as_str(), port)).await?;
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Io> = if tls {
        let config = tls_config().map_err(|e| LinkError::Io(io::Error::other(e)))?;
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|_| bad("the panel's host name isn't valid"))?;
        let connector = tokio_rustls::TlsConnector::from(config);
        Box::new(connector.connect(name, tcp).await?)
    } else {
        Box::new(tcp)
    };
    let config = WebSocketConfig::default().max_message_size(Some(70_000));
    let (ws, _) = tokio_tungstenite::client_async_with_config(request, stream, Some(config))
        .await
        .map_err(|e| LinkError::Io(io::Error::other(e)))?;
    Ok(Box::new(WsFrames(ws)))
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, rustls::Error> {
    use rustls_platform_verifier::ConfigVerifierExt;
    Ok(Arc::new(rustls::ClientConfig::with_platform_verifier()?))
}

/// A tungstenite WebSocket as link frames: binary messages only.
struct WsFrames(tokio_tungstenite::WebSocketStream<Box<dyn Io>>);

impl Stream for WsFrames {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.0.poll_next_unpin(cx)) {
                Some(Ok(Message::Binary(bytes))) => Poll::Ready(Some(Ok(bytes))),
                Some(Ok(Message::Close(_))) | None => Poll::Ready(None),
                // Pings are answered by tungstenite; nothing else is expected.
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
