use super::*;
use crate::testutil::*;
use proptest::prelude::*;

fn limits() -> ParseLimits {
    ParseLimits::default()
}

fn parse_built(rec: &RecordBuilder) -> Result<MftRecord, ParseError> {
    let mut bytes = rec.build();
    parse_raw_record(&mut bytes, &limits())
}

fn sample_file() -> RecordBuilder {
    RecordBuilder::file()
        .attr(standard_info(UNIX_EPOCH_FILETIME, 0x20))
        .attr(file_name(5, 5, 1, &utf16("report.txt"), 1234, 4096))
        .attr(resident(0x80, &[], 0, b"hello"))
}

#[test]
fn parses_resident_file() {
    let r = parse_built(&sample_file()).unwrap();
    assert!(r.in_use && !r.is_directory && r.is_base());
    assert_eq!((r.sequence, r.hard_link_count, r.record_number), (7, 1, Some(42)));
    let si = r.standard_info.unwrap();
    assert_eq!((si.modified, si.attributes), (UNIX_EPOCH_FILETIME, 0x20));
    let name = r.best_name().unwrap();
    assert_eq!(name.name, utf16("report.txt"));
    assert_eq!(name.parent, FileRef { record: 5, sequence: 5 });
    assert_eq!((name.real_size, name.allocated_size, name.namespace), (1234, 4096, Namespace::Win32));
    let d = r.data.unwrap();
    assert!(d.resident);
    assert_eq!((d.size, d.allocated, d.initialized), (5, 0, 5));
    assert!(r.attribute_list.is_none() && r.named_stream_count == 0);
}

#[test]
fn parses_directory_and_not_in_use() {
    let dir = RecordBuilder::directory().attr(file_name(5, 5, 3, &utf16("dir"), 0, 0));
    let r = parse_built(&dir).unwrap();
    assert!(r.is_directory && r.in_use && r.data.is_none());
    let freed = RecordBuilder { flags: 0, ..sample_file() };
    let r = parse_built(&freed).unwrap();
    assert!(!r.in_use);
}

#[test]
fn parses_nonresident_data_with_runs() {
    let runs = [Run { lcn: Some(1000), len: 8 }, Run { lcn: None, len: 4 }, Run { lcn: Some(900), len: 2 }];
    let rec = RecordBuilder::file().attr(file_name(5, 5, 1, &utf16("big.bin"), 0, 0)).attr(nonresident(&NonResident {
        allocated: 14 * 4096,
        size: 50_000,
        initialized: 40_000,
        flags: 0x8000,
        compression_unit: 4,
        compressed_size: Some(10 * 4096),
        runs: &encode_runs(&runs),
        ..NonResident::default()
    }));
    let d = parse_built(&rec).unwrap().data.unwrap();
    assert!(!d.resident && d.sparse && !d.compressed && !d.encrypted);
    assert_eq!((d.size, d.allocated, d.initialized, d.compressed_size), (50_000, 14 * 4096, 40_000, Some(40_960)));
    assert_eq!(d.runs, runs);
}

#[test]
fn run_decoding_can_be_disabled() {
    let rec = RecordBuilder::file().attr(nonresident(&NonResident {
        size: 10,
        runs: &encode_runs(&[Run { lcn: Some(5), len: 1 }]),
        ..NonResident::default()
    }));
    let mut bytes = rec.build();
    let l = ParseLimits { decode_runs: false, ..limits() };
    let d = parse_raw_record(&mut bytes, &l).unwrap().data.unwrap();
    assert!(d.runs.is_empty());
    assert_eq!(d.size, 10);
}

#[test]
fn bad_runs_in_data_fail_the_record() {
    let rec = RecordBuilder::file().attr(nonresident(&NonResident {
        size: 10,
        runs: &[0x11, 0x00, 0x05, 0],
        ..NonResident::default()
    }));
    assert_eq!(parse_built(&rec), Err(ParseError::ZeroLengthRun));
}

