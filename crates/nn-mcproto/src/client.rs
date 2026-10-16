//! A status-ping client, as a Minecraft client would use to fill its server list.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Server_List_Ping>

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::codec::frame;
use crate::handshake::{Handshake, NextState};
use crate::io::{FrameBuffer, ReadError};
use crate::status::{decode_status_response, encode_status_request};
use crate::{DecodeError, MAX_STATUS_RESPONSE_FRAME, StatusResponse};

/// Protocol version sent in pings. Servers answer status pings for any
/// version; 47 (1.8) is the conventional choice for probes.
pub const PING_PROTOCOL: i32 = 47;

/// Why a status ping failed.
#[derive(Debug, thiserror::Error)]
pub enum PingError {
    /// The server didn't answer in time.
    #[error("timed out")]
    Timeout,
    /// Reading the response failed.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The response wasn't valid status JSON.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// Socket error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Asks the server at `addr` for its status. `prelude` is written first
/// (e.g. a PROXY protocol header the server expects).
pub async fn ping(
    addr: SocketAddr,
    prelude: &[u8],
    timeout: Duration,
) -> Result<StatusResponse, PingError> {
    tokio::time::timeout(timeout, async {
        let mut stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        ping_on(&mut stream, &addr.ip().to_string(), addr.port(), prelude).await
    })
    .await
    .map_err(|_| PingError::Timeout)?
}

/// Asks for a server's status over an open connection to it (one reached
/// through a tunnel, say). `host` and `port` go in the handshake.
pub async fn ping_on<S>(
    stream: &mut S,
    host: &str,
    port: u16,
    prelude: &[u8],
) -> Result<StatusResponse, PingError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let handshake = Handshake {
        protocol_version: PING_PROTOCOL,
        server_address: host.to_owned(),
        server_port: port,
        next_state: NextState::Status,
    };
    let mut out = prelude.to_vec();
    out.extend_from_slice(&frame(&handshake.encode()));
    out.extend_from_slice(&frame(&encode_status_request()));
    stream.write_all(&out).await?;

    let mut buf = FrameBuffer::new();
    let range = buf.next_frame(stream, MAX_STATUS_RESPONSE_FRAME).await?;
    let json = decode_status_response(buf.payload(range))?;
    Ok(StatusResponse::from_json(&json)?)
}
