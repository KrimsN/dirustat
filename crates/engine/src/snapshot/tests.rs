use super::compare::{self, ChangeKind, Options};
use super::format::{self, DEFAULT_LOAD_NODES, MAGIC, SnapshotError};
use dirustat_model::{NodeData, NodeKind, SizeMode, Tree};
use std::io::Cursor;

fn data(size: u64) -> NodeData {
    NodeData::with_size(size)
}

/// Root `R` with `docs/{a.txt 100, b.txt 200}`, `media/c.bin 1000` and `top.txt 50`.
pub(crate) fn sample() -> Tree {
    let mut t = Tree::new("R");
    let docs = t.add_child(t.root(), "docs", NodeKind::Dir, 0).unwrap();
    t.add_node(docs, "a.txt", NodeKind::File, NodeData { mtime: Some(-5), attributes: 0x20, ..data(100) }).unwrap();
    t.add_node(docs, "b\u{fffd}.txt", NodeKind::File, NodeData { name_lossy: true, ..data(200) }).unwrap();
    let media = t.add_child(t.root(), "media", NodeKind::Dir, 0).unwrap();
    t.add_node(media, "c.bin", NodeKind::File, NodeData { logical_size: 1000, allocated_size: 4096, ..data(0) })
        .unwrap();
    t.add_node(t.root(), "top.txt", NodeKind::File, NodeData { error: true, mtime: Some(i64::MAX), ..data(50) })
        .unwrap();
    t.add_child(t.root(), "lnk", NodeKind::Link, 0).unwrap();
    t.aggregate();
    t
}

/// Everything a round trip must preserve, in arena order.
pub(crate) fn dump(t: &Tree) -> Vec<String> {
    (0..t.len())
        .map(|i| {
            let id = t.node_at(i).unwrap();
            let n = t.get(id).unwrap();
            format!(
                "{}|{:?}|{:?}|{:?}|{:?}|{}|{}|{}",
                n.name(),
                n.kind(),
                n.data(),
                n.parent().map(|p| p.index()),
                t.children(id).map(|c| c.index()).collect::<Vec<_>>(),
                n.total_size(SizeMode::Logical),
                n.total_size(SizeMode::Allocated),
                n.file_count()
            )
        })
        .collect()
}

pub(crate) fn encode(t: &Tree, partial: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let n = format::write(&mut out, t, partial, 1_700_000_000).unwrap();
    assert_eq!(n, out.len() as u64);
    out
}

fn decode(bytes: &[u8]) -> Result<format::Loaded, SnapshotError> {
    format::read(Cursor::new(bytes), bytes.len() as u64, DEFAULT_LOAD_NODES)
}

#[test]
fn round_trip_preserves_every_node() {
    let t = sample();
    let bytes = encode(&t, true);
    assert!(bytes.starts_with(MAGIC));
    let loaded = decode(&bytes).unwrap();
    assert_eq!(dump(&loaded.tree), dump(&t));
    assert_eq!((loaded.meta.created, loaded.meta.partial, loaded.meta.nodes), (1_700_000_000, true, 8));
    assert!(!decode(&encode(&t, false)).unwrap().meta.partial);
}

#[test]
fn round_trip_of_root_data_and_trivial_tree() {
    let mut t = Tree::new("only-root");
    let root = t.root();
    t.set_node_data(root, NodeData { logical_size: 9, mtime: Some(3), ..NodeData::default() }).unwrap();
    t.aggregate();
    let loaded = decode(&encode(&t, false)).unwrap();
    assert_eq!(dump(&loaded.tree), dump(&t));
}

#[test]
fn bad_magic_and_version_are_named() {
    let bytes = encode(&sample(), false);
    let mut bad = bytes.clone();
    bad[0] ^= 0xff;
    assert!(matches!(decode(&bad), Err(SnapshotError::BadMagic)));
    let mut newer = bytes;
    newer[8] = 2;
    assert!(matches!(decode(&newer), Err(SnapshotError::Version(2))));
}