#[test]
fn alternate_streams_do_not_count_for_size() {
    let rec = sample_file().attr(resident(0x80, &utf16("Zone.Identifier"), 0, b"[ZoneTransfer]")).attr(nonresident(
        &NonResident { name: &utf16("big"), size: 1 << 30, runs: &[0x11, 0x01, 0x05, 0], ..NonResident::default() },
    ));
    let r = parse_built(&rec).unwrap();
    assert_eq!(r.logical_size(), 5);
    assert_eq!(r.named_stream_count, 2);
    assert_eq!(r.named_streams.len(), 2);
    assert_eq!(
        r.named_streams.first().map(|s| (s.name.clone(), s.size, s.resident)),
        Some((utf16("Zone.Identifier"), 14, true))
    );
    assert_eq!(r.named_streams.get(1).map(|s| (s.size, s.resident)), Some((1 << 30, false)));
}

#[test]
fn named_stream_cap_still_counts() {
    let mut rec = sample_file();
    for i in 0..5 {
        rec = rec.attr(resident(0x80, &utf16(&format!("s{i}")), 0, b"x"));
    }
    let mut bytes = rec.build();
    let l = ParseLimits { max_named_streams: 2, ..limits() };
    let r = parse_raw_record(&mut bytes, &l).unwrap();
    assert_eq!((r.named_streams.len(), r.named_stream_count), (2, 5));
}

#[test]
fn only_a_named_stream_leaves_no_unnamed_data() {
    let rec = RecordBuilder::file().attr(resident(0x80, &utf16("only"), 0, b"x"));
    let r = parse_built(&rec).unwrap();
    assert!(r.data.is_none());
    assert_eq!(r.logical_size(), 0);
}

#[test]
fn dos_name_policy_prefers_win32() {
    let rec = RecordBuilder::file().attr(file_name(5, 5, 2, &utf16("LONGFI~1.TXT"), 1, 1)).attr(file_name(
        5,
        5,
        1,
        &utf16("long file name.txt"),
        1,
        1,
    ));
    let r = parse_built(&rec).unwrap();
    assert_eq!(r.best_name().map(|n| n.name.clone()), Some(utf16("long file name.txt")));
    let links = r.link_names();
    assert_eq!(links.len(), 1);
    assert_eq!(links.first().map(|n| n.namespace), Some(Namespace::Win32));
}

#[test]
fn dos_name_kept_when_alone_or_when_parent_differs() {
    let only_dos = RecordBuilder::file().attr(file_name(5, 5, 2, &utf16("A~1"), 1, 1));
    let r = parse_built(&only_dos).unwrap();
    assert_eq!(r.best_name().map(|n| n.namespace), Some(Namespace::Dos));
    assert_eq!(r.link_names().len(), 1);
    // A DOS name in another directory is a separate hard link and is not dropped.
    let rec = RecordBuilder::file().attr(file_name(5, 5, 3, &utf16("a.txt"), 1, 1)).attr(file_name(
        9,
        1,
        2,
        &utf16("B~1.TXT"),
        1,
        1,
    ));
    assert_eq!(parse_built(&rec).unwrap().link_names().len(), 2);
    // Win32AndDos beats POSIX; hard links under different parents both stay.
    let rec = RecordBuilder::file().attr(file_name(5, 5, 0, &utf16("Posix"), 1, 1)).attr(file_name(
        5,
        5,
        3,
        &utf16("Both"),
        1,
        1,
    ));
    let r = parse_built(&rec).unwrap();
    assert_eq!(r.best_name().map(|n| n.name.clone()), Some(utf16("Both")));
    assert_eq!(r.link_names().len(), 2);
}

#[test]
fn keeps_unpaired_surrogates() {
    let wide = vec![0x0061u16, 0xD800, 0x0062];
    let rec = RecordBuilder::file().attr(file_name(5, 5, 1, &wide, 0, 0));
    assert_eq!(parse_built(&rec).unwrap().best_name().map(|n| n.name.clone()), Some(wide));
}

#[test]
fn extension_record_and_attribute_list() {
    let mut list = list_entry(0x10, false, 0, 40);
    list.extend(list_entry(0x80, false, 0, 99));
    list.extend(list_entry(0x80, true, 0, 99));
    let rec = RecordBuilder { base: 40 | (3u64 << 48), ..RecordBuilder::file() }.attr(resident(0x20, &[], 0, &list));
    let r = parse_built(&rec).unwrap();
    assert!(!r.is_base());
    assert_eq!(r.base, Some(FileRef { record: 40, sequence: 3 }));
    let al = r.attribute_list.as_ref().unwrap();
    assert!(r.has_attribute_list() && !al.nonresident);
    assert_eq!(al.entries.len(), 3);
    assert_eq!(al.entries.get(1).map(|e| (e.attribute_type, e.segment.record, e.named)), Some((0x80, 99, false)));
    assert_eq!(al.entries.get(2).map(|e| e.named), Some(true));
}

