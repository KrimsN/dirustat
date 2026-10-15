use std::sync::atomic::{AtomicBool, Ordering};

use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use dirustat_ntfs::testutil::{NonResident, encode_runs, file_name, nonresident, standard_info, utf16};
use dirustat_ntfs::{BlockSource, FileRef, MemSource, NtfsVolumeData, ParseError, Run, VolumeError};
use proptest::prelude::*;

use super::testimg::*;
use super::*;
use crate::ScanBackend;

fn scan_image(src: &MemSource, vol: &NtfsVolumeData) -> Result<MftScan, MftError> {
    scan_mft(src, vol, "V:", &AtomicBool::new(false), &Counters::default())
}

fn run(img: &Img) -> Result<MftScan, MftError> {
    let (src, vol) = img.finish();
    scan_image(&src, &vol)
}

fn ok(img: &Img) -> MftScan {
    run(img).unwrap()
}

fn find(tree: &Tree, parent: NodeId, name: &str) -> Option<NodeId> {
    tree.children(parent).find(|&c| tree.get(c).is_some_and(|n| n.name() == name))
}

fn path(tree: &Tree, parts: &[&str]) -> Option<NodeId> {
    parts.iter().try_fold(tree.root(), |id, p| find(tree, id, p))
}

fn logical(tree: &Tree, id: NodeId) -> u64 {
    tree.get(id).unwrap().total_size(SizeMode::Logical)
}

fn allocated(tree: &Tree, id: NodeId) -> u64 {
    tree.get(id).unwrap().total_size(SizeMode::Allocated)
}

fn sample() -> Img {
    let mut img = Img::new();
    img.dir(30, 5, "docs");
    img.big(31, 30, "a.txt", 10_000, 12_288);
    img.small(32, 30, "b.txt", 100);
    img.big(33, 5, "top.bin", 5_000, 8_192);
    img.small(34, 5, "Ünï ✓ 日本語.txt", 3);
    img
}

#[test]
fn builds_the_expected_tree_with_sizes() {
    let out = ok(&sample());
    let tree = &out.outcome.tree;
    assert!(!out.outcome.cancelled);
    assert_eq!(out.outcome.backend, ScanBackend::Mft);
    assert_eq!(out.outcome.fallback_reason, None);
    assert_eq!(
        dump(tree),
        vec![
            "/docs/a.txt|file|10000|12288",
            "/docs/b.txt|file|100|0",
            "/docs|dir|0|0",
            "/top.bin|file|5000|8192",
            "/|dir|0|0",
            "/Ünï ✓ 日本語.txt|file|3|0",
        ]
    );
    let root = tree.get(tree.root()).unwrap();
    assert_eq!(root.total_size(SizeMode::Logical), 15_103);
    assert_eq!(root.total_size(SizeMode::Allocated), 20_480);
    assert_eq!((root.dir_count(), root.file_count(), root.error_count()), (2, 4, 0));
    assert_eq!(tree.get(tree.root()).unwrap().name(), "V:");
}

#[test]
fn mtime_attributes_and_lossy_names_are_carried() {
    let mut img = Img::new();
    img.small(40, 5, "ok", 1);
    img.put(41, with_resident(record(41, false, 5, "x"), 1));
    // A name with an unpaired surrogate.
    let mut bad = record(42, false, 5, "placeholder");
    bad.attrs.pop();
    bad.attrs.push(file_name(5, SEQ, WIN32, &[0x66, 0xD800], 0, 0));
    img.put(42, bad);
    img.dir(43, 5, "d");
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let ok_node = tree.get(find(tree, tree.root(), "ok").unwrap()).unwrap();
    assert_eq!(ok_node.data().mtime, Some(SAMPLE_MTIME));
    assert_eq!(ok_node.data().attributes, 0x20);
    let lossy = tree.get(find(tree, tree.root(), "f\u{FFFD}").unwrap()).unwrap();
    assert!(lossy.data().name_lossy);
    let dir = tree.get(find(tree, tree.root(), "d").unwrap()).unwrap();
    assert_eq!(dir.kind(), NodeKind::Dir);
    assert_eq!(dir.data().attributes & 0x10, 0x10);
}

