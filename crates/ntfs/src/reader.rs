//! Chunked sequential reading of the whole MFT: sector-aligned reads of at most `MAX_READ_BYTES`, one parsed
//! record at a time, with a hard cap on the number of records.

use crate::error::{ParseError, ParseLimits};
use crate::record::{FIXUP_STRIDE, MftRecord, parse_raw_record};
use crate::runs::Extent;
use crate::volume::{MAX_READ_BYTES, NtfsVolumeData, Volume, VolumeError, is_pow2_in};

/// A random-access, read-only byte source. `Volume` implements it; tests use `MemSource`.
pub trait BlockSource {
    /// Reads exactly `len` bytes at the byte `offset` (both sector-aligned by the caller).
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError>;
}

impl BlockSource for Volume {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        Volume::read_at(self, offset, len)
    }
}

impl<T: BlockSource + ?Sized> BlockSource for &T {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        (**self).read_at(offset, len)
    }
}

/// An in-memory disk image, for tests and tools.
#[derive(Clone, Debug, Default)]
pub struct MemSource(pub Vec<u8>);

impl BlockSource for MemSource {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        let start = usize::try_from(offset).map_err(|_| VolumeError::InvalidRange)?;
        let end = start.checked_add(len).ok_or(VolumeError::InvalidRange)?;
        if len > MAX_READ_BYTES {
            return Err(VolumeError::InvalidRange);
        }
        self.0.get(start..end).map(<[u8]>::to_vec).ok_or(VolumeError::ShortRead)
    }
}

/// Sizes needed to map clusters and records to bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MftGeometry {
    pub bytes_per_sector: u32,
    pub bytes_per_cluster: u32,
    /// Size of one file record (usually 1024).
    pub bytes_per_record: u32,
}

impl MftGeometry {
    /// Validates the sizes: powers of two, cluster >= sector, record a multiple of the fixup stride.
    pub fn new(bytes_per_sector: u32, bytes_per_cluster: u32, bytes_per_record: u32) -> Result<Self, ParseError> {
        let stride = u32::try_from(FIXUP_STRIDE).map_err(|_| ParseError::BadGeometry)?;
        let ok = is_pow2_in(bytes_per_sector, 512, 65_536)
            && is_pow2_in(bytes_per_cluster, bytes_per_sector, 2 * 1024 * 1024)
            && is_pow2_in(bytes_per_record, stride, 65_536);
        if ok {
            Ok(Self { bytes_per_sector, bytes_per_cluster, bytes_per_record })
        } else {
            Err(ParseError::BadGeometry)
        }
    }

    /// Geometry from an opened volume's validated data.
    pub fn from_volume(data: &NtfsVolumeData) -> Result<Self, ParseError> {
        Self::new(data.bytes_per_sector, data.bytes_per_cluster, data.bytes_per_file_record)
    }
}

/// Caps and tuning for `MftReader`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderLimits {
    /// Preferred size of one read (rounded to whole clusters, capped at `MAX_READ_BYTES`).
    pub chunk_bytes: usize,
    /// Records yielded at most. 2^32 records is the NTFS maximum.
    pub max_records: u64,
    pub parse: ParseLimits,
}

impl Default for ReaderLimits {
    fn default() -> Self {
        Self { chunk_bytes: 4 * 1024 * 1024, max_records: 1 << 32, parse: ParseLimits::default() }
    }
}

/// A record slot of the MFT: its number and the parse result. Zero-filled slots are skipped, not yielded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MftItem {
    /// Record number = position in the MFT.
    pub number: u64,
    pub record: Result<MftRecord, ParseError>,
}

/// Iterator over the records of the MFT. An I/O error is yielded once and ends the iteration.
#[derive(Debug)]
pub struct MftReader<S> {
    source: S,
    geometry: MftGeometry,
    extents: Vec<Extent>,
    limits: ReaderLimits,
    max_records: u64,
    ext_index: usize,
    ext_offset: u64,
    chunk: usize,
    buf: Vec<u8>,
    pos: usize,
    next_number: u64,
    skipped_empty: u64,
    done: bool,
    stopped: bool,
}

impl<S: BlockSource> MftReader<S> {
    /// `mft_size` is the number of valid MFT bytes (the `$MFT` data size or the volume's valid data length).
    pub fn new(
        source: S,
        geometry: MftGeometry,
        extents: Vec<Extent>,
        mft_size: u64,
        limits: ReaderLimits,
    ) -> Result<Self, ParseError> {
        for e in &extents {
            let offset = e.byte_offset(geometry.bytes_per_cluster).ok_or(ParseError::RunOverflow)?;
            let len = e.byte_len(geometry.bytes_per_cluster).ok_or(ParseError::RunOverflow)?;
            offset.checked_add(len).ok_or(ParseError::RunOverflow)?;
        }
        let cluster = usize::try_from(geometry.bytes_per_cluster).map_err(|_| ParseError::BadGeometry)?;
        let whole = limits.chunk_bytes.min(MAX_READ_BYTES) / cluster * cluster;
        let chunk = whole.max(cluster);
        let by_size = mft_size / u64::from(geometry.bytes_per_record);
        Ok(Self {
            source,
            geometry,
            extents,
            max_records: limits.max_records.min(by_size),
            limits,
            ext_index: 0,
            ext_offset: 0,
            chunk,
            buf: Vec::new(),
            pos: 0,
            next_number: 0,
            skipped_empty: 0,
            done: false,
            stopped: false,
        })
    }

