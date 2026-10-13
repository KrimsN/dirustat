//! Phase 1: stream the MFT and keep compact entries and links.

use std::sync::atomic::{AtomicBool, Ordering};

use dirustat_ntfs::{
    BlockSource, Extent, MftGeometry, MftReader, MftRecord, NtfsVolumeData, ParseError, ParseLimits, ReaderLimits,
    mft_extents, parse_raw_record,
};

use super::{
    Collected, Entry, EntryKind, FIRST_USER_RECORD, Link, MftError, MftLimits, NO_MTIME, NO_PARENT, ROOT_RECORD,
    SYSTEM_PARENT,
};
use crate::Counters;

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
const ATTR_REPARSE: u32 = 0x400;
/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const FILETIME_UNIX_OFFSET_SECS: i64 = 11_644_473_600;
/// FILETIME ticks (100 ns) per second.
const TICKS_PER_SEC: u64 = 10_000_000;

/// FILETIME to Unix seconds; 0 (never set) and out-of-range values give `None`.
pub(super) fn filetime_to_unix(filetime: u64) -> Option<i64> {
    if filetime == 0 {
        return None;
    }
    i64::try_from(filetime / TICKS_PER_SEC).ok()?.checked_sub(FILETIME_UNIX_OFFSET_SECS)
}

/// Reads record 0 and returns the `$MFT` extents plus the number of valid MFT bytes.
fn locate_mft<S: BlockSource>(source: &S, volume: &NtfsVolumeData) -> Result<(Vec<Extent>, u64), MftError> {
    let offset = volume.mft_offset().ok_or(dirustat_ntfs::VolumeError::Malformed)?;
    let record_len = usize::try_from(volume.bytes_per_file_record).map_err(|_| ParseError::BadGeometry)?;
    let sector = usize::try_from(volume.bytes_per_sector).map_err(|_| ParseError::BadGeometry)?;
    // Raw reads are sector-granular; a record can be smaller than a sector on exotic volumes.
    let mut buf = source.read_at(offset, record_len.max(sector))?;
    let record = buf.get_mut(..record_len).ok_or(ParseError::Truncated)?;
    let record0 = parse_raw_record(record, &ParseLimits::default())?;
    if !record0.in_use || record0.base.is_some() || record0.record_number.is_some_and(|n| n != 0) {
        return Err(ParseError::BadHeader.into());
    }
    let extents = mft_extents(&record0, volume.bytes_per_cluster)?;
    // The MFT cannot be larger than the volume, nor than the valid length the driver reports (when known).
    let mut size = record0.logical_size().min(volume.total_bytes);
    if volume.mft_valid_data_length > 0 {
        size = size.min(volume.mft_valid_data_length);
    }
    Ok((extents, size))
}

pub(super) fn collect<S: BlockSource>(
    source: S,
    volume: &NtfsVolumeData,
    limits: &MftLimits,
    cancel: &AtomicBool,
    counters: &Counters,
) -> Result<Collected, MftError> {
    let geometry = MftGeometry::from_volume(volume)?;
    let (extents, mft_size) = locate_mft(&source, volume)?;
    let reader_limits = ReaderLimits {
        // Record numbers must fit `u32`; the two top values are reserved as sentinels.
        max_records: u64::from(u32::MAX - 2),
        // Run lists are only needed for record 0; skipping them keeps parsing cheap and allocation-free.
        parse: ParseLimits { decode_runs: false, ..ParseLimits::default() },
        ..ReaderLimits::default()
    };
    // Extents are cross-checked against the volume size, which also caps the record count.
    let mut reader =
        MftReader::new(&source, geometry, extents, mft_size, reader_limits)?.bounded_by(volume.total_bytes)?;
    let caps = Caps::derive(limits, mft_size, volume.bytes_per_file_record);
    let mut c = Collected::default();
    // The reader polls the flag before every chunk and inside runs of unused slots, so cancellation
    // does not wait for a whole chunk of records or a long stretch of zeroes.
    while let Some(item) = reader.next_cancellable(&mut || cancel.load(Ordering::Relaxed)) {
        let item = item?;
        c.stats.records_seen = c.stats.records_seen.saturating_add(1);
        match item.record {
            Ok(record) => keep(&mut c, item.number, &record, limits, &caps, counters)?,
            Err(_) => c.stats.corrupt = c.stats.corrupt.saturating_add(1),
        }
    }
    if reader.was_stopped() {
        c.cancelled = true;
        return Ok(c);
    }
    if c.stats.corrupt >= limits.min_corrupt_to_fail && c.stats.corrupt.saturating_mul(4) > c.stats.records_seen {
        return Err(MftError::TooManyCorrupt { corrupt: c.stats.corrupt, seen: c.stats.records_seen });
    }
    Ok(c)
}