#[test]
fn metafiles_and_their_children_are_skipped() {
    let mut img = sample();
    // Record 24 is the first user record and must be kept.
    img.small(24, 5, "first-user.txt", 1);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let names: Vec<_> = tree.children(tree.root()).filter_map(|c| tree.get(c)).map(|n| n.name().to_owned()).collect();
    assert!(!names.iter().any(|n| n.starts_with('$') || n == "<orphaned>"), "{names:?}");
    assert!(names.contains(&"first-user.txt".to_owned()));
    assert_eq!(out.stats.metafiles, 7);
    // $UsnJrnl, $RmMetadata, $Txf, $TxfLog.
    assert_eq!(out.stats.system_entries, 4);
    assert_eq!(out.stats.orphan_links, 0);
    assert_eq!(tree.get(tree.root()).unwrap().error_count(), 0);
}

#[test]
fn deleted_and_extension_records_are_skipped_and_counted() {
    let mut img = sample();
    let mut deleted = with_data(record(70, false, 5, "deleted.bin"), 777, 4096);
    deleted.flags = 0;
    img.put(70, deleted);
    let mut ext = with_data(record(71, false, 5, "ext.bin"), 555, 4096);
    ext.base = FileRef { record: 33, sequence: SEQ }.to_raw();
    img.put(71, ext);
    let out = ok(&img);
    assert_eq!((out.stats.not_in_use, out.stats.extension_records), (1, 1));
    assert!(path(&out.outcome.tree, &["deleted.bin"]).is_none());
    assert!(path(&out.outcome.tree, &["ext.bin"]).is_none());
    assert_eq!(logical(&out.outcome.tree, out.outcome.tree.root()), 15_103);
}

#[test]
fn file_with_unmerged_attribute_list_is_flagged() {
    let mut img = Img::new();
    let mut b = record(40, false, 5, "fragmented.bin");
    b.attrs.push(dirustat_ntfs::testutil::resident(
        0x20,
        &[],
        0,
        &dirustat_ntfs::testutil::list_entry(0x80, false, 0, 99),
    ));
    img.put(40, b);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let node = tree.get(path(tree, &["fragmented.bin"]).unwrap()).unwrap();
    assert!(node.data().error);
    assert_eq!(tree.get(tree.root()).unwrap().error_count(), 1);
}

#[test]
fn hardlinks_get_one_node_per_name_and_are_charged_once() {
    let mut img = Img::new();
    img.dir(30, 5, "one");
    img.dir(31, 5, "two");
    let mut file = record(40, false, 30, "a.txt");
    file.attrs.push(file_name(30, SEQ, DOS, &utf16("A~1.TXT"), 0, 0));
    file.attrs.push(file_name(31, SEQ, WIN32, &utf16("b.txt"), 0, 0));
    img.put(40, with_data(file, 1_000, 4_096));
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let first = path(tree, &["one", "a.txt"]).unwrap();
    let second = path(tree, &["two", "b.txt"]).unwrap();
    // The DOS name in the same directory is not a separate link.
    assert_eq!(tree.children(path(tree, &["one"]).unwrap()).count(), 1);
    let sizes = [first, second].map(|id| tree.get(id).unwrap().data().logical_size);
    assert_eq!(sizes.iter().sum::<u64>(), 1_000);
    assert!(sizes.contains(&0));
    let root = tree.get(tree.root()).unwrap();
    assert_eq!((root.total_size(SizeMode::Logical), root.total_size(SizeMode::Allocated)), (1_000, 4_096));
    assert_eq!(root.file_count(), 2);
    assert_eq!(out.stats.links, 2 + 2 + 4);
}

#[test]
fn orphans_go_below_a_flagged_synthetic_directory() {
    let mut img = sample();
    img.small(50, 99, "missing-parent.txt", 10);
    img.small(51, 33, "parent-is-a-file.txt", 20);
    let mut stale = with_resident(record(52, false, 30, "stale-generation.txt"), 30);
    stale.attrs.remove(1);
    stale.attrs.insert(1, file_name(30, SEQ + 2, WIN32, &utf16("stale-generation.txt"), 0, 0));
    img.put(52, stale);
    img.dir(53, 98, "lost-dir");
    img.small(54, 53, "inside-lost.txt", 40);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let orphans = find(tree, tree.root(), "<orphaned>").unwrap();
    assert!(tree.get(orphans).unwrap().data().error);
    for name in ["missing-parent.txt", "parent-is-a-file.txt", "stale-generation.txt", "lost-dir"] {
        assert!(find(tree, orphans, name).is_some(), "{name}");
    }
    assert!(path(tree, &["<orphaned>", "lost-dir", "inside-lost.txt"]).is_some());
    assert_eq!(out.stats.orphan_links, 4);
    assert_eq!(logical(tree, orphans), 10 + 20 + 30 + 40);
    assert_eq!(tree.get(tree.root()).unwrap().error_count(), 1);
}

