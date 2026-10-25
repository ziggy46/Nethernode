//! Bedrock Dedicated Server's status, as its NetherNet transport serves it.
//!
//! With NetherNet (the default since Bedrock 1.26.50) a server listens on
//! one TCP port for a small HTTP exchange; the game itself then runs over
//! UDP ports negotiated per player. `GET /v1/join` on that port answers with
//! the server's name, version and player counts as JSON, which is all
//! NetherNode needs to know a server is up and who's on it.
//!
//! Nothing here covers the join itself: players connect to the server's
//! ports directly.

/// The path a client asks first, and the server answers with its status.
pub const STATUS_PATH: &str = "/v1/join";

/// What `GET /v1/join` says about a server.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The server's name, as shown in the server list.
    #[serde(default)]
    pub name: String,
    /// Network protocol version.
    #[serde(default)]
    pub protocol: i64,
    /// Game version, e.g. `1.26.52`.
    #[serde(default)]
    pub version: String,
    /// The world's name.
    #[serde(default)]
    pub level: String,
    /// Players online.
    #[serde(default)]
    pub players: u32,
    /// Player limit.
    #[serde(default)]
    pub max_players: u32,
    /// Default game mode (0 survival, 1 creative, 2 adventure).
    #[serde(default)]
    pub game_type: i64,
}

/// The request for [`STATUS_PATH`], for a server at `host`.
pub fn status_request(host: &str) -> Vec<u8> {
    format!("GET {STATUS_PATH} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: NetherNode\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

/// Reads the status out of a complete HTTP response. `None` if it isn't a
/// `200` with a JSON status body.
pub fn parse_response(raw: &[u8]) -> Option<Status> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status_line = head.lines().next()?;
    let mut parts = status_line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") || parts.next()? != "200" {
        return None;
    }
    serde_json::from_slice(&raw[split + 4..]).ok()
}

#[cfg(feature = "tokio")]
pub use client::{ProbeError, probe};

#[cfg(feature = "tokio")]
mod client {
    use std::net::SocketAddr;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use super::{Status, parse_response, status_request};

    /// The most a status response may be (a real one is about 130 bytes).
    const MAX_RESPONSE: u64 = 16 * 1024;

    /// Why asking a server for its status failed.
    #[derive(Debug, thiserror::Error)]
    pub enum ProbeError {
        /// The server didn't answer in time.
        #[error("timed out")]
        Timeout,
        /// Something answered, but not with a Bedrock status.
        #[error("not a Bedrock server's status")]
        NotAStatus,
        /// Socket error.
        #[error(transparent)]
        Io(#[from] std::io::Error),
    }

    /// Asks the server whose signalling port is at `addr` for its status.
    pub async fn probe(addr: SocketAddr, timeout: Duration) -> Result<Status, ProbeError> {
        tokio::time::timeout(timeout, async {
            let mut stream = TcpStream::connect(addr).await?;
            stream.write_all(&status_request(&addr.to_string())).await?;
            let mut raw = Vec::new();
            (&mut stream)
                .take(MAX_RESPONSE)
                .read_to_end(&mut raw)
                .await?;
            parse_response(&raw).ok_or(ProbeError::NotAStatus)
        })
        .await
        .map_err(|_| ProbeError::Timeout)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a Bedrock 1.26.52 server answered, byte for byte.
    const CAPTURED: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 127\r\nConnection: close\r\n\r\n{\"name\":\"NetherNet probe\",\"protocol\":2193,\"version\":\"1.26.52\",\"level\":\"Bedrock level\",\"players\":0,\"maxPlayers\":10,\"gameType\":0}";

    #[test]
    fn reads_a_real_servers_answer() {
        let status = parse_response(CAPTURED).unwrap();
        assert_eq!(status.name, "NetherNet probe");
        assert_eq!(status.version, "1.26.52");
        assert_eq!((status.players, status.max_players), (0, 10));
        assert_eq!(status.protocol, 2193);
    }

    #[test]
    fn refuses_what_is_not_a_status() {
        assert!(parse_response(b"").is_none());
        assert!(parse_response(b"HTTP/1.1 404 Not Found\r\n\r\nNot Found").is_none());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n\r\nnot json").is_none());
        // A TLS alert, which is what a server says to a handshake.
        assert!(parse_response(b"\x15\x03\x01\x00\x02\x02(").is_none());
    }

    #[test]
    fn missing_fields_default() {
        let status = parse_response(b"HTTP/1.1 200 OK\r\n\r\n{\"players\":3}").unwrap();
        assert_eq!((status.players, status.max_players), (3, 0));
    }

    #[test]
    fn the_request_names_the_host() {
        let request = String::from_utf8(status_request("10.0.0.1:19132")).unwrap();
        assert!(request.starts_with("GET /v1/join HTTP/1.1\r\nHost: 10.0.0.1:19132\r\n"));
        assert!(request.ends_with("\r\n\r\n"));
    }

    #[cfg(feature = "tokio")]
    #[tokio::test]
    async fn probes_a_server_over_tcp() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 512];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(buf[..n].starts_with(b"GET /v1/join HTTP/1.1"));
            stream.write_all(CAPTURED).await.unwrap();
        });
        let status = probe(addr, std::time::Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(status.level, "Bedrock level");
    }
}
