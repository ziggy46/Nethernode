//! Length-prefixed frames (uncompressed, as used before login completes).
//!
//! `parse_frame` works on a buffer that may hold a partial frame, so the
//! proxy can read from the socket incrementally and reject oversize frames
//! as soon as the length prefix arrives, before buffering the body.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Packets#Without_compression>

use std::ops::Range;

use crate::error::{DecodeError, Result};
use crate::varint::read_varint;

/// Maximum bytes of a pre-login length prefix (packet length ≤ 2,097,151).
pub const MAX_LENGTH_PREFIX: usize = 3;

/// A complete frame located within a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRange {
    /// Payload (packet ID + fields), excluding the length prefix.
    pub payload: Range<usize>,
    /// Total bytes the frame occupies, including the length prefix.
    pub total: usize,
}

/// Locates the first frame in `buf`.
///
/// Returns `Ok(None)` if more bytes are needed. Fails as soon as the length
/// prefix shows the frame would exceed `max_len` payload bytes.
pub fn parse_frame(buf: &[u8], max_len: usize) -> Result<Option<FrameRange>> {
    let (len, prefix) = match read_varint(buf) {
        Ok(v) => v,
        Err(DecodeError::Incomplete) if buf.len() < MAX_LENGTH_PREFIX => return Ok(None),
        Err(DecodeError::Incomplete) => return Err(DecodeError::VarIntTooLong),
        Err(e) => return Err(e),
    };
    if prefix > MAX_LENGTH_PREFIX {
        return Err(DecodeError::VarIntTooLong);
    }
    let len = usize::try_from(len).map_err(|_| DecodeError::NegativeLength)?;
    if len == 0 {
        return Err(DecodeError::EmptyFrame);
    }
    if len > max_len {
        return Err(DecodeError::FrameTooLarge { len, max: max_len });
    }
    let total = prefix + len;
    if buf.len() < total {
        return Ok(None);
    }
    Ok(Some(FrameRange {
        payload: prefix..total,
        total,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_complete_frames() {
        let buf = [0x02, 0xaa, 0xbb, 0xcc];
        assert_eq!(
            parse_frame(&buf, 10).unwrap(),
            Some(FrameRange {
                payload: 1..3,
                total: 3
            })
        );
    }

    #[test]
    fn waits_for_more_bytes() {
        assert_eq!(parse_frame(&[], 10).unwrap(), None);
        assert_eq!(parse_frame(&[0x03, 0x00], 10).unwrap(), None);
        // Partial length prefix.
        assert_eq!(parse_frame(&[0x80], 1000).unwrap(), None);
    }

    #[test]
    fn rejects_oversize_before_body_arrives() {
        // Declares 300 bytes with only the prefix present.
        assert_eq!(
            parse_frame(&[0xac, 0x02], 256),
            Err(DecodeError::FrameTooLarge { len: 300, max: 256 })
        );
    }

    #[test]
    fn rejects_bad_prefixes() {
        assert_eq!(parse_frame(&[0x00], 10), Err(DecodeError::EmptyFrame));
        assert_eq!(
            parse_frame(&[0x80, 0x80, 0x80, 0x01], 10),
            Err(DecodeError::VarIntTooLong)
        );
        assert_eq!(
            parse_frame(&[0x80, 0x80, 0x80], 10),
            Err(DecodeError::VarIntTooLong)
        );
        assert_eq!(
            parse_frame(&[0xff, 0xff, 0xff, 0xff, 0x0f], 10),
            Err(DecodeError::VarIntTooLong)
        );
    }
}