#[test]
fn nonresident_attribute_list_is_flagged() {
    let rec = RecordBuilder::file().attr(nonresident(&NonResident {
        ty: 0x20,
        size: 4096,
        runs: &[0x11, 0x01, 0x05, 0],
        ..NonResident::default()
    }));
    let al = parse_built(&rec).unwrap().attribute_list.unwrap();
    assert!(al.nonresident && al.entries.is_empty());
}

#[test]
fn malformed_attribute_list_entry_is_an_error() {
    let mut e = list_entry(0x80, false, 0, 1);
    e[4..6].copy_from_slice(&0u16.to_le_bytes()); // zero record length would loop forever
    let rec = RecordBuilder::file().attr(resident(0x20, &[], 0, &e));
    assert_eq!(parse_built(&rec), Err(ParseError::BadAttribute));
}

#[test]
fn unknown_namespace_and_short_names_fail() {
    assert_eq!(
        parse_built(&RecordBuilder::file().attr(file_name(5, 5, 9, &utf16("x"), 0, 0))),
        Err(ParseError::BadNamespace)
    );
    // Name length claims more units than the value holds.
    let mut fnv = file_name(5, 5, 1, &utf16("ab"), 0, 0);
    fnv[24 + 64] = 200;
    assert_eq!(parse_built(&RecordBuilder::file().attr(fnv)), Err(ParseError::BadAttribute));
}

#[test]
fn unknown_attributes_are_skipped() {
    let rec = sample_file().attr(resident(0x90, &utf16("$I30"), 0, &[0; 16])).attr(resident(0xB0, &[], 0, &[0; 8]));
    assert!(parse_built(&rec).is_ok());
}

#[test]
fn huge_sizes_are_rejected() {
    let rec = RecordBuilder::file().attr(nonresident(&NonResident { size: u64::MAX, ..NonResident::default() }));
    assert_eq!(parse_built(&rec), Err(ParseError::SizeOverflow));
}

#[test]
fn attribute_cap_is_enforced() {
    let mut rec = RecordBuilder::file();
    for _ in 0..10 {
        rec = rec.attr(resident(0x90, &[], 0, &[0; 8]));
    }
    let mut bytes = rec.build();
    let l = ParseLimits { max_attributes: 5, ..limits() };
    assert_eq!(parse_raw_record(&mut bytes, &l), Err(ParseError::TooManyAttributes));
}

#[test]
fn zero_length_attribute_cannot_loop() {
    let mut bytes = sample_file().build();
    // First attribute starts at 56; set its length to 0 (a classic infinite loop).
    bytes[60..64].copy_from_slice(&0u32.to_le_bytes());
    let mut copy = bytes.clone();
    // Fixups were installed on the on-disk form; the length field is not at a trailer.
    assert_eq!(parse_raw_record(&mut copy, &limits()), Err(ParseError::BadAttribute));
}

#[test]
fn attribute_overrunning_used_size_fails() {
    let mut bytes = sample_file().build();
    bytes[60..64].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
    assert_eq!(parse_raw_record(&mut bytes, &limits()), Err(ParseError::BadAttribute));
}

#[test]
fn header_corruption_is_reported() {
    let good = sample_file().build();
    let mut b = good.clone();
    b[0] = b'B';
    assert_eq!(parse_raw_record(&mut b, &limits()), Err(ParseError::BadMagic));
    let mut b = good.clone();
    b[24..28].copy_from_slice(&5000u32.to_le_bytes());
    assert_eq!(parse_raw_record(&mut b, &limits()), Err(ParseError::BadHeader));
    let mut b = good.clone();
    b[20..22].copy_from_slice(&10u16.to_le_bytes());
    assert_eq!(parse_raw_record(&mut b, &limits()), Err(ParseError::BadHeader));
    assert_eq!(parse_record(&good[..30], &limits()), Err(ParseError::Truncated));
    assert_eq!(parse_record(&[], &limits()), Err(ParseError::BadMagic));
}

#[test]
fn zeroed_record_is_bad_magic() {
    let mut z = vec![0u8; 1024];
    assert_eq!(parse_raw_record(&mut z, &limits()), Err(ParseError::BadMagic));
}

