//! Anvil region reading, export to a new region, and in-place chunk writes
//! as vanilla's `RegionFile` makes them (a file opened with `open_writable`).
//! Exports never overwrite a file: use a separate directory for offline
//! edits and copied .mcc data.
use crate::{
    compression::{Compression, invalid},
    nbt::{self, Limits, NamedTag},
};
use horizon_core::position::ChunkPos;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const SECTOR: usize = 4096;
const HEADER: usize = SECTOR * 2;
const MAX_COMPRESSED: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ChunkRecord {
    pub compression: Compression,
    pub compressed: Vec<u8>,
    pub timestamp: u32,
}
impl ChunkRecord {
    pub fn from_nbt(
        root: &NamedTag,
        compression: Compression,
        timestamp: u32,
        limits: Limits,
    ) -> io::Result<Self> {
        let bytes = nbt::write_named(root, limits).map_err(|e| invalid(e.to_string()))?;
        Ok(Self {
            compression,
            compressed: compression.encode(&bytes)?,
            timestamp,
        })
    }
    pub fn nbt(&self, limits: Limits) -> io::Result<NamedTag> {
        let bytes = self
            .compression
            .decode(&self.compressed, limits.max_bytes)?;
        let (root, used) = nbt::read_named(&bytes, limits).map_err(|e| invalid(e.to_string()))?;
        if used != bytes.len() {
            return Err(invalid("trailing bytes after region chunk NBT"));
        }
        Ok(root)
    }
}

pub struct RegionFile {
    file: File,
    path: PathBuf,
    position: ChunkPos,
    offsets: [u32; 1024],
    timestamps: [u32; 1024],
    /// The sectors in use (`RegionBitmap`): the header's two and every
    /// chunk's.
    used: Vec<bool>,
    /// The lowest sector that may be free: allocations start here instead of
    /// scanning from zero, and freeing a lower run rewinds it, so the chosen
    /// sectors (first fit) are the same as a full scan.
    next_free: usize,
}

pub fn position_from_path(path: &Path) -> io::Result<ChunkPos> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("invalid region filename"))?;
    let parts: Vec<_> = name.split('.').collect();
    if parts.len() != 4 || parts[0] != "r" || parts[3] != "mca" {
        return Err(invalid("expected r.<x>.<z>.mca"));
    }
    let x: i32 = parts[1]
        .parse()
        .map_err(|_| invalid("invalid region x coordinate"))?;
    let z: i32 = parts[2]
        .parse()
        .map_err(|_| invalid("invalid region z coordinate"))?;
    // Regions outside this range cannot be addressed with signed chunk coordinates.
    if !(-67_108_864..=67_108_863).contains(&x) || !(-67_108_864..=67_108_863).contains(&z) {
        return Err(invalid("region coordinate outside chunk coordinate range"));
    }
    Ok(ChunkPos::new(x, z))
}

fn slot(region: ChunkPos, chunk: ChunkPos) -> io::Result<usize> {
    if chunk.region() != region {
        return Err(invalid("chunk does not belong to this region"));
    }
    let (x, z) = chunk.region_local();
    Ok(usize::from(x) + usize::from(z) * 32)
}
fn position(region: ChunkPos, index: usize) -> ChunkPos {
    ChunkPos::new(
        region.x * 32 + (index % 32) as i32,
        region.z * 32 + (index / 32) as i32,
    )
}
fn external_path(directory: &Path, chunk: ChunkPos) -> PathBuf {
    directory.join(format!("c.{}.{}.mcc", chunk.x, chunk.z))
}
fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
/// Positioned I/O: a read needs no file cursor, so concurrent readers of one
/// region do not serialize.
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}
#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}
#[cfg(not(any(unix, windows)))]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(buf)
}
fn force_in(used: &mut Vec<bool>, start: usize, count: usize) {
    if used.len() < start + count {
        used.resize(start + count, false);
    }
    used[start..start + count].fill(true);
}
fn free_in(used: &mut [bool], next_free: &mut usize, start: usize, count: usize) {
    let end = (start + count).min(used.len());
    if start < end {
        used[start..end].fill(false);
    }
    if start < *next_free {
        *next_free = start;
    }
}
/// The first run of free sectors long enough, starting from `next_free`.
fn allocate_in(used: &mut Vec<bool>, next_free: &mut usize, count: usize) -> usize {
    let mut current = *next_free;
    loop {
        let start = (current..used.len())
            .find(|&s| !used[s])
            .unwrap_or(used.len());
        let end = (start..used.len()).find(|&s| used[s]);
        if end.is_none_or(|end| end - start >= count) {
            force_in(used, start, count);
            *next_free = start + count;
            return start;
        }
        current = end.unwrap_or(start);
    }
}

