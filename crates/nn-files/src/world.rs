//! A world's dimensions (how big each is, how many chunks it has), and
//! trimming the chunks nobody has spent time in, so they generate again
//! when someone gets there.
//!
//! Region files (`r.<x>.<z>.mca` in `region/`, `entities/` and `poi/`): an
//! 8 KiB header of 1024 locations (a 3-byte big-endian offset and a 1-byte
//! length, in 4 KiB sectors) and 1024 timestamps, then each chunk as a
//! 4-byte big-endian length, a compression byte (1 gzip, 2 zlib,
//! 3 none, 4 LZ4; +128 when the data is in an external `c.<x>.<z>.mcc`
//! file) and the data. A chunk's index is `x + 32 z` of its coordinates
//! within the region. Chunks record `InhabitedTime` (ticks players spent in
//! them) at the top of their NBT since 1.18, and under `Level` before.
//!
//! Anything that can't be read is kept: external and LZ4 chunks, unknown
//! compression, damaged data.

use std::io::Read;

use crate::error::{FileError, Result};
use crate::path::RelPath;
use crate::sandbox::{EntryKind, Expect, ServerFiles};

const SECTOR: usize = 4096;
const HEADER: usize = 2 * SECTOR;

/// One dimension of a world.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Dimension {
    /// `minecraft:overworld`, `minecraft:the_nether`, `minecraft:the_end`,
    /// or a mod's (`<namespace>:<name>`).
    pub id: String,
    /// Its folder (holding `region/`).
    pub path: String,
    /// Bytes in its region, entity and POI files.
    pub bytes: u64,
    /// Chunks saved in its region files.
    pub chunks: u64,
}

/// What to trim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrimOptions {
    /// Chunks players spent less than this many ticks in go.
    pub min_ticks: i64,
    /// Chunks within this many chunks of the world spawn (the overworld)
    /// and of the End's centre stay.
    pub keep_radius: i32,
}

/// What trimming did, or would do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrimReport {
    /// Chunks looked at.
    pub chunks: u64,
    /// Chunks removed (or that would be).
    pub removed: u64,
    /// Chunks kept because they couldn't be read.
    pub unreadable: u64,
    /// Bytes in the region, entity and POI files before.
    pub bytes_before: u64,
    /// And after.
    pub bytes_after: u64,
}

impl ServerFiles {
    /// The dimensions of the world in `level` (and Paper's separate
    /// `<level>_nether` and `<level>_the_end` folders).
    pub fn world_dimensions(&self, level: &str) -> Result<Vec<Dimension>> {
        let mut out = Vec::new();
        for (id, path) in self.dimension_dirs(level)? {
            let mut bytes = 0;
            for sub in ["region", "entities", "poi"] {
                bytes += self
                    .region_files(&format!("{path}/{sub}"))?
                    .iter()
                    .map(|(_, size)| size)
                    .sum::<u64>();
            }
            let mut chunks = 0;
            for (file, _) in self.region_files(&format!("{path}/region"))? {
                let data = self.read_all(&file)?;
                chunks += locations(&data).iter().filter(|l| l.is_some()).count() as u64;
            }
            out.push(Dimension {
                id,
                path,
                bytes,
                chunks,
            });
        }
        Ok(out)
    }

