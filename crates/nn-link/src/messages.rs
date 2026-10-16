//! What the panel and an agent say to each other. JSON, each message
//! prefixed with its length (big-endian `u32`): at most [`MAX_MESSAGE`], or
//! [`MAX_LARGE_MESSAGE`] where the panel reads an agent's replies.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest message accepted by default: handshakes, heartbeats, and
/// everything an agent is sent.
pub const MAX_MESSAGE: usize = 64 * 1024;

/// Largest reply or report the panel accepts from a joined agent: an hour of
/// stats, a console backlog, a big folder listing. See [`recv_large`].
pub const MAX_LARGE_MESSAGE: usize = 16 * 1024 * 1024;

/// The agent's first message once the link is encrypted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// [`crate::PROTOCOL`].
    pub protocol: u32,
    /// The agent's NetherNode version.
    pub version: String,
    /// The join code, on the agent's first connection.
    pub join: Option<JoinProof>,
}

/// Proof that the agent was given a join code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinProof {
    /// The code's ID.
    pub code_id: String,
    /// The code's secret, hex encoded.
    pub secret: String,
}

impl std::fmt::Debug for JoinProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinProof")
            .field("code_id", &self.code_id)
            .finish_non_exhaustive()
    }
}

/// The panel's answer to [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Welcome {
    /// The agent is this node.
    Accepted {
        /// The node's ID.
        node_id: String,
        /// The node's name.
        name: String,
    },
    /// Not a node this panel knows (removed, or a bad join code).
    Rejected {
        /// Why, for the agent's log.
        reason: String,
    },
}

/// Facts about the agent's machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostReport {
    /// Docker could be reached.
    pub docker_available: bool,
    /// Why not, when it couldn't.
    pub docker_error: String,
    /// Host name.
    pub hostname: String,
    /// Operating system.
    pub operating_system: String,
    /// Docker engine version.
    pub docker_version: String,
    /// Logical CPUs.
    pub cpus: u32,
    /// Total memory in bytes.
    pub memory_bytes: u64,
    /// Disk holding servers and backups: total bytes.
    pub disk_total_bytes: Option<u64>,
    /// And free bytes.
    pub disk_free_bytes: Option<u64>,
}

/// Messages on the control stream, from the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// Sent every [`HEARTBEAT`]; the panel counts a node offline after
    /// three missed.
    Heartbeat {
        /// The machine right now.
        host: HostReport,
    },
}

/// The first message on every stream the agent opens: what it's for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Heartbeats ([`AgentMessage`]).
    Control,
    /// Changes to the agent's servers, and its events.
    Events,
}

/// How often the agent sends a heartbeat.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

/// Writes one message.
pub async fn send<W, T>(out: &mut W, message: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(message)?;
    if body.len() > MAX_LARGE_MESSAGE {
        return Err(std::io::Error::other("message too large"));
    }
    out.write_u32(body.len() as u32).await?;
    out.write_all(&body).await?;
    out.flush().await
}

/// Reads one message of at most [`MAX_MESSAGE`]. `Ok(None)` when the stream
/// ends between messages.
pub async fn recv<R, T>(input: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    recv_limited(input, MAX_MESSAGE).await
}

/// Like [`recv`], up to [`MAX_LARGE_MESSAGE`]: for what the panel reads from
/// an agent that has already proved its key.
pub async fn recv_large<R, T>(input: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    recv_limited(input, MAX_LARGE_MESSAGE).await
}

async fn recv_limited<R, T>(input: &mut R, max: usize) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let len = match input.read_u32().await {
        Ok(len) => len as usize,
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    if len > max {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn large_messages_only_where_allowed() {
        let big = "x".repeat(MAX_MESSAGE * 4);
        let mut wire = Vec::new();
        send(&mut wire, &big).await.unwrap();
        assert!(recv::<_, String>(&mut wire.as_slice()).await.is_err());
        let got: Option<String> = recv_large(&mut wire.as_slice()).await.unwrap();
        assert_eq!(got.as_deref(), Some(big.as_str()));

        let mut wire = Vec::new();
        send(&mut wire, &"small").await.unwrap();
        let got: Option<String> = recv(&mut wire.as_slice()).await.unwrap();
        assert_eq!(got.as_deref(), Some("small"));
    }
}