    /// Cross-checks the extents against the size of the volume they claim to live on: every extent must end
    /// inside `total_bytes`, the extents together may not be larger than the volume (forged or overlapping
    /// run lists), and the number of records is capped by what fits in the volume.
    pub fn bounded_by(mut self, total_bytes: u64) -> Result<Self, ParseError> {
        let bpc = self.geometry.bytes_per_cluster;
        let mut sum = 0u64;
        for e in &self.extents {
            let offset = e.byte_offset(bpc).ok_or(ParseError::RunOverflow)?;
            let len = e.byte_len(bpc).ok_or(ParseError::RunOverflow)?;
            let end = offset.checked_add(len).ok_or(ParseError::RunOverflow)?;
            sum = sum.checked_add(len).ok_or(ParseError::RunOverflow)?;
            if end > total_bytes || sum > total_bytes {
                return Err(ParseError::ExtentOutOfRange);
            }
        }
        self.max_records = self.max_records.min(total_bytes / u64::from(self.geometry.bytes_per_record));
        Ok(self)
    }

    /// Zero-filled (never used) record slots skipped so far.
    pub fn skipped_empty(&self) -> u64 {
        self.skipped_empty
    }

    /// Reads the next chunk into the buffer. `Ok(false)` when the extents are exhausted.
    fn refill(&mut self) -> Result<bool, VolumeError> {
        self.buf.drain(..self.pos.min(self.buf.len()));
        self.pos = 0;
        let bpc = self.geometry.bytes_per_cluster;
        while let Some(extent) = self.extents.get(self.ext_index) {
            let total = extent.byte_len(bpc).ok_or(VolumeError::InvalidRange)?;
            let remaining = total.saturating_sub(self.ext_offset);
            if remaining == 0 {
                self.ext_index += 1;
                self.ext_offset = 0;
                continue;
            }
            let len = usize::try_from(remaining).map_or(self.chunk, |r| r.min(self.chunk));
            let start = extent
                .byte_offset(bpc)
                .and_then(|o| o.checked_add(self.ext_offset))
                .ok_or(VolumeError::InvalidRange)?;
            let data = self.source.read_at(start, len)?;
            if data.len() != len {
                return Err(VolumeError::ShortRead);
            }
            self.ext_offset =
                self.ext_offset.saturating_add(u64::try_from(len).map_err(|_| VolumeError::InvalidRange)?);
            self.buf.extend_from_slice(&data);
            return Ok(true);
        }
        Ok(false)
    }
}

impl<S: BlockSource> Iterator for MftReader<S> {
    type Item = Result<MftItem, VolumeError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_cancellable(&mut || false)
    }
}

/// Zero-filled slots skipped between two polls of the stop callback.
const STOP_POLL_SLOTS: u64 = 4096;

impl<S: BlockSource> MftReader<S> {
    /// Whether the iteration ended because `stop` returned true.
    pub fn was_stopped(&self) -> bool {
        self.stopped
    }

    /// Like `next`, but polls `stop` before every chunk read and every 4096 skipped empty slots, so a long run
    /// of unused records or a slow disk cannot delay cancellation. When it returns true the iteration ends.
    pub fn next_cancellable(&mut self, stop: &mut dyn FnMut() -> bool) -> Option<<Self as Iterator>::Item> {
        let mut skipped_run = 0u64;
        let record_size = usize::try_from(self.geometry.bytes_per_record).ok()?;
        loop {
            if self.done || self.next_number >= self.max_records {
                self.done = true;
                return None;
            }
            let end = self.pos.checked_add(record_size)?;
            if let Some(slot) = self.buf.get_mut(self.pos..end) {
                self.pos = end;
                let number = self.next_number;
                self.next_number += 1;
                if slot.iter().all(|&b| b == 0) {
                    self.skipped_empty += 1;
                    skipped_run += 1;
                    if skipped_run.is_multiple_of(STOP_POLL_SLOTS) && stop() {
                        self.stopped = true;
                        self.done = true;
                        return None;
                    }
                    continue;
                }
                let record = parse_raw_record(slot, &self.limits.parse);
                return Some(Ok(MftItem { number, record }));
            }
            if stop() {
                self.stopped = true;
                self.done = true;
                return None;
            }
            match self.refill() {
                Ok(true) => {
                    // A read can take long; look again before spending time on the new chunk.
                    if stop() {
                        self.stopped = true;
                        self.done = true;
                        return None;
                    }
                }
                Ok(false) => {
                    self.done = true;
                    return None;
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
