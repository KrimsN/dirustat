//! The NTFS fast path: builds a `Tree` from the records of the `$MFT` instead of walking directories.
//!
//! Pipeline (all iterative, all bounded, nothing here panics on hostile input):
//!
//! 1. `collect`: read record 0, locate every extent of the `$MFT`, stream the records through
//!    `MftReader` and keep one compact [`Entry`] per usable base record plus one compact [`Link`] per
//!    hard link name. Names are raw UTF-16 and live in a single shared pool, so a million records cost
//!    a few dozen bytes each plus their names, not a `Vec` per record. Parsed records are dropped at once.
//! 2. `build`: resolve parent references to entry indexes (binary search by record number), group the links
//!    by parent with a counting sort, then create the nodes top-down from record 5 with an explicit stack.
//!
//! Policies (the tests pin each of them):
//!
//! * Only base records that are in use are kept. Extension records (`base != None`) are counted and
//!   ignored: attributes that live only in an extension record are not merged in this version, so a file
//!   whose unnamed `$DATA` sits in one is shown with size 0 and flagged `error`.
//! * Metafiles (record numbers below 24, except the root directory 5) are skipped, and so is everything
//!   below them (`$Extend\$UsnJrnl`, `$RmMetadata`, ...): their parent is a metafile.
//! * A file with several hard links gets one node per link name (DOS-only 8.3 duplicates excluded). The
//!   size is charged to the first node created in traversal order; the other nodes carry size 0, so the
//!   volume total is never inflated. The portable scan applies the same rule where `std` exposes it.
//! * A link whose parent is missing, not a directory, in a different generation (sequence mismatch) or
//!   itself unreachable (parent cycles, self-parents) is attached below a synthetic `<orphaned>` directory
//!   flagged `error`. Directories are descended into at most once, so cycles cannot loop.
//! * Reparse points (symlinks, junctions, mount points) become `NodeKind::Link` and are never followed.
//!   A directory link has size 0; a file link keeps the size recorded for it.
//! * Logical size is the unnamed `$DATA` size. Allocated size is the recorded allocation, or the stored
//!   (compressed) size for compressed and sparse streams. Resident data lives inside the MFT record and
//!   occupies no clusters, so its allocated size is 0 by design. Alternate data streams are not counted yet.
//! * Timestamps are the `$STANDARD_INFORMATION` modification time converted to Unix seconds.

mod build;
mod collect;

#[cfg(test)]
pub(crate) mod testimg;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::atomic::AtomicBool;

use dirustat_model::{NodeKind, Tree};
use dirustat_ntfs::{BlockSource, NtfsVolumeData, ParseError, Volume, VolumeError};

use crate::{Counters, ScanOutcome};

/// First record number that can hold a user file. Records below it are NTFS metafiles (`$MFT`, `$Bitmap`...).
pub(crate) const FIRST_USER_RECORD: u32 = 24;
/// The record number of the volume root directory (`.`).
pub(crate) const ROOT_RECORD: u32 = 5;
/// `FILE_ATTRIBUTE_DIRECTORY`, added to directory nodes so they look like the portable scan's.
pub(crate) const ATTR_DIRECTORY: u32 = 0x10;
/// Name of the synthetic directory that collects records which cannot be placed.
pub(crate) const ORPHAN_DIR: &str = "<orphaned>";

/// Why the MFT path could not produce a tree. Every variant makes `start` fall back to the portable walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MftError {
    /// The volume could not be opened or read (access denied, not NTFS, I/O error, short read).
    Volume(VolumeError),
    /// The `$MFT` itself could not be understood (record 0 corrupt, extents incomplete, ...).
    Parse(ParseError),
    /// Record 5 is missing, not in use or not a directory.
    NoRoot,
    /// Too many records failed validation for the result to be trusted.
    TooManyCorrupt { corrupt: u64, seen: u64 },
    /// A safety cap was hit (record, link or name pool size).
    TooLarge(&'static str),
}

impl fmt::Display for MftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Volume(e) => write!(f, "volume: {e}"),
            Self::Parse(e) => write!(f, "MFT: {e}"),
            Self::NoRoot => f.write_str("MFT has no usable root directory record"),
            Self::TooManyCorrupt { corrupt, seen } => write!(f, "{corrupt} of {seen} MFT records are corrupt"),
            Self::TooLarge(what) => write!(f, "MFT {what} limit exceeded"),
        }
    }
}

impl std::error::Error for MftError {}

impl From<VolumeError> for MftError {
    fn from(e: VolumeError) -> Self {
        Self::Volume(e)
    }
}

impl From<ParseError> for MftError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

/// What the MFT pass saw; logged and available to tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MftStats {
    /// Record slots that were not zero-filled.
    pub records_seen: u64,
    /// Records that failed fixup, header or number validation.
    pub corrupt: u64,
    pub not_in_use: u64,
    /// Extension records (`base != None`), ignored in this version.
    pub extension_records: u64,
    /// Metafile records below 24 (except the root).
    pub metafiles: u64,
    /// In-use user records without any usable `$FILE_NAME`.
    pub nameless: u64,
    /// Entries kept (base records that became candidates for nodes).
    pub entries: u64,
    /// Link names kept (entries can have several).
    pub links: u64,
    /// Link names dropped because a record exceeded the per-record cap.
    pub links_dropped: u64,
    /// Entries below metafiles (`$Extend` children and similar), skipped on purpose.
    pub system_entries: u64,
    /// Links attached under `<orphaned>` because their parent could not be used.
    pub orphan_links: u64,
    /// Entries attached under `<orphaned>` because they were unreachable (parent cycles).
    pub cycle_entries: u64,
    /// Extra links to an already placed directory that were dropped.
    pub duplicate_dirs: u64,
    /// Nodes that could not be added (node limit).
    pub node_errors: u64,
}

