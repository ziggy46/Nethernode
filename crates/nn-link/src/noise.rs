//! Noise XX over frames, and the Hello/Welcome exchange that follows.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::Bytes;
use futures_util::io::{AsyncRead, AsyncWrite};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::frames::Frames;
use crate::messages::{self, Hello, Welcome};
use crate::session::{Inbound, Session};

const PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
/// Largest Noise message, tag included.
const MAX_NOISE: usize = 65_535;
/// Largest plaintext in one message (the AEAD tag is 16 bytes).
const MAX_PLAINTEXT: usize = MAX_NOISE - 16;

/// Why a link couldn't be set up, or ended.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The transport failed.
    #[error("connection failed: {0}")]
    Io(#[from] io::Error),
    /// The encryption handshake failed.
    #[error("encryption handshake failed: {0}")]
    Noise(#[from] snow::Error),
    /// The other side closed the connection.
    #[error("the connection closed")]
    Closed,
    /// The panel isn't the one this agent joined.
    #[error("the panel's key doesn't match the one this agent joined; refusing to connect")]
    WrongPanel,
    /// The other side broke the protocol.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// The panel turned the agent away.
    #[error("the panel refused this agent: {0}")]
    Rejected(String),
}

/// A Noise static key pair (X25519).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keypair {
    /// Private key.
    #[serde(with = "hex32")]
    pub private: [u8; 32],
    /// Public key.
    #[serde(with = "hex32")]
    pub public: [u8; 32],
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair")
            .field("public", &hex::encode(self.public))
            .finish_non_exhaustive()
    }
}

impl Keypair {
    /// A new random key pair.
    pub fn generate() -> Self {
        let pair = snow::Builder::new(params())
            .generate_keypair()
            .expect("X25519 key generation is available");
        Self {
            private: pair.private.try_into().expect("X25519 keys are 32 bytes"),
            public: pair.public.try_into().expect("X25519 keys are 32 bytes"),
        }
    }
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

fn params() -> snow::params::NoiseParams {
    PARAMS.parse().expect("valid Noise parameters")
}

async fn send_frame(frames: &mut Frames, bytes: &[u8]) -> Result<(), LinkError> {
    frames.send(Bytes::copy_from_slice(bytes)).await?;
    Ok(())
}

async fn recv_frame(frames: &mut Frames) -> Result<Bytes, LinkError> {
    match frames.next().await {
        Some(Ok(frame)) if frame.len() <= MAX_NOISE => Ok(frame),
        Some(Ok(_)) => Err(LinkError::Protocol("frame too large".into())),
        Some(Err(e)) => Err(e.into()),
        None => Err(LinkError::Closed),
    }
}

/// Connects as an agent: checks that the panel holds `panel_key`, says
/// [`Hello`], and returns the session once the panel accepts.
pub async fn connect(
    mut frames: Frames,
    me: &Keypair,
    panel_key: &[u8; 32],
    hello: &Hello,
) -> Result<(Welcome, Session, Inbound), LinkError> {
    let mut hs = snow::Builder::new(params())
        .local_private_key(&me.private)?
        .build_initiator()?;
    let mut buf = vec![0u8; MAX_NOISE];
    let n = hs.write_message(&[], &mut buf)?;
    send_frame(&mut frames, &buf[..n]).await?;
    let reply = recv_frame(&mut frames).await?;
    hs.read_message(&reply, &mut buf)?;
    if hs.get_remote_static() != Some(&panel_key[..]) {
        return Err(LinkError::WrongPanel);
    }
    let n = hs.write_message(&[], &mut buf)?;
    send_frame(&mut frames, &buf[..n]).await?;
    let mut io = NoiseStream::new(frames, hs.into_transport_mode()?);

    let welcome: Welcome = {
        let mut control = (&mut io).compat();
        messages::send(&mut control, hello).await?;
        messages::recv(&mut control)
            .await?
            .ok_or(LinkError::Closed)?
    };
    match &welcome {
        Welcome::Accepted { .. } => {
            let (session, inbound) = Session::start(io, yamux::Mode::Client);
            Ok((welcome, session, inbound))
        }
        Welcome::Rejected { reason } => Err(LinkError::Rejected(reason.clone())),
    }
}

/// Accepts an agent's connection as the panel. The agent's key and Hello
/// say who it claims to be; [`Handshaken::admit`] or
/// [`Handshaken::reject`] answers.
pub async fn accept(mut frames: Frames, me: &Keypair) -> Result<Handshaken, LinkError> {
    let mut hs = snow::Builder::new(params())
        .local_private_key(&me.private)?
        .build_responder()?;
    let mut buf = vec![0u8; MAX_NOISE];
    let first = recv_frame(&mut frames).await?;
    hs.read_message(&first, &mut buf)?;
    let n = hs.write_message(&[], &mut buf)?;
    send_frame(&mut frames, &buf[..n]).await?;
    let last = recv_frame(&mut frames).await?;
    hs.read_message(&last, &mut buf)?;
    let agent_key: [u8; 32] = hs
        .get_remote_static()
        .and_then(|k| k.try_into().ok())
        .ok_or_else(|| LinkError::Protocol("no agent key".into()))?;
    let mut io = NoiseStream::new(frames, hs.into_transport_mode()?);
    let hello: Hello = messages::recv(&mut (&mut io).compat())
        .await?
        .ok_or(LinkError::Closed)?;
    Ok(Handshaken {
        agent_key,
        hello,
        io,
    })
}

/// An agent whose encrypted connection is up but who hasn't been let in.
pub struct Handshaken {
    /// The agent's public key, proven by the handshake.
    pub agent_key: [u8; 32],
    /// What the agent said.
    pub hello: Hello,
    io: NoiseStream,
}

impl Handshaken {
    /// Lets the agent in as node `node_id`.
    pub async fn admit(
        mut self,
        node_id: &str,
        name: &str,
    ) -> Result<(Session, Inbound), LinkError> {
        let welcome = Welcome::Accepted {
            node_id: node_id.to_owned(),
            name: name.to_owned(),
        };
        messages::send(&mut (&mut self.io).compat(), &welcome).await?;
        Ok(Session::start(self.io, yamux::Mode::Server))
    }