#[test]
fn checksum_catches_a_flipped_content_byte() {
    let mut bytes = encode(&sample(), false);
    // Last byte before the checksum: the tail of the last record, still a valid number.
    let at = bytes.len() - 33;
    bytes[at] ^= 0x01;
    assert!(matches!(decode(&bytes), Err(SnapshotError::Checksum)), "{:?}", decode(&bytes).err());
    let mut bytes = encode(&sample(), false);
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80;
    assert!(matches!(decode(&bytes), Err(SnapshotError::Checksum)));
}

#[test]
fn every_truncation_and_every_bit_flip_is_an_error() {
    let bytes = encode(&sample(), true);
    for len in 0..bytes.len() {
        let cut = &bytes[..len];
        assert!(decode(cut).is_err(), "truncated to {len}");
    }
    for at in 0..bytes.len() {
        let mut copy = bytes.clone();
        copy[at] ^= 0xff;
        assert!(decode(&copy).is_err(), "flipped byte {at}");
    }
    let mut extra = bytes;
    extra.push(0);
    assert!(decode(&extra).is_err(), "trailing byte");
}

/// A file with a valid header and checksum around the given record bytes.
fn forge(count: u64, records: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&1_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_i64.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(records);
    let digest = blake3::hash(&out);
    out.extend_from_slice(digest.as_bytes());
    out
}

fn rec(parent: Option<&[u8]>, tag: u8, name: &str, sizes: [u8; 3]) -> Vec<u8> {
    let mut r = Vec::new();
    if let Some(p) = parent {
        r.extend_from_slice(p);
    }
    r.push(tag);
    r.push(u8::try_from(name.len()).unwrap());
    r.extend_from_slice(name.as_bytes());
    r.extend_from_slice(&sizes);
    r
}

#[test]
fn forged_files_with_valid_checksums_are_still_checked() {
    let root = rec(None, 0, "R", [0, 0, 0]);
    let file = |parent: u8, name: &str| rec(Some(&[parent]), 1, name, [1, 1, 0]);
    let join = |parts: &[Vec<u8>]| parts.concat();
    // Control: a good two-node file decodes.
    let good = forge(2, &join(&[root.clone(), file(0, "f")]));
    assert_eq!(decode(&good).unwrap().tree.len(), 2);

    let bad: Vec<(&str, Vec<u8>)> = vec![
        ("parent is itself", forge(2, &join(&[root.clone(), file(1, "f")]))),
        ("parent out of range", forge(2, &join(&[root.clone(), file(9, "f")]))),
        ("parent is a file", forge(3, &join(&[root.clone(), file(0, "f"), file(1, "g")]))),
        ("huge parent varint", forge(2, &join(&[root.clone(), rec(Some(&[0xff; 9]), 1, "f", [0; 3])]))),
        ("overlong varint", forge(2, &join(&[root.clone(), rec(Some(&[0x80; 11]), 1, "f", [0; 3])]))),
        ("count larger than the file", forge(1_000_000, &join(&[root.clone(), file(0, "f")]))),
        ("count zero", forge(0, &[])),
        ("root is a file", forge(2, &join(&[rec(None, 1, "R", [0; 3]), file(0, "f")]))),
        ("reserved tag bits", forge(2, &join(&[root.clone(), rec(Some(&[0]), 0x21, "f", [0; 3])]))),
        ("kind 3", forge(2, &join(&[root.clone(), rec(Some(&[0]), 3, "f", [0; 3])]))),
        ("separator in name", forge(2, &join(&[root.clone(), file(0, "a/b")]))),
        ("backslash in name", forge(2, &join(&[root.clone(), file(0, "a\\b")]))),
        ("empty name", forge(2, &join(&[root.clone(), file(0, "")]))),
        ("NUL in name", forge(2, &join(&[root.clone(), file(0, "a\0b")]))),
        ("invalid UTF-8", {
            let mut r = root.clone();
            r.extend_from_slice(&[0, 1, 1, 0xff, 0, 0, 0]);
            forge(2, &r)
        }),
        ("name longer than the data", forge(2, &join(&[root.clone(), vec![0, 1, 200, b'x']]))),
        ("fewer records than the count", forge(3, &join(&[root.clone(), file(0, "f")]))),
        ("attributes beyond u32", {
            let mut r = root.clone();
            r.extend_from_slice(&[0, 1, 1, b'f', 0, 0, 0x80, 0x80, 0x80, 0x80, 0x10]);
            forge(2, &r)
        }),
    ];
    for (what, bytes) in &bad {
        assert!(decode(bytes).is_err(), "{what} must be rejected");
    }
}