#[test]
fn parent_cycles_terminate_and_become_orphans() {
    let mut img = sample();
    img.dir(60, 61, "cycle-a");
    img.dir(61, 60, "cycle-b");
    img.small(62, 60, "in-cycle.txt", 5);
    img.dir(63, 63, "self-parent");
    let out = ok(&img);
    let tree = &out.outcome.tree;
    assert!(path(tree, &["<orphaned>", "cycle-a", "cycle-b"]).is_some());
    assert!(path(tree, &["<orphaned>", "cycle-a", "in-cycle.txt"]).is_some());
    assert!(path(tree, &["<orphaned>", "self-parent"]).is_some());
    assert_eq!(out.stats.cycle_entries, 2);
    assert_eq!(out.stats.duplicate_dirs, 2);
    // docs, top-level files, the three cycle dirs and the orphan dir, nothing twice.
    let root = tree.get(tree.root()).unwrap();
    assert_eq!(root.dir_count() + root.file_count(), u64::try_from(tree.len()).unwrap());
}

#[test]
fn deep_chain_does_not_recurse() {
    let mut img = Img::new();
    let depth = 4_000u32;
    for i in 0..depth {
        let number = 100 + i;
        let parent = if i == 0 { 5 } else { number - 1 };
        img.dir(number, parent, "d");
    }
    img.small(100 + depth, 99 + depth, "leaf", 7);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    assert_eq!(tree.len(), usize::try_from(depth).unwrap() + 2);
    assert_eq!(logical(tree, tree.root()), 7);
}

#[test]
fn huge_fan_out_in_one_directory() {
    let mut img = Img::new();
    img.dir(30, 5, "big");
    let n = 20_000u32;
    for i in 0..n {
        img.small(100 + i, 30, &format!("f{i:05}"), 1 + usize::try_from(i % 50).unwrap());
    }
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let dir = path(tree, &["big"]).unwrap();
    assert_eq!(tree.children(dir).count(), usize::try_from(n).unwrap());
    let expected: u64 = (0..u64::from(n)).map(|i| 1 + i % 50).sum();
    assert_eq!(logical(tree, dir), expected);
    assert_eq!(out.stats.entries, u64::from(n) + 2 + 4);
}

#[test]
fn allocated_size_follows_compression_sparseness_and_residency() {
    let mut img = Img::new();
    let data = |flags: u16, size: u64, allocated: u64, compressed: Option<u64>| {
        nonresident(&NonResident {
            flags,
            size,
            allocated,
            initialized: size,
            compression_unit: if compressed.is_some() { 4 } else { 0 },
            compressed_size: compressed,
            runs: &encode_runs(&[Run { lcn: Some(10), len: 1 }]),
            ..NonResident::default()
        })
    };
    img.put(40, record(40, false, 5, "plain").attr(data(0, 10_000, 12_288, None)));
    img.put(41, record(41, false, 5, "compressed").attr(data(0x0001, 100_000, 102_400, Some(32_768))));
    img.put(42, record(42, false, 5, "sparse").attr(data(0x8000, 1 << 30, 1 << 30, Some(8_192))));
    img.put(43, record(43, false, 5, "compressed-nosize").attr(data(0x0001, 100_000, 102_400, None)));
    img.small(44, 5, "resident", 100);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    for (name, logical, alloc) in [
        ("plain", 10_000, 12_288),
        ("compressed", 100_000, 32_768),
        ("sparse", 1 << 30, 8_192),
        // Without a recorded stored size the recorded allocation is the best available figure.
        ("compressed-nosize", 100_000, 102_400),
        ("resident", 100, 0),
    ] {
        let d = *tree.get(find(tree, tree.root(), name).unwrap()).unwrap().data();
        assert_eq!((d.logical_size, d.allocated_size), (logical, alloc), "{name}");
    }
}

