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
use nn_link::messages::{self, AgentMessage, Hello, HostReport, JoinProof, Welcome};
use nn_link::{JoinToken, Keypair, LinkError, PROTOCOL, Session};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::Node;

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
            Ok((node_id, name, session)) => {
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
                serve(&session, &node).await;
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
) -> Result<(String, String, Session), LinkError> {
    let frames = dial(panel_url).await?;
    let (welcome, session, _inbound) = tokio::time::timeout(
        Duration::from_secs(20),
        nn_link::connect(frames, key, panel_key, hello),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the handshake timed out"))??;
    match welcome {
        Welcome::Accepted { node_id, name } => Ok((node_id, name, session)),
        Welcome::Rejected { reason } => Err(LinkError::Rejected(reason)),
    }
}

/// Keeps the control stream going until the link drops.
async fn serve(session: &Session, node: &Node) {
    let mut control = match session.open().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't open the control stream");
            return;
        }
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