#[test]
fn dot_entries_and_colon_names_are_rejected() {
    let root = rec(None, 0, "R", [0, 0, 0]);
    let child = |name: &str| forge(2, &[root.clone(), rec(Some(&[0]), 1, name, [1, 1, 0])].concat());
    for name in [".", ".."] {
        assert!(decode(&child(name)).is_err(), "{name:?} must be rejected");
    }
    assert!(decode(&child("...")).is_ok(), "three dots is an ordinary name");
    if cfg!(windows) {
        for name in ["C:evil", "a:b", "x:stream"] {
            assert!(decode(&child(name)).is_err(), "{name:?} must be rejected on Windows");
        }
    }
}

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let low = u8::try_from(v & 0x7f).unwrap();
        v >>= 7;
        if v == 0 {
            out.push(low);
            return out;
        }
        out.push(low | 0x80);
    }
}

/// A chain of `depth` nested directories below the root.
fn chain(depth: u64) -> Vec<u8> {
    let mut records = rec(None, 0, "R", [0, 0, 0]);
    for i in 1..=depth {
        records.extend(rec(Some(&varint(i - 1)), 0, "d", [0, 0, 0]));
    }
    forge(depth + 1, &records)
}

#[test]
fn nesting_beyond_the_depth_cap_is_refused() {
    let max = u64::from(format::MAX_DEPTH);
    assert!(decode(&chain(max)).is_ok());
    assert!(matches!(decode(&chain(max + 1)), Err(SnapshotError::TooLarge(_))));
}

#[test]
fn limits_apply_before_allocation() {
    let bytes = encode(&sample(), false);
    let len = bytes.len() as u64;
    assert!(matches!(format::read(Cursor::new(&bytes), len, 3), Err(SnapshotError::TooLarge(_))));
    assert!(format::read(Cursor::new(&bytes), len, 8).is_ok());
    assert!(matches!(
        format::read(Cursor::new(&bytes), format::MAX_FILE_BYTES + 1, 8),
        Err(SnapshotError::TooLarge(_))
    ));
    assert!(format::read(Cursor::new(&bytes), 10, 7).is_err());
}

#[test]
fn deep_chain_and_wide_tree_round_trip() {
    let mut t = Tree::new("deep");
    let mut at = t.root();
    for i in 0..4000 {
        at = t.add_child(at, &format!("d{i}"), NodeKind::Dir, 0).unwrap();
    }
    for i in 0..2000 {
        t.add_child(at, &format!("f{i}"), NodeKind::File, i).unwrap();
    }
    t.aggregate();
    let loaded = decode(&encode(&t, false)).unwrap();
    assert_eq!(loaded.tree.len(), t.len());
    assert_eq!(dump(&loaded.tree), dump(&t));
}

// ------------------------------------------------------------------------------ compare

fn opts(mode: SizeMode) -> Options {
    Options { mode, limit: 100, prefix: Vec::new(), include_dirs: false, min_delta: 0, kinds: ChangeKind::ALL.to_vec() }
}

/// `sample()` with a.txt grown to 150, b.txt gone, c.bin shrunk to 400 and a new dir `new/{x 70, y 30}`.
fn changed() -> Tree {
    let mut t = Tree::new("R2");
    let docs = t.add_child(t.root(), "docs", NodeKind::Dir, 0).unwrap();
    t.add_child(docs, "a.txt", NodeKind::File, 150).unwrap();
    let media = t.add_child(t.root(), "media", NodeKind::Dir, 0).unwrap();
    t.add_child(media, "c.bin", NodeKind::File, 400).unwrap();
    t.add_child(t.root(), "top.txt", NodeKind::File, 50).unwrap();
    t.add_child(t.root(), "lnk", NodeKind::Link, 0).unwrap();
    let new = t.add_child(t.root(), "new", NodeKind::Dir, 0).unwrap();
    t.add_child(new, "x", NodeKind::File, 70).unwrap();
    t.add_child(new, "y", NodeKind::File, 30).unwrap();
    t.aggregate();
    t
}