#[test]
fn hostile_sizes_saturate_instead_of_wrapping() {
    let mut img = Img::new();
    let max = i64::MAX as u64;
    for i in 0..4u32 {
        img.big(40 + i, 5, &format!("huge{i}"), max, max);
    }
    let out = ok(&img);
    let tree = &out.outcome.tree;
    assert_eq!(logical(tree, tree.root()), u64::MAX);
    assert_eq!(allocated(tree, tree.root()), u64::MAX);
}

#[test]
fn filetime_conversion_is_checked() {
    use super::collect::filetime_to_unix;
    assert_eq!(filetime_to_unix(0), None);
    assert_eq!(filetime_to_unix(116_444_736_000_000_000), Some(0));
    assert_eq!(filetime_to_unix(filetime(SAMPLE_MTIME)), Some(SAMPLE_MTIME));
    // Before 1970 is representable as a negative number, u64::MAX is a valid (far future) value.
    assert_eq!(filetime_to_unix(10_000_000), Some(-11_644_473_599));
    assert!(filetime_to_unix(u64::MAX).is_some());
}

#[test]
fn cancellation_before_start_returns_a_cancelled_root_only_tree() {
    let (src, vol) = sample().finish();
    let cancel = AtomicBool::new(true);
    let out = scan_mft(&src, &vol, "V:", &cancel, &Counters::default()).unwrap();
    assert!(out.outcome.cancelled);
    assert_eq!(out.outcome.tree.len(), 1);
}

#[test]
fn cancellation_during_the_build_keeps_a_valid_partial_tree() {
    let mut img = Img::new();
    for i in 0..10_000u32 {
        img.small(100 + i, 5, &format!("f{i}"), 1);
    }
    let (src, vol) = img.finish();
    let counters = Counters::default();
    let cancel = AtomicBool::new(false);
    let collected = collect::collect(&src, &vol, &MftLimits::default(), &cancel, &counters).unwrap();
    cancel.store(true, Ordering::Relaxed);
    let (tree, cancelled, _) = build::build(collected, "V:", &cancel, &counters).unwrap();
    assert!(cancelled);
    assert!(tree.len() < 10_000, "{}", tree.len());
    let root = tree.get(tree.root()).unwrap();
    assert_eq!(root.dir_count() + root.file_count(), u64::try_from(tree.len()).unwrap());
}

/// A source that raises the cancel flag on its second read (the first record chunk).
struct CancelOnChunk<'a> {
    inner: &'a MemSource,
    cancel: &'a AtomicBool,
    reads: std::cell::Cell<u32>,
}

impl BlockSource for CancelOnChunk<'_> {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        self.reads.set(self.reads.get() + 1);
        if self.reads.get() >= 2 {
            self.cancel.store(true, Ordering::Relaxed);
        }
        self.inner.read_at(offset, len)
    }
}

#[test]
fn cancellation_while_reading_stops_collection() {
    let (src, vol) = sample().finish();
    let cancel = AtomicBool::new(false);
    let source = CancelOnChunk { inner: &src, cancel: &cancel, reads: std::cell::Cell::new(0) };
    let out = scan_mft(source, &vol, "V:", &cancel, &Counters::default()).unwrap();
    assert!(out.outcome.cancelled);
}

#[test]
fn progress_counters_match_the_final_tree() {
    let (src, vol) = sample().finish();
    let counters = Counters::default();
    let out = scan_mft(&src, &vol, "V:", &AtomicBool::new(false), &counters).unwrap();
    let root = out.outcome.tree.get(out.outcome.tree.root()).unwrap();
    assert_eq!(counters.dirs.load(Ordering::Relaxed), root.dir_count());
    assert_eq!(counters.files.load(Ordering::Relaxed), root.file_count());
    assert_eq!(counters.bytes.load(Ordering::Relaxed), 15_103);
    assert_eq!(counters.errors.load(Ordering::Relaxed), 0);
}

// ---- failures ----------------------------------------------------------------------------------

#[test]
fn corrupt_record_zero_is_a_typed_error() {
    let (mut src, vol) = sample().finish();
    src.0[record_at(0) + 510] ^= 0xFF;
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::BadFixup)));
    let (mut src, vol) = sample().finish();
    src.0[record_at(0)..record_at(0) + 4].copy_from_slice(b"BAAD");
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::BadMagic)));
}

