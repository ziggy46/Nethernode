//! Moving bytes between the player and the backend once routing is done.
//!
//! The default path waits for a socket to become readable *before* taking
//! a buffer from a shared pool, reads, writes the bytes to the other side,
//! and returns the buffer. Idle connections therefore hold no buffer at
//! all, which keeps memory per idle player to a few KiB (tokio's
//! `copy_bidirectional` holds two buffers per connection for its lifetime).
//!
//! A Linux `splice(2)` zero-copy path was benchmarked against this and lost
//! on throughput and memory; see `BENCHMARKS.md`.

use std::sync::Mutex;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::net::tcp::{ReadHalf, WriteHalf};

/// Buffers kept for reuse. Beyond this, returned buffers are freed.
const POOL_RETAIN: usize = 256;

/// A pool of equally sized copy buffers.
#[derive(Debug)]
pub(crate) struct BufferPool {
    size: usize,
    free: Mutex<Vec<Box<[u8]>>>,
}

impl BufferPool {
    pub(crate) fn new(size: usize) -> Self {
        Self {
            size: size.max(1024),
            free: Mutex::new(Vec::new()),
        }
    }

    fn take(&self) -> Box<[u8]> {
        self.free
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .unwrap_or_else(|| vec![0u8; self.size].into_boxed_slice())
    }

    fn give(&self, buf: Box<[u8]>) {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        if free.len() < POOL_RETAIN {
            free.push(buf);
        }
    }
}

/// Copies in both directions until both sides finish, or either errors.
/// Returns (client → backend, backend → client) byte counts so far.
pub(crate) async fn copy_bidirectional(
    client: &mut TcpStream,
    backend: &mut TcpStream,
    pool: &BufferPool,
) -> (u64, u64) {
    let (client_read, mut client_write) = client.split();
    let (backend_read, mut backend_write) = backend.split();
    let (mut up, mut down) = (0u64, 0u64);
    {
        let up_fut = copy_one_way(&client_read, &mut backend_write, pool, &mut up);
        let down_fut = copy_one_way(&backend_read, &mut client_write, pool, &mut down);
        tokio::pin!(up_fut, down_fut);

        let (mut up_done, mut down_done) = (false, false);
        while !(up_done && down_done) {
            tokio::select! {
                result = &mut up_fut, if !up_done => {
                    up_done = true;
                    if result.is_err() { break; }
                }
                result = &mut down_fut, if !down_done => {
                    down_done = true;
                    if result.is_err() { break; }
                }
            }
        }
    }
    (up, down)
}

/// Copies `from` → `to` until EOF, then half-closes `to`.
async fn copy_one_way(
    from: &ReadHalf<'_>,
    to: &mut WriteHalf<'_>,
    pool: &BufferPool,
    copied: &mut u64,
) -> std::io::Result<()> {
    loop {
        from.readable().await?;
        let mut buf = pool.take();
        let read = from.try_read(&mut buf);
        let result = match read {
            Ok(0) => {
                pool.give(buf);
                return to.shutdown().await;
            }
            Ok(n) => to.write_all(&buf[..n]).await.map(|()| n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(e),
        };
        pool.give(buf);
        *copied += result? as u64;
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    async fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (a, b) = tokio::join!(TcpStream::connect(addr), listener.accept());
        (a.unwrap(), b.unwrap().0)
    }

    #[tokio::test]
    async fn copies_both_ways_and_propagates_half_close() {
        // player <-> [proxy_client | proxy_backend] <-> server
        let (mut player, mut proxy_client) = pair().await;
        let (mut proxy_backend, mut server) = pair().await;
        let pool = BufferPool::new(4096);
        let copy = tokio::spawn(async move {
            copy_bidirectional(&mut proxy_client, &mut proxy_backend, &pool).await
        });

        let big = vec![7u8; 1 << 20];
        player.write_all(&big).await.unwrap();
        let mut got = vec![0u8; big.len()];
        server.read_exact(&mut got).await.unwrap();
        assert_eq!(got, big);

        server.write_all(b"pong").await.unwrap();
        let mut pong = [0u8; 4];
        player.read_exact(&mut pong).await.unwrap();
        assert_eq!(&pong, b"pong");

        // Player half-closes; the server sees EOF but can still reply.
        player.shutdown().await.unwrap();
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        server.write_all(b"bye").await.unwrap();
        drop(server);
        let mut tail = Vec::new();
        player.read_to_end(&mut tail).await.unwrap();
        assert_eq!(tail, b"bye");

        let (up, down) = copy.await.unwrap();
        assert_eq!((up, down), (1 << 20, 7));
    }

    #[test]
    fn pool_reuses_and_caps_buffers() {
        let pool = BufferPool::new(2048);
        let a = pool.take();
        let ptr = a.as_ptr();
        pool.give(a);
        let again = pool.take();
        assert_eq!(again.as_ptr(), ptr);
        pool.give(again);
        for _ in 0..POOL_RETAIN + 10 {
            pool.give(vec![0u8; 2048].into_boxed_slice());
        }
        assert_eq!(pool.free.lock().unwrap().len(), POOL_RETAIN);
    }
}
