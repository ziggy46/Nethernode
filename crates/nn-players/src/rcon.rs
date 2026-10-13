//! A small RCON client.
//!
//! Packet: `length:i32 | request_id:i32 | type:i32 | payload | 0x00 | 0x00`,
//! little-endian, where `length` counts everything after itself. Types:
//! 3 = login, 2 = command (also the login reply), 0 = response. A failed
//! login replies with request ID -1. Long responses arrive in several
//! packets of up to 4096 bytes. When a reply fills a whole packet, more may
//! follow, so a sentinel packet with an unknown type is sent; its reply
//! (same ID as the sentinel) marks that everything before it has arrived.
//! The sentinel goes out only after the first reply: vanilla servers read
//! one packet per `read()` and drop the connection if two arrive together.
//! Source: <https://minecraft.wiki/w/RCON>

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::time::timeout;

const TYPE_LOGIN: i32 = 3;
const TYPE_COMMAND: i32 = 2;
/// Any unrecognized type works as a sentinel; servers reply "Unknown request".
const TYPE_SENTINEL: i32 = 200;
/// Client→server payload limit.
pub const MAX_COMMAND_BYTES: usize = 1446;
/// Largest payload in one server packet; a full one may be continued.
const MAX_PAYLOAD: usize = 4096;
/// Largest packet the server sends (payload + header).
const MAX_PACKET: usize = MAX_PAYLOAD + 14;

/// RCON failures.
#[derive(Debug, thiserror::Error)]
pub enum RconError {
    /// Couldn't connect or the connection dropped.
    #[error("RCON connection failed: {0}")]
    Io(#[from] std::io::Error),
    /// The server rejected the password.
    #[error("RCON authentication failed")]
    AuthFailed,
    /// The server didn't answer in time.
    #[error("RCON timed out")]
    Timeout,
    /// The command is longer than RCON allows.
    #[error("command too long ({0} bytes, max {MAX_COMMAND_BYTES})")]
    CommandTooLong(usize),
    /// The server sent something that isn't a valid RCON packet.
    #[error("invalid RCON packet")]
    Protocol,
}

/// An authenticated RCON connection.
pub struct Rcon {
    stream: TcpStream,
    next_id: i32,
    timeout: Duration,
}

impl std::fmt::Debug for Rcon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rcon")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl Rcon {
    /// Connects and logs in. `op_timeout` bounds the connect, the login,
    /// and later each command.
    pub async fn connect(
        addr: impl ToSocketAddrs,
        password: &str,
        op_timeout: Duration,
    ) -> Result<Self, RconError> {
        let stream = timeout(op_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| RconError::Timeout)??;
        stream.set_nodelay(true)?;
        let mut rcon = Self {
            stream,
            next_id: 1,
            timeout: op_timeout,
        };
        let id = rcon.take_id();
        rcon.send(id, TYPE_LOGIN, password).await?;
        // Some servers send an empty response packet before the auth reply.
        loop {
            let (reply_id, kind, _) = rcon.recv().await?;
            if reply_id == -1 {
                return Err(RconError::AuthFailed);
            }
            if reply_id == id && kind == TYPE_COMMAND {
                return Ok(rcon);
            }
        }
    }

    /// Runs a command and returns its full output.
    pub async fn command(&mut self, command: &str) -> Result<String, RconError> {
        if command.len() > MAX_COMMAND_BYTES {
            return Err(RconError::CommandTooLong(command.len()));
        }
        let id = self.take_id();
        self.send(id, TYPE_COMMAND, command).await?;
        let mut output = self.recv_reply(id).await?;
        if output.len() >= MAX_PAYLOAD {
            // Possibly fragmented: the server answers the sentinel only
            // after sending every fragment.
            let sentinel = self.take_id();
            self.send(sentinel, TYPE_SENTINEL, "").await?;
            loop {
                let (reply_id, _, payload) = self.recv().await?;
                if reply_id == -1 {
                    return Err(RconError::AuthFailed);
                }
                if reply_id == sentinel {
                    break;
                }
                if reply_id == id {
                    output.extend_from_slice(&payload);
                }
            }
        }
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// The first reply to request `id`.
    async fn recv_reply(&mut self, id: i32) -> Result<Vec<u8>, RconError> {
        loop {
            let (reply_id, _, payload) = self.recv().await?;
            if reply_id == -1 {
                return Err(RconError::AuthFailed);
            }
            if reply_id == id {
                return Ok(payload);
            }
        }
    }

    /// Sends a command without waiting for output. For `stop`, where the
    /// server may close the connection before replying.
    pub async fn send_only(&mut self, command: &str) -> Result<(), RconError> {
        if command.len() > MAX_COMMAND_BYTES {
            return Err(RconError::CommandTooLong(command.len()));
        }
        let id = self.take_id();
        self.send(id, TYPE_COMMAND, command).await
    }

    fn take_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        id
    }