#[test]
fn incomplete_mft_extents_are_reported() {
    // The run list covers one cluster but the $MFT claims 1 MiB: the rest lives in extension records.
    let (mut src, vol) = sample().finish();
    let rec0 = Img::record0(1 << 20, 1 << 20, &[Run { lcn: Some(1), len: 1 }]);
    src.0[record_at(0)..record_at(0) + rec0.len()].copy_from_slice(&rec0);
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::Incomplete)));
    // Resident $MFT data cannot describe extents either.
    let (mut src, vol) = sample().finish();
    let mut b = record(0, false, 5, "$MFT");
    b = with_resident(b, 10);
    b.number = Some(0);
    let rec0 = b.build();
    src.0[record_at(0)..record_at(0) + rec0.len()].copy_from_slice(&rec0);
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::Incomplete)));
}

#[test]
fn record_zero_that_is_not_the_mft_is_rejected() {
    let (mut src, vol) = sample().finish();
    let mut b = record(0, false, 5, "$MFT");
    b.number = Some(9);
    let rec0 = with_data(b, 1 << 20, 1 << 20).build();
    src.0[record_at(0)..record_at(0) + rec0.len()].copy_from_slice(&rec0);
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::BadHeader)));
}

#[test]
fn missing_or_non_directory_root_is_an_error() {
    let mut img = sample();
    img.records.remove(&5);
    assert_eq!(run(&img).err(), Some(MftError::NoRoot));
    let mut img = sample();
    img.put(5, record(5, false, 5, "."));
    assert_eq!(run(&img).err(), Some(MftError::NoRoot));
    let mut img = sample();
    let mut deleted = record(5, true, 5, ".");
    deleted.flags = 2;
    img.put(5, deleted);
    assert_eq!(run(&img).err(), Some(MftError::NoRoot));
}

#[test]
fn wholesale_corruption_is_rejected_but_a_few_torn_records_are_not() {
    let mut img = Img::new();
    for i in 0..40u32 {
        img.small(100 + i, 5, &format!("f{i}"), 1);
    }
    let (src, vol) = img.finish();
    let mut few = src.clone();
    for n in [100usize, 101] {
        few.0[record_at(n) + 510] ^= 0xFF;
    }
    let out = scan_image(&few, &vol).unwrap();
    assert_eq!(out.stats.corrupt, 2);
    assert_eq!(out.outcome.tree.len(), 1 + 38);
    let mut many = src.clone();
    for n in 100..140usize {
        many.0[record_at(n) + 510] ^= 0xFF;
    }
    assert!(matches!(scan_image(&many, &vol).err(), Some(MftError::TooManyCorrupt { corrupt: 40, .. })));
}

#[test]
fn a_record_in_the_wrong_slot_counts_as_corrupt() {
    let mut img = sample();
    let mut moved = record(34, false, 5, "moved.txt");
    moved.number = Some(777);
    // Bypass `put`, which would rewrite the number.
    img.records.insert(34, moved.build());
    let out = ok(&img);
    assert_eq!(out.stats.corrupt, 1);
    assert!(path(&out.outcome.tree, &["moved.txt"]).is_none());
}

#[test]
fn truncated_image_is_an_io_error_not_a_panic() {
    let (src, vol) = sample().finish();
    let cut = MemSource(src.0[..record_at(0) + 3 * RECORD].to_vec());
    assert_eq!(scan_image(&cut, &vol).err(), Some(MftError::Volume(VolumeError::ShortRead)));
}

#[test]
fn hostile_geometry_is_rejected() {
    let (src, vol) = sample().finish();
    for mutate in [
        (|v: &mut NtfsVolumeData| v.bytes_per_cluster = 0) as fn(&mut NtfsVolumeData),
        |v| v.bytes_per_file_record = 100,
        |v| v.bytes_per_sector = 0,
        |v| v.mft_start_lcn = u64::MAX,
        |v| v.mft_start_lcn = 1 << 40,
        |v| v.mft_valid_data_length = 0,
    ] {
        let mut v = vol;
        mutate(&mut v);
        // Either a typed error or (for a zero valid length, which means "unknown") a normal scan.
        let _ = scan_image(&src, &v);
    }
    let mut v = vol;
    v.bytes_per_cluster = 0;
    assert_eq!(scan_image(&src, &v).err(), Some(MftError::Parse(ParseError::BadGeometry)));
    let mut v = vol;
    v.mft_start_lcn = u64::MAX;
    assert_eq!(scan_image(&src, &v).err(), Some(MftError::Volume(VolumeError::Malformed)));
}

