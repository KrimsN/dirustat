//! Synthetic NTFS volume images for tests: a contiguous `$MFT` at cluster 1 in a `MemSource`, built with the
//! record builders of `dirustat-ntfs` (feature `testutil`). No real volume and no administrator rights.

use std::collections::BTreeMap;

use dirustat_model::{NodeId, NodeKind, Tree};
use dirustat_ntfs::testutil::{
    NonResident, RecordBuilder, UNIX_EPOCH_FILETIME, encode_runs, file_name, nonresident, resident, standard_info,
    utf16,
};
use dirustat_ntfs::{MemSource, NtfsVolumeData, Run};

/// Sequence number of every record (and of every parent reference that should match).
pub(crate) const SEQ: u16 = 7;
pub(crate) const CLUSTER: usize = 4096;
pub(crate) const RECORD: usize = 1024;
pub(crate) const WIN32: u8 = 1;
pub(crate) const DOS: u8 = 2;
/// Where the MFT starts on the image, in bytes.
pub(crate) const MFT_OFFSET: usize = CLUSTER;
/// Unix time used for the sample modification times.
pub(crate) const SAMPLE_MTIME: i64 = 1_700_000_000;

pub(crate) fn filetime(unix_secs: i64) -> u64 {
    UNIX_EPOCH_FILETIME + u64::try_from(unix_secs).unwrap() * 10_000_000
}

/// A base record with standard information (modified = [`SAMPLE_MTIME`]) and one Win32 name.
pub(crate) fn record(number: u32, dir: bool, parent: u32, name: &str) -> RecordBuilder {
    let mut b = if dir { RecordBuilder::directory() } else { RecordBuilder::file() };
    b.sequence = SEQ;
    b.number = Some(number);
    b.attr(standard_info(filetime(SAMPLE_MTIME), if dir { 0x10 } else { 0x20 })).attr(file_name(
        u64::from(parent),
        SEQ,
        WIN32,
        &utf16(name),
        0,
        0,
    ))
}

/// Adds a resident unnamed `$DATA` of `len` bytes.
pub(crate) fn with_resident(b: RecordBuilder, len: usize) -> RecordBuilder {
    b.attr(resident(0x80, &[], 0, &vec![b'x'; len]))
}

/// Adds a non-resident unnamed `$DATA`.
pub(crate) fn with_data(b: RecordBuilder, size: u64, allocated: u64) -> RecordBuilder {
    b.attr(nonresident(&NonResident {
        size,
        allocated,
        initialized: size,
        runs: &encode_runs(&[Run { lcn: Some(100), len: (allocated / 4096).max(1) }]),
        ..NonResident::default()
    }))
}

/// A volume under construction, keyed by record number. Starts with a root directory and the usual metafiles.
pub(crate) struct Img {
    pub records: BTreeMap<u32, Vec<u8>>,
}

impl Img {
    pub fn new() -> Self {
        let mut img = Self { records: BTreeMap::new() };
        img.put(5, record(5, true, 5, "."));
        for (n, name, dir) in [
            (1, "$MftMirr", false),
            (2, "$LogFile", false),
            (3, "$Volume", false),
            (6, "$Bitmap", false),
            (8, "$BadClus", false),
            (11, "$Extend", true),
        ] {
            img.put(n, with_data(record(n, dir, 5, name), 1 << 20, 1 << 20));
        }
        // System subtree below `$Extend`: must be skipped without becoming `<orphaned>`.
        img.put(25, with_data(record(25, false, 11, "$UsnJrnl"), 1 << 20, 1 << 20));
        img.put(26, record(26, true, 11, "$RmMetadata"));
        img.put(27, record(27, true, 26, "$Txf"));
        img.put(28, with_data(record(28, false, 27, "$TxfLog"), 4096, 4096));
        img
    }

    pub fn put(&mut self, number: u32, b: RecordBuilder) -> &mut Self {
        let mut b = b;
        b.number = Some(number);
        self.records.insert(number, b.build());
        self
    }