/// Allocation caps derived from the size of the MFT itself: it cannot hold more entries than records,
/// and the fixed hard limits only matter for forged sizes.
pub(super) struct Caps {
    pub entries: usize,
    pub links: usize,
    pub name_units: usize,
}

impl Caps {
    pub fn derive(limits: &MftLimits, mft_size: u64, record_size: u32) -> Self {
        let records = mft_size / u64::from(record_size.max(1));
        let records = usize::try_from(records).unwrap_or(usize::MAX);
        // Typical volumes have well under two names per record; 8 leaves room for hardlink-heavy trees.
        let links = limits.max_links.min(records.saturating_mul(MAX_LINKS_PER_RECORD_AVG));
        Self {
            entries: limits.max_entries.min(records),
            links,
            // A name has at most 255 UTF-16 units.
            name_units: limits.max_name_units.min(links.saturating_mul(255)),
        }
    }
}

/// Average number of link names per MFT record that a volume may legitimately reach.
const MAX_LINKS_PER_RECORD_AVG: usize = 8;

/// Grows `v` so `extra` more items fit, failing with a typed error instead of aborting on OOM.
fn reserve_for<T>(v: &mut Vec<T>, extra: usize, what: &'static str) -> Result<(), MftError> {
    if v.capacity().saturating_sub(v.len()) >= extra {
        return Ok(());
    }
    v.try_reserve(extra.max(v.len() / 2).max(1024)).map_err(|_| MftError::TooLarge(what))
}

/// Symlink and mount point (junction) reparse tags.
const TAG_SYMLINK: u32 = 0xA000_000C;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;

/// Symlinks and junctions are links. Other reparse points (cloud placeholders, dedup, WIM...) keep
/// their real contents, so files stay files; an unknown reparse *directory* is still never descended.
fn classify(record: &MftRecord, attributes: u32) -> EntryKind {
    if attributes & ATTR_REPARSE != 0 {
        match record.reparse_tag {
            Some(TAG_SYMLINK | TAG_MOUNT_POINT) => EntryKind::Link,
            _ if record.is_directory => EntryKind::Link,
            _ => EntryKind::File,
        }
    } else if record.is_directory {
        EntryKind::Dir
    } else {
        EntryKind::File
    }
}

/// `(logical, allocated)` of the file contents, see the module documentation for the policy.
fn sizes_of(record: &MftRecord, kind: EntryKind) -> (u64, u64) {
    if kind == EntryKind::Dir {
        return (0, 0);
    }
    let Some(data) = record.data.as_ref() else { return (0, 0) };
    let allocated = if data.resident {
        0
    } else if data.compressed || data.sparse {
        data.compressed_size.unwrap_or(data.allocated)
    } else {
        data.allocated
    };
    (data.size, allocated)
}