impl RegionFile {
    pub fn position(&self) -> ChunkPos {
        self.position
    }
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        let file = File::open(&path)?;
        Self::from_file(path, file)
    }

    /// Open for reading and in-place writes (`write`, `clear`), creating an
    /// empty region with `create`.
    pub fn open_writable(path: impl AsRef<Path>, create: bool) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(&path)?;
        Self::from_file(path, file)
    }

    fn from_file(path: PathBuf, mut file: File) -> io::Result<Self> {
        let position = position_from_path(&path)?;
        let len = file.metadata()?.len();
        // RegionFile's constructor: a short header (an empty or truncated file)
        // reads as far as it goes, the rest stays zero (no chunks).
        let mut header = [0; HEADER];
        let mut read = 0;
        while read < HEADER {
            match file.read(&mut header[read..])? {
                0 => break,
                n => read += n,
            }
        }
        let mut offsets = [0; 1024];
        let mut timestamps = [0; 1024];
        for i in 0..1024 {
            offsets[i] = u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap());
            timestamps[i] = u32::from_be_bytes(
                header[SECTOR + i * 4..SECTOR + i * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
            if offsets[i] == 0 {
                continue;
            }
            let start = offsets[i] >> 8;
            let count = offsets[i] & 255;
            // Vanilla drops entries that overlap the header, are empty or start past
            // the end of the file (logged as invalid sectors).
            if start < 2 || count == 0 || u64::from(start) * SECTOR as u64 > len {
                offsets[i] = 0;
            }
        }
        let mut region = Self {
            file,
            path,
            position,
            offsets,
            timestamps,
            used: Vec::new(),
            next_free: 0,
        };
        region.force(0, 2);
        for offset in offsets.into_iter().filter(|&o| o != 0) {
            region.force((offset >> 8) as usize, (offset & 255) as usize);
        }
        Ok(region)
    }

    /// `RegionBitmap.force`.
    fn force(&mut self, start: usize, count: usize) {
        force_in(&mut self.used, start, count);
    }
    /// `RegionBitmap.free`.
    fn free(&mut self, start: usize, count: usize) {
        free_in(&mut self.used, &mut self.next_free, start, count);
    }

    /// `RegionFile.write`: the chunk goes to the first free run of sectors
    /// that holds it (an oversized one, 256 sectors or more, to its external
    /// `c.X.Z.mcc` file with a one-sector stub), the header points to it,
    /// and only then are its old sectors freed. The other chunks stay where
    /// they are. A chunk's last sector is padded with zeros.
    pub fn write(&mut self, chunk: ChunkPos, record: &ChunkRecord) -> io::Result<()> {
        self.write_batch(&[(chunk, Some(record))], record.timestamp)
            .map(|_| ())
    }

    /// Write and clear several chunks of this region with one header write.
    /// Planning uses copies of the tables and bitmap, so a validation error
    /// leaves the file and the in-memory tables as they were; data is written
    /// before the header points at it, and old sectors are freed only after
    /// the header is durable. Returns whether any slot changed.
    pub fn write_batch(
        &mut self,
        records: &[(ChunkPos, Option<&ChunkRecord>)],
        clear_timestamp: u32,
    ) -> io::Result<bool> {
        enum BatchOp<'a> {
            Write {
                chunk: ChunkPos,
                start: usize,
                sectors: usize,
                data: Vec<u8>,
                external: Option<&'a [u8]>,
                old: u32,
            },
            Clear {
                chunk: ChunkPos,
                old: u32,
            },
        }
        let mut used = self.used.clone();
        let mut next_free = self.next_free;
        let mut offsets = self.offsets;
        let mut timestamps = self.timestamps;
        let mut plans: BTreeMap<usize, BatchOp<'_>> = BTreeMap::new();
        for &(chunk, record) in records {
            let i = slot(self.position, chunk)?;
            // A repeated slot writes once, as sequential writes would end up.
            if let Some(BatchOp::Write { start, sectors, .. }) = plans.remove(&i) {
                free_in(&mut used, &mut next_free, start, sectors);
            }
            let old = self.offsets[i];
            match record {
                Some(record) => {
                    if record.compressed.len() > MAX_COMPRESSED {
                        return Err(invalid("compressed chunk limit exceeded"));
                    }
                    let required = (record.compressed.len() + 5).div_ceil(SECTOR);
                    let external = required >= 256;
                    let sectors = if external { 1 } else { required };
                    let start = allocate_in(&mut used, &mut next_free, sectors);
                    if start + sectors > 0x100_0000 {
                        return Err(invalid("region sector address limit exceeded"));
                    }
                    let mut data = Vec::with_capacity(sectors * SECTOR);
                    if external {
                        data.extend_from_slice(&1u32.to_be_bytes());
                        data.push(record.compression as u8 | 128);
                    } else {
                        data.extend_from_slice(&(record.compressed.len() as u32 + 1).to_be_bytes());
                        data.push(record.compression as u8);
                        data.extend_from_slice(&record.compressed);
                    }
                    data.resize(sectors * SECTOR, 0);
                    offsets[i] = (start as u32) << 8 | sectors as u32;
                    timestamps[i] = record.timestamp;
                    plans.insert(
                        i,
                        BatchOp::Write {
                            chunk,
                            start,
                            sectors,
                            data,
                            external: external.then_some(record.compressed.as_slice()),
                            old,
                        },
                    );
                }
                None => {
                    // The final clear wins even if an earlier record in the
                    // batch planned a write to this slot: reset its offset.
                    offsets[i] = 0;
                    timestamps[i] = clear_timestamp;
                    if old != 0 {
                        plans.insert(i, BatchOp::Clear { chunk, old });
                    }
                }
            }
        }
        if plans.is_empty() {
            return Ok(false);
        }
        let directory = self.path.parent().unwrap_or(Path::new(".")).to_owned();
        let external_path_of = |chunk: ChunkPos| external_path(&directory, chunk);
        // External files first: a failure leaves the header on the old data.
        for op in plans.values() {
            let BatchOp::Write {
                chunk,
                external: Some(bytes),
                ..
            } = op
            else {
                continue;
            };
            let temporary = directory.join(format!(".c.{}.{}.mcc.tmp", chunk.x, chunk.z));
            let written = File::create(&temporary)
                .and_then(|mut f| f.write_all(bytes))
                .and_then(|_| fs::rename(&temporary, external_path_of(*chunk)));
            if let Err(e) = written {
                let _ = fs::remove_file(&temporary);
                return Err(e);
            }
        }
        // Adjacent runs go out as one buffer.
        let mut runs: Vec<(usize, Vec<u8>)> = Vec::new();
        let mut unlink = Vec::new();
        let mut old_runs = Vec::new();
        for (_, op) in std::mem::take(&mut plans) {
            match op {
                BatchOp::Write {
                    chunk,
                    start,
                    data,
                    external,
                    old,
                    ..
                } => {
                    if old != 0 {
                        old_runs.push((old >> 8, old & 255));
                    }
                    // A header record does not say whether it was external, so
                    // any previous record may still have a `.mcc`. A slot
                    // without one (a fresh region) skips the unlink entirely.
                    if old != 0 && external.is_none() {
                        unlink.push(external_path_of(chunk));
                    }
                    match runs.last_mut() {
                        Some((last, buffer)) if *last * SECTOR + buffer.len() == start * SECTOR => {
                            buffer.extend_from_slice(&data);
                        }
                        _ => runs.push((start, data)),
                    }
                }
                BatchOp::Clear { chunk, old } => {
                    old_runs.push((old >> 8, old & 255));
                    unlink.push(external_path_of(chunk));
                }
            }
        }
        for (start, data) in runs {
            write_all_at(&self.file, &data, (start * SECTOR) as u64)?;
        }
        let mut header = [0; HEADER];
        for i in 0..1024 {
            header[i * 4..i * 4 + 4].copy_from_slice(&offsets[i].to_be_bytes());
            header[SECTOR + i * 4..SECTOR + i * 4 + 4]
                .copy_from_slice(&timestamps[i].to_be_bytes());
        }
        write_all_at(&self.file, &header, 0)?;
        self.offsets = offsets;
        self.timestamps = timestamps;
        self.used = used;
        self.next_free = next_free;
        for path in unlink {
            remove_if_exists(&path)?;
        }
        for (start, count) in old_runs {
            self.free(start as usize, count as usize);
        }
        Ok(true)
    }

    /// `RegionFile.clear`: the chunk is gone; `timestamp` marks when.
    /// Returns whether the region held it.
    pub fn clear(&mut self, chunk: ChunkPos, timestamp: u32) -> io::Result<bool> {
        let i = slot(self.position, chunk)?;
        let old = self.offsets[i];
        if old == 0 {
            return Ok(false);
        }
        self.offsets[i] = 0;
        self.timestamps[i] = timestamp;
        self.write_header_entry(i)?;
        let directory = self.path.parent().unwrap_or(Path::new("."));
        remove_if_exists(&external_path(directory, chunk))?;
        self.free((old >> 8) as usize, (old & 255) as usize);
        Ok(true)
    }

    /// Flush the writes to the disk.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// The header's offset and timestamp of slot `i`.
    fn write_header_entry(&mut self, i: usize) -> io::Result<()> {
        write_all_at(&self.file, &self.offsets[i].to_be_bytes(), (i * 4) as u64)?;
        write_all_at(
            &self.file,
            &self.timestamps[i].to_be_bytes(),
            (SECTOR + i * 4) as u64,
        )
    }

    pub fn chunks(&self) -> impl Iterator<Item = (ChunkPos, u32)> + '_ {
        self.offsets
            .iter()
            .enumerate()
            .filter(|(_, value)| **value != 0)
            .map(|(i, _)| (position(self.position, i), self.timestamps[i]))
    }

    pub fn read(&self, chunk: ChunkPos) -> io::Result<Option<ChunkRecord>> {
        let i = slot(self.position, chunk)?;
        let offset = self.offsets[i];
        if offset == 0 {
            return Ok(None);
        }
        let start = u64::from(offset >> 8) * SECTOR as u64;
        let mut header = [0; 5];
        read_exact_at(&self.file, &mut header, start)?;
        let len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        if len < 1 || len as u64 + 4 > (u64::from(offset & 255) * SECTOR as u64) {
            return Err(invalid("invalid chunk payload length"));
        }
        let compression = Compression::try_from(header[4] & 127)?;
        let compressed = if header[4] & 128 != 0 {
            if len != 1 {
                return Err(invalid("external chunk stub has inline data"));
            }
            let path = external_path(self.path.parent().unwrap_or(Path::new(".")), chunk);
            let file = File::open(path)?;
            let mut data = Vec::new();
            file.take((MAX_COMPRESSED + 1) as u64)
                .read_to_end(&mut data)?;
            if data.len() > MAX_COMPRESSED {
                return Err(invalid("external chunk compressed limit exceeded"));
            }
            data
        } else {
            let mut data = vec![0; len - 1];
            read_exact_at(&self.file, &mut data, start + 5)?;
            data
        };
        Ok(Some(ChunkRecord {
            compression,
            compressed,
            timestamp: self.timestamps[i],
        }))
    }

    /// Export all chunks, preserving untouched compressed bytes and timestamps.
    /// Overrides are keyed by chunk position; None deletes a chunk in the export.
    pub fn export(
        &mut self,
        destination: impl AsRef<Path>,
        changes: &[(ChunkPos, Option<ChunkRecord>)],
    ) -> io::Result<()> {
        let destination = destination.as_ref();
        if position_from_path(destination)? != self.position {
            return Err(invalid("export region coordinates differ"));
        }
        let mut replacements = BTreeChanges::new();
        for (chunk, record) in changes {
            let i = slot(self.position, *chunk)?;
            if replacements.insert(i, record.as_ref()).is_some() {
                return Err(invalid("duplicate chunk override"));
            }
        }
        let mut writer = RegionWriter::create(destination)?;
        for i in 0..1024 {
            let chunk = position(self.position, i);
            if let Some(record) = replacements.get(&i) {
                if let Some(record) = record {
                    writer.write(chunk, record)?;
                }
            } else if let Some(record) = self.read(chunk)? {
                writer.write(chunk, &record)?;
            }
        }
        writer.finish()
    }
}
type BTreeChanges<'a> = std::collections::BTreeMap<usize, Option<&'a ChunkRecord>>;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Writes a complete new region to a temporary sibling, then publishes it with
/// an atomic, non-overwriting hard link. Drop rolls back unpublished files.
pub struct RegionWriter {
    file: File,
    temporary: PathBuf,
    destination: PathBuf,
    position: ChunkPos,
    offsets: [u32; 1024],
    timestamps: [u32; 1024],
    next_sector: u32,
    externals: Vec<PathBuf>,
    committed: bool,
    failed: bool,
}
impl RegionWriter {
    pub fn create(destination: impl AsRef<Path>) -> io::Result<Self> {
        let destination = destination.as_ref().to_owned();
        let position = position_from_path(&destination)?;
        if destination.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination region already exists",
            ));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let (temporary, file) = loop {
            let path = parent.join(format!(
                ".horizon-{}-{}.tmp",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => break (path, file),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let mut writer = Self {
            file,
            temporary,
            destination,
            position,
            offsets: [0; 1024],
            timestamps: [0; 1024],
            next_sector: 2,
            externals: Vec::new(),
            committed: false,
            failed: false,
        };
        writer.file.write_all(&[0; HEADER])?;
        Ok(writer)
    }

    pub fn write(&mut self, chunk: ChunkPos, record: &ChunkRecord) -> io::Result<()> {
        if self.failed {
            return Err(invalid("region writer failed previously"));
        }
        let result = self.write_record(chunk, record);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn write_record(&mut self, chunk: ChunkPos, record: &ChunkRecord) -> io::Result<()> {
        let i = slot(self.position, chunk)?;
        if self.offsets[i] != 0 {
            return Err(invalid("chunk already written"));
        }
        if record.compressed.len() > MAX_COMPRESSED {
            return Err(invalid("compressed chunk limit exceeded"));
        }
        let required = (record.compressed.len() + 5).div_ceil(SECTOR);
        let external = required >= 256;
        let sectors = if external { 1 } else { required as u32 };
        if self.next_sector + sectors > 0x100_0000 {
            return Err(invalid("region sector address limit exceeded"));
        }
        if external {
            let path = external_path(self.destination.parent().unwrap_or(Path::new(".")), chunk);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            self.externals.push(path);
            file.write_all(&record.compressed)?;
            file.sync_all()?;
        }
        self.file
            .seek(SeekFrom::Start(u64::from(self.next_sector) * SECTOR as u64))?;
        let len = if external {
            1
        } else {
            record.compressed.len() as u32 + 1
        };
        self.file.write_all(&len.to_be_bytes())?;
        self.file
            .write_all(&[record.compression as u8 | if external { 128 } else { 0 }])?;
        if !external {
            self.file.write_all(&record.compressed)?;
        }
        self.offsets[i] = self.next_sector << 8 | sectors;
        self.timestamps[i] = record.timestamp;
        self.next_sector += sectors;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        if self.failed {
            return Err(invalid("cannot publish a failed region writer"));
        }
        self.file
            .set_len(u64::from(self.next_sector) * SECTOR as u64)?;
        self.file.seek(SeekFrom::Start(0))?;
        for value in self.offsets {
            self.file.write_all(&value.to_be_bytes())?;
        }
        for value in self.timestamps {
            self.file.write_all(&value.to_be_bytes())?;
        }
        self.file.sync_all()?;
        // Hard-link publication fails atomically if another writer won the name.
        fs::hard_link(&self.temporary, &self.destination)?;
        self.committed = true;
        Ok(())
    }
}
impl Drop for RegionWriter {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.temporary);
        if !self.committed {
            for path in &self.externals {
                let _ = fs::remove_file(path);
            }
        }
    }
}
