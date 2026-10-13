//! A small, bounded reader for Minecraft's NBT format (big-endian Java
//! edition), as used by `world/playerdata/<uuid>.dat` (gzip-compressed).
//!
//! Files can be uploaded through the file manager, so input is untrusted:
//! nesting depth, element counts, and decompressed size are all capped,
//! and lengths are checked against the remaining input before allocating.

use std::io::Read;

/// A parsed tag.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    /// TAG_Byte.
    Byte(i8),
    /// TAG_Short.
    Short(i16),
    /// TAG_Int.
    Int(i32),
    /// TAG_Long.
    Long(i64),
    /// TAG_Float.
    Float(f32),
    /// TAG_Double.
    Double(f64),
    /// TAG_Byte_Array.
    ByteArray(Vec<i8>),
    /// TAG_String.
    String(String),
    /// TAG_List.
    List(Vec<Tag>),
    /// TAG_Compound, in file order.
    Compound(Vec<(String, Tag)>),
    /// TAG_Int_Array.
    IntArray(Vec<i32>),
    /// TAG_Long_Array.
    LongArray(Vec<i64>),
}

impl Tag {
    /// A compound's child by name.
    pub fn get(&self, key: &str) -> Option<&Tag> {
        match self {
            Tag::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Any integer tag as `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Tag::Byte(v) => Some(v.into()),
            Tag::Short(v) => Some(v.into()),
            Tag::Int(v) => Some(v.into()),
            Tag::Long(v) => Some(v),
            _ => None,
        }
    }

    /// Any number as `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Tag::Float(v) => Some(v.into()),
            Tag::Double(v) => Some(v),
            _ => self.as_i64().map(|v| v as f64),
        }
    }

    /// A string tag.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }

    /// A list tag's items.
    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Tag::List(items) => Some(items),
            _ => None,
        }
    }
}

/// Why a file couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NbtError {
    /// Ran out of input.
    #[error("the file ends early")]
    Eof,
    /// An unknown tag type.
    #[error("unknown tag type {0}")]
    BadTag(u8),
    /// A negative or impossible length.
    #[error("invalid length")]
    BadLength,
    /// Over a safety limit.
    #[error("the file is too large or too deeply nested")]
    TooLarge,
    /// Not gzip data.
    #[error("the file isn't compressed NBT")]
    Compression,
}

/// Limits on what a file may contain.
const MAX_DEPTH: usize = 64;
const MAX_TAGS: usize = 2_000_000;
/// Largest decompressed size accepted.
pub const MAX_DECOMPRESSED: u64 = 32 * 1024 * 1024;

/// Reads a gzip-compressed NBT file (player data). Returns the root
/// compound's name and value.
pub fn read_gzip(bytes: &[u8]) -> Result<(String, Tag), NbtError> {
    let mut decoder = flate2::read::MultiGzDecoder::new(bytes).take(MAX_DECOMPRESSED + 1);
    let mut raw = Vec::new();
    decoder
        .read_to_end(&mut raw)
        .map_err(|_| NbtError::Compression)?;
    if raw.len() as u64 > MAX_DECOMPRESSED {
        return Err(NbtError::TooLarge);
    }
    read(&raw)
}

/// Reads uncompressed NBT: one named root tag.
pub fn read(bytes: &[u8]) -> Result<(String, Tag), NbtError> {
    let mut reader = Reader {
        input: bytes,
        tags: 0,
    };
    let kind = reader.u8()?;
    if kind != 10 {
        return Err(NbtError::BadTag(kind));
    }
    let name = reader.string()?;
    let root = reader.payload(kind, 0)?;
    Ok((name, root))
}