fn keep(
    c: &mut Collected,
    number: u64,
    record: &MftRecord,
    limits: &MftLimits,
    caps: &Caps,
    counters: &Counters,
) -> Result<(), MftError> {
    if !record.in_use {
        c.stats.not_in_use = c.stats.not_in_use.saturating_add(1);
        return Ok(());
    }
    let Ok(number) = u32::try_from(number) else {
        c.stats.corrupt = c.stats.corrupt.saturating_add(1);
        return Ok(());
    };
    // A record that claims another number than its slot is misplaced or stale: do not trust it.
    if record.record_number.is_some_and(|n| n != number) {
        c.stats.corrupt = c.stats.corrupt.saturating_add(1);
        return Ok(());
    }
    if record.base.is_some() {
        c.stats.extension_records = c.stats.extension_records.saturating_add(1);
        return Ok(());
    }
    if number < FIRST_USER_RECORD && number != ROOT_RECORD {
        c.stats.metafiles = c.stats.metafiles.saturating_add(1);
        return Ok(());
    }
    let is_root = number == ROOT_RECORD;
    // The root's own name (".", parent = itself) is never used, so it is not stored.
    let names = if is_root { Vec::new() } else { record.link_names() };
    if names.is_empty() && !is_root {
        c.stats.nameless = c.stats.nameless.saturating_add(1);
        return Ok(());
    }
    let take = names.len().min(limits.max_links_per_record);
    c.stats.links_dropped =
        c.stats.links_dropped.saturating_add(u64::try_from(names.len().saturating_sub(take)).unwrap_or(u64::MAX));
    if c.entries.len() >= caps.entries {
        return Err(MftError::TooLarge("record"));
    }
    if c.links.len().saturating_add(take) > caps.links {
        return Err(MftError::TooLarge("link"));
    }
    reserve_for(&mut c.entries, 1, "record")?;
    reserve_for(&mut c.links, take, "link")?;
    let owner = u32::try_from(c.entries.len()).map_err(|_| MftError::TooLarge("record"))?;
    let link_start = u32::try_from(c.links.len()).map_err(|_| MftError::TooLarge("link"))?;
    let mut stored = 0u16;
    for name in names.iter().take(take) {
        let Ok(name_len) = u16::try_from(name.name.len()) else { continue };
        let pool_end = c.names.len().saturating_add(name.name.len());
        if pool_end > caps.name_units {
            return Err(MftError::TooLarge("name pool"));
        }
        reserve_for(&mut c.names, name.name.len(), "name pool")?;
        let name_offset = u32::try_from(c.names.len()).map_err(|_| MftError::TooLarge("name pool"))?;
        c.names.extend_from_slice(&name.name);
        c.links.push(Link {
            owner,
            parent: u32::try_from(name.parent.record).ok().filter(|&p| p < SYSTEM_PARENT).unwrap_or(NO_PARENT),
            parent_sequence: name.parent.sequence,
            name_len,
            name_offset,
        });
        stored = stored.saturating_add(1);
    }
    let attributes = record.standard_info.map_or_else(|| record.best_name().map_or(0, |n| n.flags), |s| s.attributes);
    let kind = classify(record, attributes);
    let (logical, allocated) = sizes_of(record, kind);
    // The unnamed `$DATA` of a fragmented file may live in an extension record that is not merged yet.
    // (`DataStream`s that start past VCN 0 are ignored by the parser for the same reason.)
    let data_missing = kind == EntryKind::File && record.data.is_none() && record.has_attribute_list();
    if data_missing {
        c.stats.undercounted_files = c.stats.undercounted_files.saturating_add(1);
    }
    c.entries.push(Entry {
        number,
        sequence: record.sequence,
        link_start,
        link_count: stored,
        kind,
        error: data_missing,
        attributes,
        logical,
        allocated,
        mtime: record.standard_info.and_then(|s| filetime_to_unix(s.modified)).unwrap_or(NO_MTIME),
    });
    c.stats.entries = c.stats.entries.saturating_add(1);
    c.stats.links = c.stats.links.saturating_add(u64::from(stored));
    if kind == EntryKind::Dir {
        counters.dirs.fetch_add(1, Ordering::Relaxed);
    } else {
        counters.files.fetch_add(1, Ordering::Relaxed);
        counters.add_bytes(logical);
    }
    Ok(())
}