/// The sample with `c.bin` as 1000 logical bytes in both sizes, so logical and allocated agree.
fn base_plain() -> Tree {
    let mut t = Tree::new("R");
    let docs = t.add_child(t.root(), "docs", NodeKind::Dir, 0).unwrap();
    t.add_child(docs, "a.txt", NodeKind::File, 100).unwrap();
    t.add_child(docs, "b.txt", NodeKind::File, 200).unwrap();
    let media = t.add_child(t.root(), "media", NodeKind::Dir, 0).unwrap();
    t.add_child(media, "c.bin", NodeKind::File, 1000).unwrap();
    t.add_child(t.root(), "top.txt", NodeKind::File, 50).unwrap();
    t.add_child(t.root(), "lnk", NodeKind::Link, 0).unwrap();
    t.aggregate();
    t
}

fn summary(r: &compare::Report) -> Vec<(ChangeKind, String, i64)> {
    r.changes.iter().map(|c| (c.kind, c.path.clone(), c.delta)).collect()
}

#[test]
fn compare_reports_added_removed_grown_shrunk() {
    let r = compare::compare(&base_plain(), &changed(), &opts(SizeMode::Logical));
    assert_eq!(
        summary(&r),
        vec![
            (ChangeKind::Shrunk, "media/c.bin".to_owned(), -600),
            (ChangeKind::Removed, "docs/b.txt".to_owned(), -200),
            (ChangeKind::Added, "new".to_owned(), 100),
            (ChangeKind::Grown, "docs/a.txt".to_owned(), 50),
        ]
    );
    let c = r.counts;
    assert_eq!((c.added, c.removed, c.grown, c.shrunk), (1, 1, 1, 1));
    assert_eq!((c.added_bytes, c.removed_bytes, c.grown_bytes, c.shrunk_bytes), (100, 200, 50, 600));
    assert_eq!(c.unchanged, 2, "top.txt and lnk");
    assert_eq!((r.base_total, r.other_total), (1350, 700));
    assert!(!r.identical);
    let added = r.changes.iter().find(|c| c.kind == ChangeKind::Added).unwrap();
    assert!(added.old_id.is_none() && added.new_id.is_some() && added.node_kind == NodeKind::Dir);
    let removed = r.changes.iter().find(|c| c.kind == ChangeKind::Removed).unwrap();
    assert!(removed.new_id.is_none() && removed.old_id.is_some());
    // Swapping the sides swaps added and removed and negates the deltas.
    let back = compare::compare(&changed(), &base_plain(), &opts(SizeMode::Logical));
    assert_eq!((back.counts.added, back.counts.removed), (1, 1));
    assert_eq!(back.changes.iter().map(|c| c.delta).sum::<i64>(), 650);
}

#[test]
fn identical_trees_have_no_changes() {
    let t = base_plain();
    let r = compare::compare(&t, &t.clone(), &opts(SizeMode::Allocated));
    assert!(r.identical && r.changes.is_empty() && r.matching == 0);
    assert_eq!(r.counts.unchanged, 5);
    assert_eq!(r.base_total, r.other_total);
    // Root names do not matter, only the structure below.
    let mut renamed = base_plain();
    renamed.set_node_name(renamed.root(), "elsewhere").unwrap();
    assert!(compare::compare(&t, &renamed, &opts(SizeMode::Logical)).identical);
}