/// Caps of the MFT pass. Every allocation driven by volume data is bounded by one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MftLimits {
    /// Entries kept; 2^27 covers every real volume (a 20 TB volume with tiny files has ~2^27 records).
    pub max_entries: usize,
    /// Link names kept in total.
    pub max_links: usize,
    /// UTF-16 units in the shared name pool (offsets are `u32`, 2^30 units = 2 GiB).
    pub max_name_units: usize,
    /// Hard links read from one record. NTFS itself allows 1024.
    pub max_links_per_record: usize,
    /// A scan is rejected when at least this many records are corrupt *and* they are more than a quarter of
    /// the records seen. A handful of torn records on a live volume is normal; wholesale damage is not.
    pub min_corrupt_to_fail: u64,
}

impl Default for MftLimits {
    fn default() -> Self {
        Self {
            max_entries: 1 << 27,
            max_links: 1 << 28,
            max_name_units: 1 << 30,
            max_links_per_record: 1024,
            min_corrupt_to_fail: 8,
        }
    }
}

/// Result of a successful MFT scan.
#[derive(Debug)]
pub struct MftScan {
    pub outcome: ScanOutcome,
    pub stats: MftStats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    Dir,
    File,
    /// A reparse point: never followed.
    Link,
}

impl EntryKind {
    pub(crate) fn node_kind(self) -> NodeKind {
        match self {
            Self::Dir => NodeKind::Dir,
            Self::File => NodeKind::File,
            Self::Link => NodeKind::Link,
        }
    }
}

/// One kept base record. Entries are stored in increasing record number order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    pub number: u32,
    pub sequence: u16,
    /// Index of this entry's first link in `Collected::links`; its links are contiguous.
    pub link_start: u32,
    pub link_count: u16,
    pub kind: EntryKind,
    pub error: bool,
    pub attributes: u32,
    pub logical: u64,
    pub allocated: u64,
    pub mtime: Option<i64>,
}

/// Parent reference value: no usable parent.
pub(crate) const NO_PARENT: u32 = u32::MAX;
/// Parent reference value: the parent is a metafile, the link belongs to a skipped system subtree.
pub(crate) const SYSTEM_PARENT: u32 = u32::MAX - 1;

/// One hard link name of an entry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Link {
    /// Index of the owning entry.
    pub owner: u32,
    /// The parent *record number* while collecting; after resolution an entry index, [`NO_PARENT`] or
    /// [`SYSTEM_PARENT`].
    pub parent: u32,
    pub parent_sequence: u16,
    pub name_len: u16,
    /// Offset of the name in the shared pool.
    pub name_offset: u32,
}

/// Everything phase 1 keeps about the volume.
#[derive(Debug, Default)]
pub(crate) struct Collected {
    pub entries: Vec<Entry>,
    pub links: Vec<Link>,
    /// Shared UTF-16 name pool.
    pub names: Vec<u16>,
    pub stats: MftStats,
    /// The cancel flag was observed; the data is incomplete.
    pub cancelled: bool,
}

/// Builds a tree from the MFT of `volume`, read through `source`.
///
/// `label` is the root node's name. Progress goes to `counters`, cancellation is read from `cancel`; a
/// cancelled scan returns `Ok` with `cancelled: true` and a partial (possibly root-only) tree.
pub(crate) fn scan_mft<S: BlockSource>(
    source: S,
    volume: &NtfsVolumeData,
    label: &str,
    cancel: &AtomicBool,
    counters: &Counters,
) -> Result<MftScan, MftError> {
    scan_mft_with(source, volume, label, &MftLimits::default(), cancel, counters)
}

/// [`scan_mft`] with explicit limits.
pub(crate) fn scan_mft_with<S: BlockSource>(
    source: S,
    volume: &NtfsVolumeData,
    label: &str,
    limits: &MftLimits,
    cancel: &AtomicBool,
    counters: &Counters,
) -> Result<MftScan, MftError> {
    let collected = collect::collect(source, volume, limits, cancel, counters)?;
    if collected.cancelled {
        let mut tree = Tree::new(label);
        tree.aggregate();
        let outcome = ScanOutcome::new(tree, true, crate::ScanBackend::Mft);
        return Ok(MftScan { outcome, stats: collected.stats });
    }
    let (tree, cancelled, stats) = build::build(collected, label, cancel, counters)?;
    tracing::info!(?stats, nodes = tree.len(), "MFT scan finished");
    Ok(MftScan { outcome: ScanOutcome::new(tree, cancelled, crate::ScanBackend::Mft), stats })
}

/// Opens `volume` (for example `C:`) read-only and scans its MFT. Needs administrator rights.
pub(crate) fn scan_volume(
    volume: &str,
    label: &str,
    cancel: &AtomicBool,
    counters: &Counters,
) -> Result<MftScan, MftError> {
    let vol = Volume::open(volume)?;
    let data = *vol.data();
    scan_mft(&vol, &data, label, cancel, counters)
}
