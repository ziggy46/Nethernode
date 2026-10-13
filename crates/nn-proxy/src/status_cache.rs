//! The last known status of each server, so pings can be answered while a
//! server sleeps or responds slowly.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use nn_mcproto::codec::frame;
use nn_mcproto::handshake::{Handshake, NextState};
use nn_mcproto::status::{decode_status_response, encode_status_request};
use nn_mcproto::{MAX_STATUS_RESPONSE_FRAME, StatusResponse};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::io::{FrameBuffer, ReadError};
use crate::proxy_protocol;

/// Protocol version sent when pinging backends. Servers answer status pings
/// for any version; 47 (1.8) is the conventional choice for probes.
const PING_PROTOCOL: i32 = 47;

/// A cached status response.
#[derive(Debug, Clone)]
pub struct CachedStatus {
    /// The server's response.
    pub response: StatusResponse,
    /// When it was fetched.
    pub fetched_at: Instant,
}

/// Last known status per server ID.
#[derive(Debug, Default)]
pub struct StatusCache {
    entries: RwLock<HashMap<String, CachedStatus>>,
}

impl StatusCache {
    /// The cached status for a server.
    pub fn get(&self, server_id: &str) -> Option<CachedStatus> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(server_id)
            .cloned()
    }

    /// Stores a fresh status.
    pub fn put(&self, server_id: &str, response: StatusResponse) {
        let entry = CachedStatus {
            response,
            fetched_at: Instant::now(),
        };
        self.entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(server_id.to_owned(), entry);
    }

    /// Drops entries for servers that no longer exist.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.entries
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id, _| keep(id));
    }
}

/// Why a status ping failed.
#[derive(Debug, thiserror::Error)]
pub enum PingError {
    /// The backend didn't answer in time.
    #[error("timed out")]
    Timeout,
    /// Reading the response failed.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The response wasn't valid status JSON.
    #[error(transparent)]
    Decode(#[from] nn_mcproto::DecodeError),
    /// Socket error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Asks a backend for its status, as a client would.
///
/// When the backend expects PROXY protocol, a `LOCAL` header is sent first
/// (the form meant for health checks).
pub async fn ping_backend(
    addr: SocketAddr,
    proxy_protocol: bool,
    timeout: Duration,
) -> Result<StatusResponse, PingError> {
    tokio::time::timeout(timeout, async {
        let mut stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        let mut out = Vec::with_capacity(64);
        if proxy_protocol {
            out.extend_from_slice(&proxy_protocol::local());
        }
        let handshake = Handshake {
            protocol_version: PING_PROTOCOL,
            server_address: addr.ip().to_string(),
            server_port: addr.port(),
            next_state: NextState::Status,
        };
        out.extend_from_slice(&frame(&handshake.encode()));
        out.extend_from_slice(&frame(&encode_status_request()));
        stream.write_all(&out).await?;

        let mut buf = FrameBuffer::new();
        let range = buf
            .next_frame(&mut stream, MAX_STATUS_RESPONSE_FRAME)
            .await?;
        let json = decode_status_response(buf.payload(range))?;
        Ok(StatusResponse::from_json(&json)?)
    })
    .await
    .map_err(|_| PingError::Timeout)?
}
