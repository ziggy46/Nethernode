//! yamux streams over the encrypted connection. One task drives the
//! connection; [`Session`] opens streams and [`Inbound`] receives the ones
//! the other side opens.

use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;

use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};
use tokio_util::sync::CancellationToken;

use crate::noise::NoiseStream;

/// One stream over the link: a reliable, ordered byte stream.
pub type LinkStream = Compat<yamux::Stream>;

type OpenReply = oneshot::Sender<Result<yamux::Stream, yamux::ConnectionError>>;

/// Opens streams on a link. Clones share the link; it closes when
/// [`Session::close`] is called, the other side goes away, or every clone
/// is dropped.
#[derive(Clone)]
pub struct Session {
    opens: mpsc::Sender<OpenReply>,
    closed: watch::Receiver<bool>,
    cancel: CancellationToken,
}

/// Streams the other side opens.
pub struct Inbound {
    streams: mpsc::Receiver<yamux::Stream>,
}

impl Inbound {
    /// The next stream, or `None` once the link is closed.
    pub async fn next(&mut self) -> Option<LinkStream> {
        self.streams.recv().await.map(|s| s.compat())
    }
}

impl Session {
    pub(crate) fn start(io: NoiseStream, mode: yamux::Mode) -> (Self, Inbound) {
        let (opens_tx, opens) = mpsc::channel::<OpenReply>(16);
        let (inbound_tx, inbound) = mpsc::channel(32);
        let (closed_tx, closed) = watch::channel(false);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            if let Err(e) = drive(io, mode, opens, inbound_tx, stop).await {
                tracing::debug!(error = %e, "link closed with an error");
            }
            let _ = closed_tx.send(true);
        });
        (
            Self {
                opens: opens_tx,
                closed,
                cancel,
            },
            Inbound { streams: inbound },
        )
    }

    /// Opens a new stream.
    pub async fn open(&self) -> std::io::Result<LinkStream> {
        let (tx, rx) = oneshot::channel();
        let gone = || std::io::Error::new(std::io::ErrorKind::NotConnected, "the link is closed");
        self.opens.send(tx).await.map_err(|_| gone())?;
        match rx.await {
            Ok(Ok(stream)) => Ok(stream.compat()),
            Ok(Err(e)) => Err(std::io::Error::other(e)),
            Err(_) => Err(gone()),
        }
    }

    /// Whether the link has closed.
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Waits until the link closes.
    pub async fn closed(&self) {
        let mut closed = self.closed.clone();
        let _ = closed.wait_for(|c| *c).await;
    }

    /// Closes the link.
    pub fn close(&self) {
        self.cancel.cancel();
    }
}

async fn drive(
    io: NoiseStream,
    mode: yamux::Mode,
    mut opens: mpsc::Receiver<OpenReply>,
    inbound: mpsc::Sender<yamux::Stream>,
    cancel: CancellationToken,
) -> Result<(), yamux::ConnectionError> {
    let mut conn = yamux::Connection::new(io, yamux::Config::default(), mode);
    let mut pending: VecDeque<OpenReply> = VecDeque::new();
    let mut cancelled = pin!(cancel.cancelled());
    let mut closing = false;
    poll_fn(|cx| {
        loop {
            if !closing {
                if cancelled.as_mut().poll(cx).is_ready() {
                    closing = true;
                }
                loop {
                    match opens.poll_recv(cx) {
                        Poll::Ready(Some(reply)) => pending.push_back(reply),
                        // Every Session is gone: nobody can use the link.
                        Poll::Ready(None) => {
                            closing = true;
                            break;
                        }
                        Poll::Pending => break,
                    }
                }
            }
            if closing {
                return conn.poll_close(cx);
            }
            while !pending.is_empty() {
                match conn.poll_new_outbound(cx) {
                    Poll::Ready(result) => {
                        if let Some(reply) = pending.pop_front() {
                            let _ = reply.send(result);
                        }
                    }
                    Poll::Pending => break,
                }
            }
            match conn.poll_next_inbound(cx) {
                Poll::Ready(Some(Ok(stream))) => {
                    // A full queue drops (resets) the stream.
                    let _ = inbound.try_send(stream);
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    })
    .await
}
