//! Reading frames from a socket while keeping every byte for replay.

use std::ops::Range;

use crate::{DecodeError, parse_frame};
use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Why a frame couldn't be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    /// The peer closed the connection.
    #[error("connection closed")]
    Eof,
    /// The bytes weren't a valid frame (too long, malformed length, …).
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// Socket error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Accumulates everything a client sends before splicing, so the original
/// bytes can be forwarded to the backend untouched.
#[derive(Debug, Default)]
pub struct FrameBuffer {
    buf: BytesMut,
    consumed: usize,
}

impl FrameBuffer {
    /// An empty buffer.
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(512),
            consumed: 0,
        }
    }

    /// Reads until a full frame of at most `max` payload bytes is buffered,
    /// returning the payload's range. Rejects oversize frames as soon as
    /// the length prefix arrives.
    pub async fn next_frame<S: AsyncRead + Unpin>(
        &mut self,
        stream: &mut S,
        max: usize,
    ) -> Result<Range<usize>, ReadError> {
        loop {
            if let Some(frame) = parse_frame(&self.buf[self.consumed..], max)? {
                let start = self.consumed;
                self.consumed += frame.total;
                return Ok(start + frame.payload.start..start + frame.payload.end);
            }
            self.read_more(stream).await?;
        }
    }

    /// Reads whatever is available into the buffer.
    pub async fn read_more<S: AsyncRead + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> Result<usize, ReadError> {
        if self.buf.capacity() - self.buf.len() < 256 {
            self.buf.reserve(1024);
        }
        match stream.read_buf(&mut self.buf).await? {
            0 => Err(ReadError::Eof),
            n => Ok(n),
        }
    }

    /// Bytes of a payload range returned by [`FrameBuffer::next_frame`].
    pub fn payload(&self, range: Range<usize>) -> &[u8] {
        &self.buf[range]
    }

    /// Everything received so far, including bytes past the last frame.
    pub fn all(&self) -> &[u8] {
        &self.buf
    }

    /// Bytes received but not yet consumed as frames.
    pub fn unconsumed(&self) -> &[u8] {
        &self.buf[self.consumed..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_frames_across_chunks_and_keeps_bytes() {
        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            client.write_all(&[0x03, 0x00]).await.unwrap();
            client
                .write_all(&[0x01, 0x02, 0x01, 0x07, 0xaa])
                .await
                .unwrap();
        });
        let mut buf = FrameBuffer::new();
        let first = buf.next_frame(&mut server, 16).await.unwrap();
        assert_eq!(buf.payload(first), &[0x00, 0x01, 0x02]);
        let second = buf.next_frame(&mut server, 16).await.unwrap();
        assert_eq!(buf.payload(second), &[0x07]);
        assert_eq!(buf.unconsumed(), &[0xaa]);
        assert_eq!(buf.all().len(), 7);
        // 0xaa starts a VarInt that never finishes before the stream closes.
        assert!(matches!(
            buf.next_frame(&mut server, 16).await,
            Err(ReadError::Eof)
        ));
    }

    #[tokio::test]
    async fn rejects_oversize_frames_early() {
        let (mut client, mut server) = tokio::io::duplex(64);
        use tokio::io::AsyncWriteExt;
        client.write_all(&[0xff, 0x7f]).await.unwrap(); // 16383-byte frame
        let mut buf = FrameBuffer::new();
        assert!(matches!(
            buf.next_frame(&mut server, 2048).await,
            Err(ReadError::Decode(DecodeError::FrameTooLarge { .. }))
        ));
    }
}
