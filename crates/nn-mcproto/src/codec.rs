//! Bounds-checked readers and writers for protocol data types.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Data_types>

use uuid::Uuid;

use crate::error::{DecodeError, Result};
use crate::varint::{read_varint, read_varlong, write_varint};

/// A cursor over a packet payload. Every read is bounds-checked.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Reads from the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not yet read.
    pub fn remaining(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    /// Bytes consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Reads exactly `n` bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Incomplete)?;
        let slice = self.buf.get(self.pos..end).ok_or(DecodeError::Incomplete)?;
        self.pos = end;
        Ok(slice)
    }

    /// Reads a VarInt.
    pub fn varint(&mut self) -> Result<i32> {
        let (value, len) = read_varint(self.remaining())?;
        self.pos += len;
        Ok(value)
    }

    /// Reads a VarLong.
    pub fn varlong(&mut self) -> Result<i64> {
        let (value, len) = read_varlong(self.remaining())?;
        self.pos += len;
        Ok(value)
    }

    /// Reads one byte.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    /// Reads a boolean (any non-zero byte is true).
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    /// Reads a big-endian unsigned short.
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// Reads a big-endian signed long.
    pub fn i64(&mut self) -> Result<i64> {
        let b = self.bytes(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(i64::from_be_bytes(arr))
    }

    /// Reads a 128-bit UUID (two big-endian longs).
    pub fn uuid(&mut self) -> Result<Uuid> {
        let b = self.bytes(16)?;
        let mut arr = [0u8; 16];
        arr.copy_from_slice(b);
        Ok(Uuid::from_bytes(arr))
    }

    /// Reads a VarInt-length-prefixed UTF-8 string of at most `max_chars`
    /// UTF-16 code units (the protocol's unit for string limits).
    pub fn string(&mut self, max_chars: usize) -> Result<&'a str> {
        let len = self.length()?;
        // Each UTF-16 code unit takes at most 3 UTF-8 bytes.
        if len > max_chars.saturating_mul(3) {
            return Err(DecodeError::StringTooLong { max: max_chars });
        }
        let bytes = self.bytes(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)?;
        if s.encode_utf16().count() > max_chars {
            return Err(DecodeError::StringTooLong { max: max_chars });
        }
        Ok(s)
    }

    /// Reads a VarInt-length-prefixed byte array of at most `max` bytes.
    pub fn byte_array(&mut self, max: usize) -> Result<&'a [u8]> {
        let len = self.length()?;
        if len > max {
            return Err(DecodeError::InvalidValue("byte array length"));
        }
        self.bytes(len)
    }

    fn length(&mut self) -> Result<usize> {
        let len = self.varint()?;
        usize::try_from(len).map_err(|_| DecodeError::NegativeLength)
    }
}

/// Appends a VarInt-length-prefixed UTF-8 string.
pub fn write_string(out: &mut Vec<u8>, s: &str) {
    write_varint(out, i32::try_from(s.len()).unwrap_or(i32::MAX));
    out.extend_from_slice(s.as_bytes());
}

/// Wraps `payload` (packet ID + fields) in a length-prefixed frame.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 3);
    write_varint(&mut out, i32::try_from(payload.len()).unwrap_or(i32::MAX));
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_primitives() {
        let mut data = vec![0x01, 0x63, 0xdd];
        data.extend_from_slice(&42i64.to_be_bytes());
        let mut r = Reader::new(&data);
        assert!(r.bool().unwrap());
        assert_eq!(r.u16().unwrap(), 25565);
        assert_eq!(r.i64().unwrap(), 42);
        assert_eq!(r.u8(), Err(DecodeError::Incomplete));
    }

    #[test]
    fn string_limits_count_utf16_units() {
        let mut out = Vec::new();
        write_string(&mut out, "héllo");
        assert_eq!(Reader::new(&out).string(5).unwrap(), "héllo");
        assert!(matches!(
            Reader::new(&out).string(4),
            Err(DecodeError::StringTooLong { .. })
        ));

        // An emoji is one char but two UTF-16 code units.
        let mut out = Vec::new();
        write_string(&mut out, "🙂");
        assert!(Reader::new(&out).string(1).is_err());
        assert!(Reader::new(&out).string(2).is_ok());
    }

    #[test]
    fn rejects_negative_and_oversized_lengths() {
        // Length -1.
        let data = [0xff, 0xff, 0xff, 0xff, 0x0f];
        assert_eq!(
            Reader::new(&data).string(10),
            Err(DecodeError::NegativeLength)
        );
        // Declared length far beyond the cap fails before reading.
        let data = [0xff, 0x7f];
        assert!(matches!(
            Reader::new(&data).string(10),
            Err(DecodeError::StringTooLong { .. })
        ));
        assert_eq!(
            Reader::new(&[0x02, 0xff, 0xfe]).string(10),
            Err(DecodeError::InvalidUtf8)
        );
    }

    #[test]
    fn frame_prefixes_length() {
        assert_eq!(frame(&[0x00, 0x01]), vec![0x02, 0x00, 0x01]);
    }
}