    /// Turns the agent away, telling it why.
    pub async fn reject(mut self, reason: &str) {
        let welcome = Welcome::Rejected {
            reason: reason.to_owned(),
        };
        let _ = messages::send(&mut (&mut self.io).compat(), &welcome).await;
        let _ = self.io.frames.close().await;
    }
}

/// A byte stream encrypted with Noise, one message per frame.
pub(crate) struct NoiseStream {
    frames: Frames,
    state: snow::TransportState,
    plain: Vec<u8>,
    plain_at: usize,
    scratch: Vec<u8>,
}

impl NoiseStream {
    fn new(frames: Frames, state: snow::TransportState) -> Self {
        Self {
            frames,
            state,
            plain: Vec::new(),
            plain_at: 0,
            scratch: vec![0u8; MAX_NOISE],
        }
    }
}

fn noise_io(e: snow::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

impl AsyncRead for NoiseStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            if this.plain_at < this.plain.len() {
                let n = out.len().min(this.plain.len() - this.plain_at);
                out[..n].copy_from_slice(&this.plain[this.plain_at..this.plain_at + n]);
                this.plain_at += n;
                return Poll::Ready(Ok(n));
            }
            match ready!(Pin::new(&mut this.frames).poll_next(cx)) {
                Some(Ok(frame)) => {
                    if frame.len() > MAX_NOISE {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "frame too large",
                        )));
                    }
                    this.plain.resize(MAX_NOISE, 0);
                    let n = this
                        .state
                        .read_message(&frame, &mut this.plain)
                        .map_err(noise_io)?;
                    this.plain.truncate(n);
                    this.plain_at = 0;
                }
                Some(Err(e)) => return Poll::Ready(Err(e)),
                None => return Poll::Ready(Ok(0)),
            }
        }
    }
}

impl AsyncWrite for NoiseStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        ready!(Pin::new(&mut this.frames).poll_ready(cx))?;
        let n = data.len().min(MAX_PLAINTEXT);
        let len = this
            .state
            .write_message(&data[..n], &mut this.scratch)
            .map_err(noise_io)?;
        Pin::new(&mut this.frames).start_send(Bytes::copy_from_slice(&this.scratch[..len]))?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().frames).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().frames).poll_close(cx)
    }
}