    /// Trims every dimension of the world in `level`: removes chunks with
    /// less than `min_ticks` of players' time, outside the kept radius, with
    /// their entities and points of interest. With `apply` false, only
    /// counts. The server must be stopped.
    pub fn trim_world(
        &self,
        level: &str,
        spawn: (i32, i32),
        options: TrimOptions,
        apply: bool,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<TrimReport> {
        let dims = self.dimension_dirs(level)?;
        let mut files = Vec::new();
        for (id, path) in &dims {
            for (file, _) in self.region_files(&format!("{path}/region"))? {
                files.push((id.clone(), path.clone(), file));
            }
        }
        let mut report = TrimReport::default();
        let total = files.len() as u64;
        for (n, (id, path, file)) in files.iter().enumerate() {
            let centre = match id.as_str() {
                "minecraft:overworld" => Some(spawn),
                "minecraft:the_end" => Some((0, 0)),
                _ => None,
            };
            self.trim_region(path, file, centre, options, apply, &mut report)?;
            progress(n as u64 + 1, total);
        }
        Ok(report)
    }

    fn trim_region(
        &self,
        dim: &str,
        file: &RelPath,
        centre: Option<(i32, i32)>,
        options: TrimOptions,
        apply: bool,
        report: &mut TrimReport,
    ) -> Result<()> {
        let Some((rx, rz)) = file.file_name().and_then(region_coords) else {
            return Ok(());
        };
        let data = self.read_all(file)?;
        let locs = locations(&data);
        let mut remove = [false; 1024];
        for (i, loc) in locs.iter().enumerate() {
            let Some((offset, count)) = *loc else {
                continue;
            };
            report.chunks += 1;
            let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
            if centre.is_some_and(|(sx, sz)| {
                (cx - sx).abs() <= options.keep_radius && (cz - sz).abs() <= options.keep_radius
            }) {
                continue;
            }
            match inhabited_time(&data, offset, count) {
                Some(ticks) if ticks < options.min_ticks => remove[i] = true,
                Some(_) => {}
                None => report.unreadable += 1,
            }
        }
        let removed = remove.iter().filter(|r| **r).count() as u64;
        report.removed += removed;
        let name = file.file_name().unwrap_or_default().to_owned();
        for sub in ["region", "entities", "poi"] {
            let Ok(path) = RelPath::parse(&format!("{dim}/{sub}/{name}")) else {
                continue;
            };
            let data = if sub == "region" {
                data.clone()
            } else {
                match self.read_all(&path) {
                    Ok(d) => d,
                    Err(FileError::NotFound(_)) => continue,
                    Err(e) => return Err(e),
                }
            };
            report.bytes_before += data.len() as u64;
            let rebuilt = if removed == 0 {
                Some(data)
            } else {
                rebuild(&data, &remove)
            };
            report.bytes_after += rebuilt.as_ref().map_or(0, |d| d.len() as u64);
            if apply && removed > 0 {
                match rebuilt {
                    Some(bytes) => {
                        self.write(&path, &bytes, Expect::Any)?;
                    }
                    None => self.remove_file(&path)?,
                }
            }
        }
        Ok(())
    }

    /// Each dimension's ID and folder.
    fn dimension_dirs(&self, level: &str) -> Result<Vec<(String, String)>> {
        let has_regions = |path: &str| {
            RelPath::parse(&format!("{path}/region"))
                .ok()
                .and_then(|p| self.stat(&p).ok())
                .is_some_and(|e| e.kind == EntryKind::Directory)
        };
        let mut out = Vec::new();
        if has_regions(level) {
            out.push(("minecraft:overworld".to_owned(), level.to_owned()));
        }
        for (id, inside, paper) in [
            (
                "minecraft:the_nether",
                "DIM-1",
                format!("{level}_nether/DIM-1"),
            ),
            ("minecraft:the_end", "DIM1", format!("{level}_the_end/DIM1")),
        ] {
            let vanilla = format!("{level}/{inside}");
            if has_regions(&vanilla) {
                out.push((id.to_owned(), vanilla));
            } else if has_regions(&paper) {
                out.push((id.to_owned(), paper));
            }
        }
        // Mods' dimensions: <level>/dimensions/<namespace>/<name>.
        if let Ok(root) = RelPath::parse(&format!("{level}/dimensions"))
            && let Ok(namespaces) = self.list(&root)
        {
            for ns in namespaces.iter().filter(|e| e.kind == EntryKind::Directory) {
                let Ok(nsdir) = RelPath::parse(&format!("{level}/dimensions/{}", ns.name)) else {
                    continue;
                };
                for dim in self
                    .list(&nsdir)?
                    .iter()
                    .filter(|e| e.kind == EntryKind::Directory)
                {
                    let path = format!("{level}/dimensions/{}/{}", ns.name, dim.name);
                    let id = format!("{}:{}", ns.name, dim.name);
                    // The vanilla ones live here on some versions.
                    if has_regions(&path) && !out.iter().any(|(known, _)| *known == id) {
                        out.push((id, path));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The `.mca` files in `dir`, with their sizes.
    fn region_files(&self, dir: &str) -> Result<Vec<(RelPath, u64)>> {
        let Ok(path) = RelPath::parse(dir) else {
            return Ok(Vec::new());
        };
        let entries = match self.list(&path) {
            Ok(e) => e,
            Err(FileError::NotFound(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(entries
            .into_iter()
            .filter(|e| e.kind == EntryKind::File && region_coords(&e.name).is_some())
            .filter_map(|e| Some((RelPath::parse(&format!("{dir}/{}", e.name)).ok()?, e.size)))
            .collect())
    }

    /// Deletes a region file left with no chunks (the backup made before
    /// trimming has it).
    fn remove_file(&self, path: &RelPath) -> Result<()> {
        let (parent, name) = crate::sandbox::split(path)?;
        let dir = self.open_folder(&parent)?;
        dir.remove_file(name)
            .map_err(|e| FileError::at(path.as_str(), e))
    }

    fn read_all(&self, path: &RelPath) -> Result<Vec<u8>> {
        let (mut file, _) = self.open_read(path)?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| FileError::at(path.as_str(), e))?;
        Ok(data)
    }
}

/// `r.<x>.<z>.mca` → (x, z).
fn region_coords(name: &str) -> Option<(i32, i32)> {
    let mut parts = name.strip_prefix("r.")?.strip_suffix(".mca")?.split('.');
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((x, z))
}

/// Each chunk's (sector offset, sector count), or `None` where there's
/// no chunk.
fn locations(data: &[u8]) -> Vec<Option<(usize, usize)>> {
    (0..1024)
        .map(|i| {
            let entry = data.get(i * 4..i * 4 + 4)?;
            let offset = (usize::from(entry[0]) << 16)
                | (usize::from(entry[1]) << 8)
                | usize::from(entry[2]);
            let count = usize::from(entry[3]);
            (offset >= 2
                && count > 0
                && (offset + count) * SECTOR <= data.len().div_ceil(SECTOR) * SECTOR)
                .then_some((offset, count))
        })
        .collect()
}

/// The chunk's `InhabitedTime`, or `None` when it can't be read.
fn inhabited_time(data: &[u8], offset: usize, count: usize) -> Option<i64> {
    let start = offset * SECTOR;
    let end = (start + count * SECTOR).min(data.len());
    let chunk = data.get(start..end)?;
    let length = u32::from_be_bytes(chunk.get(..4)?.try_into().ok()?) as usize;
    let compression = *chunk.get(4)?;
    let payload = chunk.get(5..4 + length)?;
    let mut nbt = Vec::new();
    match compression {
        1 => {
            flate2::read::GzDecoder::new(payload)
                .take(64 * 1024 * 1024)
                .read_to_end(&mut nbt)
                .ok()?;
        }
        2 => {
            flate2::read::ZlibDecoder::new(payload)
                .take(64 * 1024 * 1024)
                .read_to_end(&mut nbt)
                .ok()?;
        }
        3 => nbt.extend_from_slice(payload),
        // LZ4, a custom format, or kept in an external file.
        _ => return None,
    }
    nbt::find_long(&nbt, &[&["InhabitedTime"], &["Level", "InhabitedTime"]])
}

/// The region file with the chunks in `remove` taken out, packed again;
/// `None` when no chunk is left.
fn rebuild(data: &[u8], remove: &[bool; 1024]) -> Option<Vec<u8>> {
    let locs = locations(data);
    let mut out = vec![0u8; HEADER];
    let mut kept = 0;
    for (i, loc) in locs.iter().enumerate() {
        let Some((offset, count)) = *loc else {
            continue;
        };
        if remove[i] {
            continue;
        }
        let sector = out.len() / SECTOR;
        let start = offset * SECTOR;
        let end = (start + count * SECTOR).min(data.len());
        out.extend_from_slice(&data[start..end]);
        out.resize((sector + count) * SECTOR, 0);
        out[i * 4] = (sector >> 16) as u8;
        out[i * 4 + 1] = (sector >> 8) as u8;
        out[i * 4 + 2] = sector as u8;
        out[i * 4 + 3] = count as u8;
        // The timestamp, unchanged.
        out[SECTOR + i * 4..SECTOR + i * 4 + 4]
            .copy_from_slice(&data[SECTOR + i * 4..SECTOR + i * 4 + 4]);
        kept += 1;
    }
    (kept > 0).then_some(out)
}

/// Just enough NBT to find a few numbers.
pub(crate) mod nbt {
    /// The first `Long` (or `Int`) at any of `paths` (compound names from
    /// the root), in uncompressed NBT.
    pub(crate) fn find_long(data: &[u8], paths: &[&[&str]]) -> Option<i64> {
        let mut r = Reader { data, pos: 0 };
        // The root: a named compound.
        if r.u8()? != 10 {
            return None;
        }
        r.string()?;
        let mut found: Vec<(Vec<String>, i64)> = Vec::new();
        r.compound(&mut Vec::new(), &mut found, 0)?;
        paths.iter().find_map(|path| {
            found
                .iter()
                .find(|(p, _)| p.iter().map(String::as_str).eq(path.iter().copied()))
                .map(|(_, v)| *v)
        })
    }

    struct Reader<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl Reader<'_> {
        fn take(&mut self, n: usize) -> Option<&[u8]> {
            let out = self.data.get(self.pos..self.pos.checked_add(n)?)?;
            self.pos += n;
            Some(out)
        }
        fn u8(&mut self) -> Option<u8> {
            self.take(1).map(|b| b[0])
        }
        fn i32(&mut self) -> Option<i32> {
            Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
        }
        fn i64(&mut self) -> Option<i64> {
            Some(i64::from_be_bytes(self.take(8)?.try_into().ok()?))
        }
        fn string(&mut self) -> Option<String> {
            let len = u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize;
            Some(String::from_utf8_lossy(self.take(len)?).into_owned())
        }
        fn count(&mut self) -> Option<usize> {
            usize::try_from(self.i32()?).ok()
        }

        /// Reads a compound's entries, noting numbers (by path) as it goes.
        fn compound(
            &mut self,
            path: &mut Vec<String>,
            found: &mut Vec<(Vec<String>, i64)>,
            depth: usize,
        ) -> Option<()> {
            if depth > 64 {
                return None;
            }
            loop {
                let tag = self.u8()?;
                if tag == 0 {
                    return Some(());
                }
                let name = self.string()?;
                path.push(name);
                match tag {
                    3 => {
                        let v = self.i32()?;
                        found.push((path.clone(), i64::from(v)));
                    }
                    4 => {
                        let v = self.i64()?;
                        found.push((path.clone(), v));
                    }
                    10 => self.compound(path, found, depth + 1)?,
                    other => self.skip(other, depth)?,
                }
                path.pop();
            }
        }

        /// Skips a value of `tag` (numbers inside lists aren't noted).
        fn skip(&mut self, tag: u8, depth: usize) -> Option<()> {
            if depth > 64 {
                return None;
            }
            match tag {
                1 => self.take(1).map(drop),
                2 => self.take(2).map(drop),
                3 | 5 => self.take(4).map(drop),
                4 | 6 => self.take(8).map(drop),
                7 => {
                    let n = self.count()?;
                    self.take(n).map(drop)
                }
                8 => self.string().map(drop),
                9 => {
                    let inner = self.u8()?;
                    let n = self.count()?;
                    for _ in 0..n {
                        self.skip(inner, depth + 1)?;
                    }
                    Some(())
                }
                10 => self.compound(&mut Vec::new(), &mut Vec::new(), depth + 1),
                11 => {
                    let n = self.count()?;
                    self.take(n.checked_mul(4)?).map(drop)
                }
                12 => {
                    let n = self.count()?;
                    self.take(n.checked_mul(8)?).map(drop)
                }
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// NBT for a chunk: a root compound with InhabitedTime (and, before
    /// 1.18, under Level), plus a list and a string to skip over.
    fn chunk_nbt(ticks: i64, legacy: bool) -> Vec<u8> {
        let mut v = vec![10, 0, 0];
        let mut body = Vec::new();
        // A list of ints and a string first, to be skipped.
        body.extend([9, 0, 4]);
        body.extend(b"list");
        body.extend([3, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]);
        body.extend([8, 0, 6]);
        body.extend(b"Status");
        body.extend([0, 4]);
        body.extend(b"full");
        let mut time = vec![4, 0, 13];
        time.extend(b"InhabitedTime");
        time.extend(ticks.to_be_bytes());
        if legacy {
            body.extend([10, 0, 5]);
            body.extend(b"Level");
            body.extend(time);
            body.push(0);
        } else {
            body.extend(time);
        }
        v.extend(body);
        v.push(0);
        v
    }

    /// A region file holding `chunks` (index, InhabitedTime) as zlib.
    fn region(chunks: &[(usize, i64)]) -> Vec<u8> {
        let mut out = vec![0u8; HEADER];
        for (i, ticks) in chunks {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            z.write_all(&chunk_nbt(*ticks, *i % 2 == 1)).unwrap();
            let payload = z.finish().unwrap();
            let sector = out.len() / SECTOR;
            out.extend(((payload.len() + 1) as u32).to_be_bytes());
            out.push(2);
            out.extend(&payload);
            let count = (out.len() - sector * SECTOR).div_ceil(SECTOR);
            out.resize((sector + count) * SECTOR, 0);
            out[i * 4..i * 4 + 4].copy_from_slice(&[
                (sector >> 16) as u8,
                (sector >> 8) as u8,
                sector as u8,
                count as u8,
            ]);
            out[SECTOR + i * 4..SECTOR + i * 4 + 4].copy_from_slice(&(*i as u32).to_be_bytes());
        }
        out
    }

    #[test]
    fn inhabited_time_is_found_in_both_layouts() {
        assert_eq!(
            nbt::find_long(
                &chunk_nbt(1234, false),
                &[&["InhabitedTime"], &["Level", "InhabitedTime"]]
            ),
            Some(1234)
        );
        assert_eq!(
            nbt::find_long(
                &chunk_nbt(99, true),
                &[&["InhabitedTime"], &["Level", "InhabitedTime"]]
            ),
            Some(99)
        );
        assert_eq!(nbt::find_long(b"\x0a\x00", &[&["InhabitedTime"]]), None);
    }

    #[test]
    fn region_files_round_trip_and_trim() {
        let data = region(&[(0, 5), (1, 50_000), (33, 0), (1023, 100)]);
        let locs = locations(&data);
        assert_eq!(locs.iter().filter(|l| l.is_some()).count(), 4);
        let (offset, count) = locs[1].unwrap();
        assert_eq!(inhabited_time(&data, offset, count), Some(50_000));

        let mut remove = [false; 1024];
        remove[0] = true;
        remove[33] = true;
        let rebuilt = rebuild(&data, &remove).unwrap();
        let locs = locations(&rebuilt);
        assert!(locs[0].is_none() && locs[33].is_none());
        let (offset, count) = locs[1023].unwrap();
        assert_eq!(inhabited_time(&rebuilt, offset, count), Some(100));
        assert_eq!(
            &rebuilt[SECTOR + 1023 * 4..SECTOR + 1024 * 4],
            &1023u32.to_be_bytes()
        );
        assert!(rebuilt.len() < data.len());

        let all = [true; 1024];
        assert!(rebuild(&data, &all).is_none());
        assert_eq!(region_coords("r.-1.2.mca"), Some((-1, 2)));
        assert_eq!(region_coords("r.0.0.mcr"), None);
    }

    #[test]
    fn a_world_is_trimmed_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for dir in ["world/region", "world/entities", "world/DIM1/region"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        // Region 0,0: chunk 0 (spawn) unvisited, chunk 40 unvisited, chunk
        // 41 well visited. Region 5,5 only unvisited chunks.
        std::fs::write(
            root.join("world/region/r.0.0.mca"),
            region(&[(0, 0), (40, 10), (41, 99_999)]),
        )
        .unwrap();
        std::fs::write(
            root.join("world/entities/r.0.0.mca"),
            region(&[(40, 0), (41, 0)]),
        )
        .unwrap();
        std::fs::write(root.join("world/region/r.5.5.mca"), region(&[(3, 0)])).unwrap();
        // The End's centre stays however little time was spent there.
        std::fs::write(
            root.join("world/DIM1/region/r.0.0.mca"),
            region(&[(0, 0), (500, 0)]),
        )
        .unwrap();
        let files = ServerFiles::open(root).unwrap();

        let dims = files.world_dimensions("world").unwrap();
        let ids: Vec<&str> = dims.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["minecraft:overworld", "minecraft:the_end"]);
        assert_eq!(dims[0].chunks, 4);

        let options = TrimOptions {
            min_ticks: 1200,
            keep_radius: 2,
        };
        let preview = files
            .trim_world("world", (0, 0), options, false, &mut |_, _| {})
            .unwrap();
        // Chunk 40 (8, 1), r.5.5's chunk, and the End's chunk 500 (20, 15).
        assert_eq!(
            (preview.chunks, preview.removed, preview.unreadable),
            (6, 3, 0)
        );
        assert!(
            root.join("world/region/r.5.5.mca").exists(),
            "a preview changes nothing"
        );

        let done = files
            .trim_world("world", (0, 0), options, true, &mut |_, _| {})
            .unwrap();
        assert_eq!(done, preview);
        assert!(!root.join("world/region/r.5.5.mca").exists());
        let kept = std::fs::read(root.join("world/region/r.0.0.mca")).unwrap();
        let locs = locations(&kept);
        assert!(locs[0].is_some() && locs[40].is_none() && locs[41].is_some());
        let entities = std::fs::read(root.join("world/entities/r.0.0.mca")).unwrap();
        let locs = locations(&entities);
        assert!(
            locs[40].is_none() && locs[41].is_some(),
            "entities follow their chunks"
        );
    }
}