    pub fn dir(&mut self, number: u32, parent: u32, name: &str) -> &mut Self {
        self.put(number, record(number, true, parent, name))
    }

    /// A file with a resident `$DATA` of `len` bytes.
    pub fn small(&mut self, number: u32, parent: u32, name: &str, len: usize) -> &mut Self {
        self.put(number, with_resident(record(number, false, parent, name), len))
    }

    /// A file with a non-resident `$DATA`.
    pub fn big(&mut self, number: u32, parent: u32, name: &str, size: u64, allocated: u64) -> &mut Self {
        self.put(number, with_data(record(number, false, parent, name), size, allocated))
    }

    /// Record 0 describing a `$MFT` of `size` bytes whose runs are `runs`.
    pub fn record0(size: u64, allocated: u64, runs: &[Run]) -> Vec<u8> {
        let mut b = record(0, false, 5, "$MFT");
        b.number = Some(0);
        b.attr(nonresident(&NonResident {
            size,
            allocated,
            initialized: size,
            runs: &encode_runs(runs),
            ..NonResident::default()
        }))
        .build()
    }

    /// Lays the records out on a disk image. Slots without a record stay zero-filled.
    pub fn finish(&self) -> (MemSource, NtfsVolumeData) {
        let last = self.records.keys().next_back().copied().unwrap_or(0).max(24);
        let slots = usize::try_from(last).unwrap() + 1;
        let per_cluster = CLUSTER / RECORD;
        let clusters = slots.div_ceil(per_cluster);
        let size = u64::try_from(clusters * CLUSTER).unwrap();
        let rec0 = Self::record0(
            size,
            size,
            &[Run { lcn: Some(u64::try_from(MFT_OFFSET / CLUSTER).unwrap()), len: u64::try_from(clusters).unwrap() }],
        );
        let mut disk = vec![0u8; MFT_OFFSET + clusters * CLUSTER + 2 * CLUSTER];
        disk[MFT_OFFSET..MFT_OFFSET + RECORD].copy_from_slice(&rec0);
        for (n, bytes) in &self.records {
            let at = MFT_OFFSET + usize::try_from(*n).unwrap() * RECORD;
            disk[at..at + bytes.len()].copy_from_slice(bytes);
        }
        let volume = NtfsVolumeData {
            volume_serial_number: 1,
            bytes_per_sector: 512,
            bytes_per_cluster: u32::try_from(CLUSTER).unwrap(),
            bytes_per_file_record: u32::try_from(RECORD).unwrap(),
            total_clusters: u64::try_from(disk.len() / CLUSTER).unwrap(),
            total_bytes: u64::try_from(disk.len()).unwrap(),
            mft_valid_data_length: size,
            mft_start_lcn: u64::try_from(MFT_OFFSET / CLUSTER).unwrap(),
        };
        (MemSource(disk), volume)
    }
}

/// Byte offset of record `number` on an image made by [`Img::finish`].
pub(crate) fn record_at(number: usize) -> usize {
    MFT_OFFSET + number * RECORD
}

/// Every node as `path|kind|logical|allocated`, sorted. Iterative; the root is written as `/`.
pub(crate) fn dump(tree: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack: Vec<(NodeId, String)> = vec![(tree.root(), String::new())];
    while let Some((id, path)) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        let shown = if path.is_empty() { "/".to_owned() } else { path.clone() };
        let kind = match node.kind() {
            NodeKind::Dir => "dir",
            NodeKind::File => "file",
            NodeKind::Link => "link",
        };
        out.push(format!("{shown}|{kind}|{}|{}", node.data().logical_size, node.data().allocated_size));
        for child in tree.children(id) {
            if let Some(c) = tree.get(child) {
                stack.push((child, format!("{path}/{}", c.name())));
            }
        }
    }
    out.sort();
    out
}
