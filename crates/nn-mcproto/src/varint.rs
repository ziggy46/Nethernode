//! VarInt and VarLong: little-endian base-128 integers.
//!
//! A VarInt is at most 5 bytes and a VarLong at most 10; longer encodings
//! are rejected. Non-minimal encodings within those limits are accepted, as
//! the vanilla client and server do.
//! Source: <https://minecraft.wiki/w/Java_Edition_protocol/Data_types#VarInt_and_VarLong>

use crate::error::{DecodeError, Result};

/// Maximum encoded size of a VarInt.
pub const VARINT_MAX_BYTES: usize = 5;
/// Maximum encoded size of a VarLong.
pub const VARLONG_MAX_BYTES: usize = 10;

/// Decodes a VarInt from the start of `buf`, returning the value and the
/// number of bytes it used.
///
/// Returns [`DecodeError::Incomplete`] if `buf` ends mid-number and
/// [`DecodeError::VarIntTooLong`] if a 5th byte still has the continuation bit.
pub fn read_varint(buf: &[u8]) -> Result<(i32, usize)> {
    let mut value: u32 = 0;
    for i in 0..VARINT_MAX_BYTES {
        let Some(&byte) = buf.get(i) else {
            return Err(DecodeError::Incomplete);
        };
        value |= u32::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value as i32, i + 1));
        }
    }
    Err(DecodeError::VarIntTooLong)
}

/// Decodes a VarLong from the start of `buf`.
pub fn read_varlong(buf: &[u8]) -> Result<(i64, usize)> {
    let mut value: u64 = 0;
    for i in 0..VARLONG_MAX_BYTES {
        let Some(&byte) = buf.get(i) else {
            return Err(DecodeError::Incomplete);
        };
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok((value as i64, i + 1));
        }
    }
    Err(DecodeError::VarIntTooLong)
}

/// Appends `value` as a VarInt.
pub fn write_varint(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7f == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
}

/// Appends `value` as a VarLong.
pub fn write_varlong(out: &mut Vec<u8>, value: i64) {
    let mut v = value as u64;
    loop {
        if v & !0x7f == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
}

/// Number of bytes `value` occupies as a VarInt.
pub fn varint_len(value: i32) -> usize {
    match value as u32 {
        0..=0x7f => 1,
        0x80..=0x3fff => 2,
        0x4000..=0x1f_ffff => 3,
        0x20_0000..=0xfff_ffff => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn known_encodings() {
        // Examples from the protocol documentation.
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (25565, &[0xdd, 0xc7, 0x01]),
            (2_097_151, &[0xff, 0xff, 0x7f]),
            (i32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for &(value, bytes) in cases {
            let mut out = Vec::new();
            write_varint(&mut out, value);
            assert_eq!(out, bytes, "encode {value}");
            assert_eq!(
                read_varint(bytes).unwrap(),
                (value, bytes.len()),
                "decode {value}"
            );
            assert_eq!(varint_len(value), bytes.len());
        }
    }

    #[test]
    fn rejects_overlong_and_incomplete() {
        assert_eq!(
            read_varint(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(DecodeError::VarIntTooLong)
        );
        assert_eq!(read_varint(&[0x80, 0x80]), Err(DecodeError::Incomplete));
        assert_eq!(read_varint(&[]), Err(DecodeError::Incomplete));
        assert_eq!(read_varlong(&[0x80; 11]), Err(DecodeError::VarIntTooLong));
    }

    #[test]
    fn accepts_non_minimal_encoding() {
        assert_eq!(read_varint(&[0x81, 0x80, 0x00]).unwrap(), (1, 3));
    }

    proptest! {
        #[test]
        fn varint_round_trips(value: i32, trailing in proptest::collection::vec(any::<u8>(), 0..4)) {
            let mut out = Vec::new();
            write_varint(&mut out, value);
            prop_assert_eq!(out.len(), varint_len(value));
            out.extend_from_slice(&trailing);
            prop_assert_eq!(read_varint(&out).unwrap(), (value, varint_len(value)));
        }

        #[test]
        fn varlong_round_trips(value: i64) {
            let mut out = Vec::new();
            write_varlong(&mut out, value);
            prop_assert!(out.len() <= VARLONG_MAX_BYTES);
            prop_assert_eq!(read_varlong(&out).unwrap(), (value, out.len()));
        }

        #[test]
        fn varint_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..8)) {
            let _ = read_varint(&bytes);
        }
    }
}