#[test]
fn truncated_records_never_panic() {
    let good = sample_file().build();
    for n in 0..good.len() {
        let mut part = good.get(..n).unwrap().to_vec();
        let _ = parse_raw_record(&mut part, &limits());
        let _ = parse_record(&part, &limits());
    }
}

#[test]
fn fixups_roundtrip_and_restore_trailer_bytes() {
    let mut rec = RecordBuilder::file();
    // Put recognizable bytes where the sector trailers are.
    rec = rec.attr(resident(0x90, &[], 0, &[0xAA; 8]));
    let mut bytes = rec.build();
    let usn = u16::from_le_bytes([bytes[510], bytes[511]]);
    assert_eq!(usn, 0xBEEF);
    let before = bytes.clone();
    apply_fixups(&mut bytes, 512).unwrap();
    assert_eq!(&bytes[510..512], &before[50..52]);
    assert_eq!(&bytes[1022..1024], &before[52..54]);
}

#[test]
fn fixups_detect_torn_writes() {
    let mut bytes = sample_file().build();
    bytes[1022] ^= 0xFF;
    let snapshot = bytes.clone();
    assert_eq!(apply_fixups(&mut bytes, 512), Err(ParseError::BadFixup));
    assert_eq!(bytes, snapshot, "a failed validation must not modify the record");
}

#[test]
fn fixups_reject_bad_arrays() {
    let good = sample_file().build();
    type Mutation = Box<dyn Fn(&mut Vec<u8>)>;
    let cases: Vec<Mutation> = vec![
        Box::new(|b| b[6..8].copy_from_slice(&0u16.to_le_bytes())),
        Box::new(|b| b[6..8].copy_from_slice(&1u16.to_le_bytes())),
        Box::new(|b| b[6..8].copy_from_slice(&2u16.to_le_bytes())), // covers half the record
        Box::new(|b| b[6..8].copy_from_slice(&200u16.to_le_bytes())),
        Box::new(|b| b[4..6].copy_from_slice(&0xFFFFu16.to_le_bytes())),
        Box::new(|b| b[4..6].copy_from_slice(&4u16.to_le_bytes())),
        Box::new(|b| b[4..6].copy_from_slice(&1022u16.to_le_bytes())), // array inside a trailer
        Box::new(|b| b.truncate(6)),
    ];
    for (i, mutate) in cases.iter().enumerate() {
        let mut b = good.clone();
        mutate(&mut b);
        assert!(apply_fixups(&mut b, 512).is_err(), "case {i}");
    }
    for bad in [0usize, 1, 3, 511] {
        let mut b = good.clone();
        assert!(apply_fixups(&mut b, bad).is_err(), "sector {bad}");
    }
}

#[test]
fn four_k_records_use_eight_blocks() {
    let mut rec = sample_file();
    rec.record_size = 4096;
    let r = parse_built(&rec).unwrap();
    assert_eq!(r.logical_size(), 5);
}

#[test]
fn mft_extents_from_record_zero() {
    let runs = [Run { lcn: Some(4), len: 16 }, Run { lcn: Some(100), len: 16 }];
    let rec = RecordBuilder::file().attr(nonresident(&NonResident {
        size: 32 * 4096,
        allocated: 32 * 4096,
        initialized: 32 * 4096,
        runs: &encode_runs(&runs),
        ..NonResident::default()
    }));
    let r = parse_built(&rec).unwrap();
    let extents = mft_extents(&r, 4096).unwrap();
    assert_eq!(extents, vec![Extent { vcn: 0, lcn: 4, clusters: 16 }, Extent { vcn: 16, lcn: 100, clusters: 16 }]);
}

#[test]
fn mft_extents_reports_incomplete_data() {
    let short = RecordBuilder::file().attr(nonresident(&NonResident {
        size: 100 * 4096,
        runs: &encode_runs(&[Run { lcn: Some(4), len: 16 }]),
        ..NonResident::default()
    }));
    assert_eq!(mft_extents(&parse_built(&short).unwrap(), 4096), Err(ParseError::Incomplete));
    assert_eq!(mft_extents(&parse_built(&sample_file()).unwrap(), 4096), Err(ParseError::Incomplete));
    let nodata = RecordBuilder::file();
    assert_eq!(mft_extents(&parse_built(&nodata).unwrap(), 4096), Err(ParseError::Incomplete));
    let sparse = RecordBuilder::file().attr(nonresident(&NonResident {
        size: 4096,
        runs: &encode_runs(&[Run { lcn: None, len: 1 }]),
        ..NonResident::default()
    }));
    assert_eq!(mft_extents(&parse_built(&sparse).unwrap(), 4096), Err(ParseError::BadRuns));
    let ok = RecordBuilder::file().attr(nonresident(&NonResident {
        size: 4096,
        runs: &encode_runs(&[Run { lcn: Some(1), len: 1 }]),
        ..NonResident::default()
    }));
    assert_eq!(mft_extents(&parse_built(&ok).unwrap(), 0), Err(ParseError::Incomplete));
}