#[test]
fn limits_are_enforced() {
    let (src, vol) = sample().finish();
    let run_with = |limits: MftLimits| {
        scan_mft_with(&src, &vol, "V:", &limits, &AtomicBool::new(false), &Counters::default()).err()
    };
    assert_eq!(run_with(MftLimits { max_entries: 2, ..MftLimits::default() }), Some(MftError::TooLarge("record")));
    assert_eq!(run_with(MftLimits { max_links: 1, ..MftLimits::default() }), Some(MftError::TooLarge("link")));
    assert_eq!(
        run_with(MftLimits { max_name_units: 3, ..MftLimits::default() }),
        Some(MftError::TooLarge("name pool"))
    );
    // A per-record link cap drops names but keeps the scan.
    let mut img = Img::new();
    let mut multi = record(40, false, 5, "n0");
    for i in 1..6 {
        multi.attrs.push(file_name(5, SEQ, WIN32, &utf16(&format!("n{i}")), 0, 0));
    }
    img.put(40, multi);
    let (src, vol) = img.finish();
    let limits = MftLimits { max_links_per_record: 2, ..MftLimits::default() };
    let out = scan_mft_with(&src, &vol, "V:", &limits, &AtomicBool::new(false), &Counters::default()).unwrap();
    assert_eq!((out.stats.links, out.stats.links_dropped), (2 + 4, 4));
}

// ---- random damage -----------------------------------------------------------------------------

fn damaged_image_len() -> usize {
    sample().finish().0.0.len()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn random_byte_damage_never_panics(
        flips in proptest::collection::vec((0..damaged_image_len(), any::<u8>()), 0..200)
    ) {
        let (mut src, vol) = sample().finish();
        for (at, value) in flips {
            src.0[at] = value;
        }
        if let Ok(out) = scan_image(&src, &vol) {
            let tree = &out.outcome.tree;
            let root = tree.get(tree.root()).unwrap();
            prop_assert_eq!(root.dir_count() + root.file_count(), u64::try_from(tree.len()).unwrap());
        }
    }

    #[test]
    fn damaged_record_structure_never_panics(
        records in proptest::collection::vec(0usize..40, 1..12),
        offsets in proptest::collection::vec(0usize..RECORD, 1..12),
        values in proptest::collection::vec(any::<u8>(), 1..12),
    ) {
        let mut img = sample();
        for i in 0..8u32 {
            img.small(40 + i, 30, &format!("x{i}"), 5);
        }
        let (mut src, vol) = img.finish();
        for ((r, o), v) in records.iter().zip(&offsets).zip(&values) {
            src.0[record_at(*r) + *o] = *v;
        }
        if let Ok(out) = scan_image(&src, &vol) {
            let tree = &out.outcome.tree;
            let root = tree.get(tree.root()).unwrap();
            prop_assert_eq!(root.dir_count() + root.file_count(), u64::try_from(tree.len()).unwrap());
        }
    }

    #[test]
    fn arbitrary_mft_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let (mut src, vol) = sample().finish();
        let start = record_at(1);
        for (i, b) in bytes.iter().enumerate() {
            src.0[start + i] = *b;
        }
        let _ = scan_image(&src, &vol);
    }
}

// ---- review fixes -------------------------------------------------------------------------------

const TAG_SYMLINK: u32 = 0xA000_000C;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
const TAG_CLOUD: u32 = 0x9000_001A;

fn reparse(
    number: u32,
    dir: bool,
    parent: u32,
    name: &str,
    tag: Option<u32>,
) -> dirustat_ntfs::testutil::RecordBuilder {
    let mut b = record(number, dir, parent, name);
    b.attrs[0] = standard_info(filetime(SAMPLE_MTIME), if dir { 0x410 } else { 0x420 });
    if let Some(tag) = tag {
        let mut value = tag.to_le_bytes().to_vec();
        value.extend_from_slice(&[0; 4]);
        b.attrs.push(dirustat_ntfs::testutil::resident(0xC0, &[], 0, &value));
    }
    b
}