struct Reader<'a> {
    input: &'a [u8],
    tags: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], NbtError> {
        if self.input.len() < n {
            return Err(NbtError::Eof);
        }
        let (head, rest) = self.input.split_at(n);
        self.input = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], NbtError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.take(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, NbtError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    /// A length prefix, checked against what's left at `size` bytes each.
    fn len(&mut self, size: usize) -> Result<usize, NbtError> {
        let n = usize::try_from(self.i32()?).map_err(|_| NbtError::BadLength)?;
        if n.saturating_mul(size.max(1)) > self.input.len() {
            return Err(NbtError::Eof);
        }
        Ok(n)
    }

    fn string(&mut self) -> Result<String, NbtError> {
        let n = u16::from_be_bytes(self.array()?) as usize;
        // Java's modified UTF-8; lossy decoding is fine for display.
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    fn payload(&mut self, kind: u8, depth: usize) -> Result<Tag, NbtError> {
        self.tags += 1;
        if depth > MAX_DEPTH || self.tags > MAX_TAGS {
            return Err(NbtError::TooLarge);
        }
        Ok(match kind {
            1 => Tag::Byte(i8::from_be_bytes(self.array()?)),
            2 => Tag::Short(i16::from_be_bytes(self.array()?)),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(i64::from_be_bytes(self.array()?)),
            5 => Tag::Float(f32::from_be_bytes(self.array()?)),
            6 => Tag::Double(f64::from_be_bytes(self.array()?)),
            7 => {
                let n = self.len(1)?;
                Tag::ByteArray(self.take(n)?.iter().map(|&b| b as i8).collect())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let item = self.u8()?;
                // Every element is at least one byte (empty compounds are one).
                let n = self.len(usize::from(item != 0))?;
                if item == 0 && n > 0 {
                    return Err(NbtError::BadTag(0));
                }
                let mut items = Vec::with_capacity(n.min(4096));
                for _ in 0..n {
                    items.push(self.payload(item, depth + 1)?);
                }
                Tag::List(items)
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let child = self.u8()?;
                    if child == 0 {
                        break;
                    }
                    let name = self.string()?;
                    entries.push((name, self.payload(child, depth + 1)?));
                }
                Tag::Compound(entries)
            }
            11 => {
                let n = self.len(4)?;
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    out.push(self.i32()?);
                }
                Tag::IntArray(out)
            }
            12 => {
                let n = self.len(8)?;
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    out.push(i64::from_be_bytes(self.array()?));
                }
                Tag::LongArray(out)
            }
            other => return Err(NbtError::BadTag(other)),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// Builds NBT bytes by hand.
    fn named(kind: u8, name: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend_from_slice(&(name.len() as u16).to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn sample() -> Vec<u8> {
        let mut root = Vec::new();
        root.extend(named(5, "Health", &20.0f32.to_be_bytes()));
        root.extend(named(
            8,
            "Dimension",
            &[&[0u8, 19][..], b"minecraft:overworld"].concat(),
        ));
        // Pos: list of 3 doubles.
        let mut pos = vec![6u8];
        pos.extend_from_slice(&3i32.to_be_bytes());
        for v in [1.5f64, 64.0, -3.25] {
            pos.extend_from_slice(&v.to_be_bytes());
        }
        root.extend(named(9, "Pos", &pos));
        // Inventory: one item compound.
        let mut item = Vec::new();
        item.extend(named(1, "Slot", &[0]));
        item.extend(named(
            8,
            "id",
            &[&[0u8, 17][..], b"minecraft:diamond"].concat(),
        ));
        item.extend(named(3, "count", &64i32.to_be_bytes()));
        item.push(0);
        let mut inv = vec![10u8];
        inv.extend_from_slice(&1i32.to_be_bytes());
        inv.extend(item);
        root.extend(named(9, "Inventory", &inv));
        root.push(0);
        named(10, "", &root)
    }

    #[test]
    fn reads_player_shaped_data() {
        let (name, root) = read(&sample()).unwrap();
        assert_eq!(name, "");
        assert_eq!(root.get("Health").and_then(Tag::as_f64), Some(20.0));
        assert_eq!(
            root.get("Dimension").and_then(Tag::as_str),
            Some("minecraft:overworld")
        );
        let pos: Vec<f64> = root
            .get("Pos")
            .unwrap()
            .as_list()
            .unwrap()
            .iter()
            .filter_map(Tag::as_f64)
            .collect();
        assert_eq!(pos, vec![1.5, 64.0, -3.25]);
        let item = &root.get("Inventory").unwrap().as_list().unwrap()[0];
        assert_eq!(item.get("count").and_then(Tag::as_i64), Some(64));
    }

    #[test]
    fn gzip_round_trip() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&sample()).unwrap();
        let bytes = gz.finish().unwrap();
        assert!(read_gzip(&bytes).is_ok());
        assert_eq!(read_gzip(b"not gzip"), Err(NbtError::Compression));
    }

    #[test]
    fn hostile_input_is_refused_cheaply() {
        // A list claiming two billion entries in a tiny file.
        let mut list = vec![3u8];
        list.extend_from_slice(&i32::MAX.to_be_bytes());
        let mut root = named(9, "x", &list);
        root.push(0);
        assert_eq!(read(&named(10, "", &root)), Err(NbtError::Eof));
        // Negative lengths.
        let mut bad = vec![1u8];
        bad.extend_from_slice(&(-1i32).to_be_bytes());
        let mut root = named(7, "x", &bad[1..]);
        root.push(0);
        assert_eq!(read(&named(10, "", &root)), Err(NbtError::BadLength));
        // Deep nesting.
        let mut deep = Vec::new();
        for _ in 0..200 {
            deep.extend([10u8, 0, 1, b'a']);
        }
        assert_eq!(read(&named(10, "", &deep)), Err(NbtError::TooLarge));
        // Truncation anywhere is an error, never a panic.
        let full = sample();
        for cut in 0..full.len() {
            assert!(read(&full[..cut]).is_err());
        }
        assert_eq!(read(&[]), Err(NbtError::Eof));
    }
}