#[test]
fn file_ref_roundtrip() {
    let raw = (0xABCDu64 << 48) | 0x1234_5678_9ABC;
    let r = FileRef::from_raw(raw);
    assert_eq!((r.record, r.sequence), (0x1234_5678_9ABC, 0xABCD));
    assert_eq!(r.to_raw(), raw);
}

fn arb_run() -> impl Strategy<Value = Run> {
    (proptest::option::weighted(0.8, 0u64..(1 << 40)), 1u64..(1 << 30)).prop_map(|(lcn, len)| Run { lcn, len })
}

proptest! {
    #[test]
    fn parse_record_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse_record(&bytes, &ParseLimits::default());
        let mut copy = bytes.clone();
        let _ = parse_raw_record(&mut copy, &ParseLimits::default());
    }

    #[test]
    fn parse_record_with_valid_header_never_panics(
        tail in proptest::collection::vec(any::<u8>(), 0..1500),
        used in 0u32..4096,
        first in 0u16..2048,
    ) {
        let mut bytes = vec![0u8; 48];
        bytes[0..4].copy_from_slice(b"FILE");
        bytes[20..22].copy_from_slice(&first.to_le_bytes());
        bytes[24..28].copy_from_slice(&used.to_le_bytes());
        bytes.extend_from_slice(&tail);
        let _ = parse_record(&bytes, &ParseLimits::default());
    }

    #[test]
    fn apply_fixups_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048), sector in 0usize..4096) {
        let mut copy = bytes.clone();
        if apply_fixups(&mut copy, sector).is_err() {
            prop_assert_eq!(copy, bytes);
        }
    }

    #[test]
    fn mutated_valid_records_never_panic(
        flips in proptest::collection::vec((0usize..1024, any::<u8>()), 0..16),
        cut in 0usize..1100,
    ) {
        let rec = sample_file().attr(nonresident(&NonResident {
            size: 9, runs: &encode_runs(&[Run { lcn: Some(7), len: 3 }]), ..NonResident::default()
        }));
        let mut bytes = rec.build();
        for (i, v) in flips {
            if let Some(b) = bytes.get_mut(i) { *b = v; }
        }
        bytes.truncate(cut.max(1));
        let _ = parse_raw_record(&mut bytes, &ParseLimits::default());
    }

    #[test]
    fn runs_roundtrip(runs in proptest::collection::vec(arb_run(), 0..40)) {
        let encoded = encode_runs(&runs);
        let decoded = decode_data_runs(&encoded, &ParseLimits { max_total_clusters: u64::MAX, ..ParseLimits::default() });
        prop_assert_eq!(decoded, Ok(runs));
    }

    #[test]
    fn decode_data_runs_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = decode_data_runs(&bytes, &ParseLimits::default());
        let _ = decode_data_runs(&bytes, &ParseLimits { max_total_clusters: u64::MAX, max_runs: usize::MAX, ..ParseLimits::default() });
    }

    #[test]
    fn built_records_roundtrip(
        name in "[a-zA-Z0-9 ._-]{1,40}",
        size in 0u64..(1 << 40),
        parent in 0u64..(1 << 32),
    ) {
        let rec = RecordBuilder::file()
            .attr(file_name(parent, 1, 1, &utf16(&name), size, size))
            .attr(nonresident(&NonResident { size, allocated: size, initialized: size, runs: &encode_runs(&[Run { lcn: Some(3), len: 1 }]), ..NonResident::default() }));
        let r = parse_built(&rec).unwrap();
        prop_assert_eq!(r.best_name().map(|n| n.name.clone()), Some(utf16(&name)));
        prop_assert_eq!(r.best_name().map(|n| n.parent.record), Some(parent));
        prop_assert_eq!(r.logical_size(), size);
    }
}