#[test]
fn only_symlinks_and_junctions_are_links_and_nothing_is_followed() {
    let mut img = Img::new();
    img.put(41, reparse(41, true, 5, "junction", Some(TAG_MOUNT_POINT)));
    img.small(42, 41, "behind-the-junction.txt", 9);
    img.put(43, with_data(reparse(43, false, 5, "symlink.lnk", Some(TAG_SYMLINK)), 20, 4_096));
    // A cloud placeholder file keeps its content: it is a regular file.
    img.put(44, with_data(reparse(44, false, 5, "cloud.docx", Some(TAG_CLOUD)), 5_000, 8_192));
    // A reparse file without a readable tag is a regular file too.
    img.put(45, with_data(reparse(45, false, 5, "untagged.bin", None), 7, 4_096));
    // Reparse directories with an unknown or missing tag are never descended into.
    img.put(46, reparse(46, true, 5, "cloud-dir", Some(TAG_CLOUD)));
    img.put(47, reparse(47, true, 5, "untagged-dir", None));
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let kind = |name: &str| tree.get(find(tree, tree.root(), name).unwrap()).unwrap().kind();
    assert_eq!(kind("junction"), NodeKind::Link);
    assert_eq!(kind("symlink.lnk"), NodeKind::Link);
    assert_eq!(kind("cloud.docx"), NodeKind::File);
    assert_eq!(kind("untagged.bin"), NodeKind::File);
    assert_eq!(kind("cloud-dir"), NodeKind::Link);
    assert_eq!(kind("untagged-dir"), NodeKind::Link);
    assert_eq!(tree.children(find(tree, tree.root(), "junction").unwrap()).count(), 0);
    let d = *tree.get(find(tree, tree.root(), "cloud.docx").unwrap()).unwrap().data();
    assert_eq!((d.logical_size, d.allocated_size), (5_000, 8_192));
    // The child of the junction cannot be placed: it is an orphan, not a hidden descendant.
    assert!(path(tree, &["<orphaned>", "behind-the-junction.txt"]).is_some());
}

#[test]
fn data_continuation_pieces_do_not_stand_in_for_the_file() {
    let mut img = Img::new();
    let piece = nonresident(&NonResident {
        start_vcn: 64,
        size: 1 << 20,
        allocated: 1 << 20,
        initialized: 1 << 20,
        runs: &encode_runs(&[Run { lcn: Some(10), len: 4 }]),
        ..NonResident::default()
    });
    let mut b = record(40, false, 5, "fragmented.bin").attr(piece);
    b.attrs.push(dirustat_ntfs::testutil::resident(
        0x20,
        &[],
        0,
        &dirustat_ntfs::testutil::list_entry(0x80, false, 0, 99),
    ));
    img.put(40, b);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    let node = tree.get(path(tree, &["fragmented.bin"]).unwrap()).unwrap();
    assert!(node.data().error);
    assert_eq!(node.data().logical_size, 0);
    assert_eq!(out.stats.undercounted_files, 1);
}

#[test]
fn hostile_names_cannot_traverse_a_path() {
    let mut img = Img::new();
    img.dir(30, 5, "..\u{5c}..\u{5c}Windows");
    img.small(31, 30, "a/b", 1);
    img.small(32, 5, "..", 1);
    img.small(33, 5, ".", 1);
    img.small(34, 5, "C:evil", 1);
    img.small(35, 5, "nul\0char\u{1}", 1);
    img.small(36, 5, "fine.txt", 1);
    let out = ok(&img);
    let tree = &out.outcome.tree;
    // Every node name is a single safe path component.
    let mut stack = vec![(tree.root(), String::new())];
    let mut seen = 0;
    while let Some((id, prefix)) = stack.pop() {
        for child in tree.children(id) {
            let node = tree.get(child).unwrap();
            let name = node.name();
            assert!(!name.is_empty() && name != "." && name != "..", "{name:?}");
            assert!(!name.contains(['/', '\u{5c}', ':']) && !name.chars().any(char::is_control), "{name:?}");
            let joined = format!("{prefix}/{name}");
            assert!(
                std::path::Path::new(&joined).components().all(|c| !matches!(c, std::path::Component::ParentDir)),
                "{joined}"
            );
            seen += 1;
            stack.push((child, joined));
        }
    }
    assert_eq!(seen, 6 + 1);
    assert_eq!(out.stats.names_sanitized, 6);
    let d = *tree.get(find(tree, tree.root(), "fine.txt").unwrap()).unwrap().data();
    assert!(!d.name_lossy);
    let d = *tree.get(find(tree, tree.root(), "_..").unwrap()).unwrap().data();
    assert!(d.name_lossy);
}