    async fn send(&mut self, id: i32, kind: i32, payload: &str) -> Result<(), RconError> {
        let body_len = 4 + 4 + payload.len() + 2;
        let mut packet = Vec::with_capacity(4 + body_len);
        packet.extend_from_slice(&(body_len as i32).to_le_bytes());
        packet.extend_from_slice(&id.to_le_bytes());
        packet.extend_from_slice(&kind.to_le_bytes());
        packet.extend_from_slice(payload.as_bytes());
        packet.extend_from_slice(&[0, 0]);
        timeout(self.timeout, self.stream.write_all(&packet))
            .await
            .map_err(|_| RconError::Timeout)??;
        Ok(())
    }

    async fn recv(&mut self) -> Result<(i32, i32, Vec<u8>), RconError> {
        timeout(self.timeout, async {
            let len = self.stream.read_i32_le().await?;
            let len = usize::try_from(len).map_err(|_| RconError::Protocol)?;
            if !(10..=MAX_PACKET).contains(&len) {
                return Err(RconError::Protocol);
            }
            let mut body = vec![0u8; len];
            self.stream.read_exact(&mut body).await?;
            let id = i32::from_le_bytes([body[0], body[1], body[2], body[3]]);
            let kind = i32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            // Payload is NUL-terminated, followed by one pad byte.
            let payload = body[8..len - 2].to_vec();
            Ok((id, kind, payload))
        })
        .await
        .map_err(|_| RconError::Timeout)?
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    const T: Duration = Duration::from_secs(2);

    /// Reads like vanilla's RCON thread: one `read()` must hold exactly one
    /// packet, or the connection is dropped. The pause lets anything else
    /// the client sent arrive first, so sending two packets at once fails
    /// here the way it does against a real server.
    async fn read_packet(stream: &mut TcpStream) -> Option<(i32, i32, String)> {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let mut buf = vec![0; 1460];
        let n = stream.read(&mut buf).await.ok()?;
        if n < 14 {
            return None;
        }
        let len = i32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        if len != n - 4 {
            return None;
        }
        let body = &buf[4..n];
        let id = i32::from_le_bytes(body[0..4].try_into().unwrap());
        let kind = i32::from_le_bytes(body[4..8].try_into().unwrap());
        Some((
            id,
            kind,
            String::from_utf8_lossy(&body[8..len - 2]).into_owned(),
        ))
    }

    async fn write_packet(stream: &mut TcpStream, id: i32, kind: i32, payload: &[u8]) {
        let mut p = Vec::new();
        p.extend_from_slice(&((10 + payload.len()) as i32).to_le_bytes());
        p.extend_from_slice(&id.to_le_bytes());
        p.extend_from_slice(&kind.to_le_bytes());
        p.extend_from_slice(payload);
        p.extend_from_slice(&[0, 0]);
        stream.write_all(&p).await.unwrap();
    }

    /// A fake vanilla-like RCON server. `list` returns a 9,000-byte response
    /// split across packets, like the real server does.
    async fn fake_server(password: &'static str) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut authed = false;
                    while let Some((id, kind, body)) = read_packet(&mut s).await {
                        match kind {
                            TYPE_LOGIN => {
                                authed = body == password;
                                write_packet(
                                    &mut s,
                                    if authed { id } else { -1 },
                                    TYPE_COMMAND,
                                    b"",
                                )
                                .await;
                            }
                            TYPE_COMMAND if !authed => write_packet(&mut s, -1, 0, b"").await,
                            TYPE_COMMAND if body == "list" => {
                                let big = vec![b'x'; 9000];
                                for chunk in big.chunks(4096) {
                                    write_packet(&mut s, id, 0, chunk).await;
                                }
                            }
                            TYPE_COMMAND => {
                                write_packet(&mut s, id, 0, format!("ran: {body}").as_bytes()).await
                            }
                            other => {
                                write_packet(
                                    &mut s,
                                    id,
                                    0,
                                    format!("Unknown request {other:x}").as_bytes(),
                                )
                                .await
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn logs_in_and_runs_commands() {
        let addr = fake_server("hunter2").await;
        let mut rcon = Rcon::connect(addr, "hunter2", T).await.unwrap();
        assert_eq!(rcon.command("say hi").await.unwrap(), "ran: say hi");
        assert_eq!(
            rcon.command("list").await.unwrap().len(),
            9000,
            "fragments reassembled"
        );
        assert_eq!(
            rcon.command("time query daytime").await.unwrap(),
            "ran: time query daytime"
        );
    }

    #[tokio::test]
    async fn rejects_bad_passwords() {
        let addr = fake_server("hunter2").await;
        assert!(matches!(
            Rcon::connect(addr, "wrong", T).await,
            Err(RconError::AuthFailed)
        ));
    }

    #[tokio::test]
    async fn limits_command_length_and_times_out() {
        let addr = fake_server("pw").await;
        let mut rcon = Rcon::connect(addr, "pw", T).await.unwrap();
        assert!(matches!(
            rcon.command(&"x".repeat(2000)).await,
            Err(RconError::CommandTooLong(2000))
        ));

        // A server that accepts but never answers.
        let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = silent.local_addr().unwrap();
        tokio::spawn(async move {
            let _keep = silent.accept().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let result = Rcon::connect(addr, "pw", Duration::from_millis(200)).await;
        assert!(matches!(result, Err(RconError::Timeout)));
    }
}