#[test]
fn compare_limit_prefix_kinds_and_dirs() {
    let (a, b) = (base_plain(), changed());
    let mut o = opts(SizeMode::Logical);
    o.limit = 2;
    let r = compare::compare(&a, &b, &o);
    assert_eq!((r.changes.len(), r.matching), (2, 4));
    assert_eq!(r.changes[0].path, "media/c.bin");
    assert_eq!(r.counts.grown, 1, "counts cover everything, not just the list");

    let mut o = opts(SizeMode::Logical);
    o.prefix = vec!["docs".to_owned()];
    let r = compare::compare(&a, &b, &o);
    assert_eq!(
        summary(&r),
        vec![(ChangeKind::Removed, "docs/b.txt".to_owned(), -200), (ChangeKind::Grown, "docs/a.txt".to_owned(), 50),]
    );
    assert_eq!((r.base_total, r.other_total), (300, 150));

    // A prefix that exists on one side only: the whole subtree is added.
    let mut o = opts(SizeMode::Logical);
    o.prefix = vec!["new".to_owned()];
    let r = compare::compare(&a, &b, &o);
    assert_eq!(summary(&r), vec![(ChangeKind::Added, "new".to_owned(), 100)]);
    o.prefix = vec!["nope".to_owned()];
    assert!(compare::compare(&a, &b, &o).identical);

    let mut o = opts(SizeMode::Logical);
    o.kinds = vec![ChangeKind::Grown];
    let r = compare::compare(&a, &b, &o);
    assert_eq!((r.changes.len(), r.counts.shrunk), (1, 1));

    let mut o = opts(SizeMode::Logical);
    o.min_delta = 100;
    assert_eq!(compare::compare(&a, &b, &o).changes.len(), 3);

    let mut o = opts(SizeMode::Logical);
    o.include_dirs = true;
    let r = compare::compare(&a, &b, &o);
    assert_eq!(r.counts.changed_dirs, 3, "R, docs and media");
    assert!(r.changes.iter().any(|c| c.node_kind == NodeKind::Dir && c.path == "docs" && c.delta == -150));
    assert_eq!((r.counts.grown, r.counts.shrunk), (1, 1), "directory lines are not in the counts");
}

#[test]
fn a_file_replaced_by_a_directory_is_removed_plus_added() {
    let mut a = Tree::new("R");
    a.add_child(a.root(), "x", NodeKind::File, 10).unwrap();
    a.aggregate();
    let mut b = Tree::new("R");
    let x = b.add_child(b.root(), "x", NodeKind::Dir, 0).unwrap();
    b.add_child(x, "inner", NodeKind::File, 4).unwrap();
    b.aggregate();
    let r = compare::compare(&a, &b, &opts(SizeMode::Logical));
    assert_eq!((r.counts.added, r.counts.removed), (1, 1));
}

#[test]
fn duplicate_sibling_names_pair_in_order() {
    let mut a = Tree::new("R");
    a.add_child(a.root(), "d", NodeKind::File, 5).unwrap();
    a.add_child(a.root(), "d", NodeKind::File, 7).unwrap();
    a.aggregate();
    let mut b = Tree::new("R");
    b.add_child(b.root(), "d", NodeKind::File, 5).unwrap();
    b.aggregate();
    let r = compare::compare(&a, &b, &opts(SizeMode::Logical));
    assert_eq!((r.counts.removed, r.counts.added, r.counts.unchanged), (1, 0, 1));
}

/// `depth` nested directories named `name`, with one file of `size` bytes at the bottom.
fn deep(name: &str, depth: usize, size: u64) -> Tree {
    let mut t = Tree::new("R");
    let mut at = t.root();
    for _ in 0..depth {
        at = t.add_child(at, name, NodeKind::Dir, 0).unwrap();
    }
    t.add_child(at, "f", NodeKind::File, size).unwrap();
    t.aggregate();
    t
}

#[test]
fn deep_chain_compare_is_fast_and_bounded() {
    let depth = usize::from(format::MAX_DEPTH) - 1;
    let long = "n".repeat(200);
    let (a, b) = (deep(&long, depth, 1), deep(&long, depth, 1000));
    let mut o = opts(SizeMode::Logical);
    o.limit = 10;
    let start = std::time::Instant::now();
    let r = compare::compare(&a, &b, &o);
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "took {:?}", start.elapsed());
    assert_eq!((r.changes.len(), r.matching), (1, 1));
    for ch in &r.changes {
        assert!(ch.path.len() <= compare::MAX_PATH_BYTES + 8, "path of {} bytes", ch.path.len());
    }
    assert!(r.changes.iter().any(|c| c.path.starts_with(".../")), "an over-long path is cut");
}