#[test]
fn allocation_caps_follow_the_mft_size() {
    let caps = collect::Caps::derive(&MftLimits::default(), 10 * 1024, 1024);
    assert_eq!((caps.entries, caps.links), (10, 80));
    assert_eq!(caps.name_units, 80 * 255);
    // The fixed limits still win for a forged, enormous MFT size.
    let caps = collect::Caps::derive(&MftLimits::default(), u64::MAX, 1024);
    assert_eq!(caps.entries, MftLimits::default().max_entries);
    assert_eq!(caps.links, MftLimits::default().max_links);
    assert_eq!(caps.name_units, MftLimits::default().max_name_units);
    assert_eq!(collect::Caps::derive(&MftLimits::default(), 0, 0).entries, 0);
}

#[test]
fn forged_extents_larger_than_the_volume_are_rejected() {
    let (src, mut vol) = sample().finish();
    // The volume claims to end before the MFT does.
    vol.total_bytes = 2 * u64::try_from(CLUSTER).unwrap();
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::ExtentOutOfRange)));
    // A record 0 whose run list claims far more clusters than the volume has.
    let (mut src, vol) = sample().finish();
    let rec0 = Img::record0(1 << 20, 1 << 20, &[Run { lcn: Some(1), len: 1 << 20 }]);
    src.0[record_at(0)..record_at(0) + rec0.len()].copy_from_slice(&rec0);
    assert_eq!(scan_image(&src, &vol).err(), Some(MftError::Parse(ParseError::ExtentOutOfRange)));
}

/// A source that can hand out independent copies, like a volume with several handles, so the parallel
/// reading path is exercised.
#[derive(Clone)]
struct Shared(std::sync::Arc<Vec<u8>>);

impl BlockSource for Shared {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        let start = usize::try_from(offset).map_err(|_| VolumeError::InvalidRange)?;
        self.0
            .get(start..start.checked_add(len).ok_or(VolumeError::InvalidRange)?)
            .map(<[u8]>::to_vec)
            .ok_or(VolumeError::ShortRead)
    }

    fn duplicate(&self) -> Option<Self> {
        Some(self.clone())
    }
}

fn big_image() -> Img {
    let mut img = Img::new();
    for d in 0..40u32 {
        img.dir(100 + d, 5, &format!("d{d}"));
    }
    // About 30 MiB of records: several read chunks, hundreds of parse blocks.
    for i in 0..30_000u32 {
        img.small(200 + i, 100 + i % 40, &format!("file{i}"), usize::try_from(i % 97).unwrap());
    }
    img
}

#[test]
fn parallel_reading_and_parsing_gives_the_same_tree_as_one_reader() {
    let img = big_image();
    let (src, vol) = img.finish();
    let single = scan_image(&src, &vol).unwrap();
    let shared =
        scan_mft(Shared(std::sync::Arc::new(src.0.clone())), &vol, "V:", &AtomicBool::new(false), &Counters::default())
            .unwrap();
    assert_eq!(dump(&single.outcome.tree), dump(&shared.outcome.tree));
    assert_eq!(single.stats.entries, shared.stats.entries);
    assert_eq!(single.stats.records_seen, shared.stats.records_seen);
    assert_eq!(single.stats.timings.read_threads, 1);
    assert_eq!(shared.stats.timings.read_threads, 4);
    assert!(shared.stats.timings.read_calls >= 3, "{:?}", shared.stats.timings);
}

#[test]
fn timings_are_filled_in() {
    let out = ok(&big_image());
    let t = out.stats.timings;
    assert!(t.read_calls >= 1 && t.bytes_read > 0, "{t:?}");
    assert!(t.total_nanos > 0 && t.collect_nanos > 0 && t.build_nanos > 0, "{t:?}");
    assert!(t.parse_nanos > 0 && t.resolve_nanos > 0 && t.nodes_nanos > 0, "{t:?}");
    assert!(t.collect_nanos <= t.total_nanos && t.build_nanos <= t.total_nanos, "{t:?}");
}

#[test]
fn a_cancelled_parallel_scan_returns_a_partial_tree() {
    let (src, vol) = big_image().finish();
    let cancel = AtomicBool::new(true);
    let out = scan_mft(Shared(std::sync::Arc::new(src.0)), &vol, "V:", &cancel, &Counters::default()).unwrap();
    assert!(out.outcome.cancelled);
    assert_eq!(out.outcome.tree.len(), 1);
}
