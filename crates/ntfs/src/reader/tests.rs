use super::*;
use crate::record::mft_extents;
use crate::runs::Run;
use crate::testutil::*;

const CLUSTER: usize = 4096;
const RECORD: usize = 1024;

fn geometry() -> MftGeometry {
    MftGeometry::new(512, 4096, 1024).unwrap()
}

fn file_record(n: u64) -> Vec<u8> {
    RecordBuilder::file()
        .attr(file_name(5, 5, 1, &utf16(&format!("file{n}.txt")), n, n))
        .attr(resident(0x80, &[], 0, &vec![b'x'; usize::try_from(n).unwrap()]))
        .build()
}

/// An image with an MFT of `records` records split across two fragments (clusters 1..=2 and 5..=6),
/// 4 records per cluster. Record 0 describes the MFT itself; slot 7 is left zeroed.
fn image(records: usize) -> (MemSource, Vec<Extent>, u64) {
    assert!(records <= 32);
    let mft_clusters = records.div_ceil(4) as u64;
    let first = mft_clusters.min(2);
    let second = mft_clusters - first;
    let mut runs = vec![Run { lcn: Some(1), len: first }];
    if second > 0 {
        runs.push(Run { lcn: Some(5), len: second });
    }
    let size = u64::try_from(records * RECORD).unwrap();
    let rec0 = RecordBuilder::file()
        .attr(file_name(5, 5, 1, &utf16("$MFT"), size, size))
        .attr(nonresident(&NonResident {
            size,
            allocated: mft_clusters * 4096,
            initialized: size,
            runs: &encode_runs(&runs),
            ..NonResident::default()
        }))
        .build();
    let mut disk = vec![0u8; 16 * CLUSTER];
    let mut put = |index: usize, bytes: &[u8]| {
        let at = if index < 8 { CLUSTER + index * RECORD } else { 5 * CLUSTER + (index - 8) * RECORD };
        disk[at..at + bytes.len()].copy_from_slice(bytes);
    };
    put(0, &rec0);
    for i in 1..records {
        if i != 7 {
            put(i, &file_record(i as u64));
        }
    }
    let mut parsed = rec0;
    let record0 = crate::record::parse_raw_record(&mut parsed, &ParseLimits::default()).unwrap();
    (MemSource(disk), mft_extents(&record0, 4096).unwrap(), size)
}

fn collect(records: usize, limits: ReaderLimits) -> (Vec<MftItem>, Option<VolumeError>, u64) {
    let (src, extents, size) = image(records);
    let mut reader = MftReader::new(&src, geometry(), extents, size, limits).unwrap();
    let mut items = Vec::new();
    let mut err = None;
    for item in reader.by_ref() {
        match item {
            Ok(i) => items.push(i),
            Err(e) => err = Some(e),
        }
    }
    (items, err, reader.skipped_empty())
}

#[test]
fn reads_all_records_across_fragments() {
    let (items, err, skipped) = collect(12, ReaderLimits::default());
    assert_eq!(err, None);
    assert_eq!(skipped, 1);
    let numbers: Vec<u64> = items.iter().map(|i| i.number).collect();
    assert_eq!(numbers, vec![0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11]);
    for item in &items {
        let r = item.record.as_ref().unwrap();
        if item.number == 0 {
            assert_eq!(r.best_name().map(|n| n.name.clone()), Some(utf16("$MFT")));
        } else {
            assert_eq!(r.best_name().map(|n| n.name.clone()), Some(utf16(&format!("file{}.txt", item.number))));
            assert_eq!(r.logical_size(), item.number);
        }
    }
}

#[test]
fn tiny_chunks_give_the_same_result() {
    let (full, ..) = collect(12, ReaderLimits::default());
    let small = ReaderLimits { chunk_bytes: 1, ..ReaderLimits::default() };
    let (chunked, err, _) = collect(12, small);
    assert_eq!(err, None);
    assert_eq!(full, chunked);
}

#[test]
fn record_cap_limits_the_output() {
    let (items, ..) = collect(12, ReaderLimits { max_records: 3, ..ReaderLimits::default() });
    assert_eq!(items.len(), 3);
    let (items, ..) = collect(12, ReaderLimits { max_records: 0, ..ReaderLimits::default() });
    assert!(items.is_empty());
}

#[test]
fn mft_size_bounds_the_scan() {
    let (src, extents, _) = image(12);
    // Only 5 records worth of valid data, although the extents hold 12.
    let reader = MftReader::new(&src, geometry(), extents, 5 * 1024 + 100, ReaderLimits::default()).unwrap();
    assert_eq!(reader.filter_map(Result::ok).count(), 5);
}

#[test]
fn corrupt_record_is_data_not_a_failure() {
    let (mut src, extents, size) = image(8);
    // Break the sector trailer of record 2.
    src.0[CLUSTER + 2 * RECORD + 510] ^= 0xFF;
    let reader = MftReader::new(&src, geometry(), extents, size, ReaderLimits::default()).unwrap();
    let items: Vec<MftItem> = reader.map(Result::unwrap).collect();
    assert_eq!(items.len(), 7);
    assert_eq!(items.iter().filter(|i| i.record.is_err()).count(), 1);
    assert_eq!(items.iter().find(|i| i.record.is_err()).map(|i| i.number), Some(2));
    assert_eq!(
        items.iter().find(|i| i.record.is_err()).and_then(|i| i.record.as_ref().err().copied()),
        Some(ParseError::BadFixup)
    );
}

#[test]
fn io_error_is_yielded_once_and_ends_iteration() {
    let (src, extents, size) = image(12);
    // Cut the image so the second fragment is out of range.
    let short = MemSource(src.0.get(..3 * CLUSTER).unwrap().to_vec());
    let mut reader = MftReader::new(&short, geometry(), extents, size, ReaderLimits::default()).unwrap();
    let mut errors = 0;
    let mut records = 0;
    for item in reader.by_ref() {
        match item {
            Ok(_) => records += 1,
            Err(e) => {
                errors += 1;
                assert_eq!(e, VolumeError::ShortRead);
            }
        }
    }
    assert_eq!((errors, records > 0), (1, true));
    assert!(reader.next().is_none());
}

#[test]
fn hostile_extents_are_rejected_up_front() {
    let src = MemSource(Vec::new());
    let bad = vec![Extent { vcn: 0, lcn: u64::MAX, clusters: 1 }];
    assert_eq!(
        MftReader::new(&src, geometry(), bad, 1024, ReaderLimits::default()).err(),
        Some(ParseError::RunOverflow)
    );
    let bad = vec![Extent { vcn: 0, lcn: 1 << 50, clusters: u64::MAX }];
    assert!(MftReader::new(&src, geometry(), bad, 1024, ReaderLimits::default()).is_err());
    // An extent that points outside the source yields a read error, not a panic.
    let far = vec![Extent { vcn: 0, lcn: 1 << 40, clusters: 4 }];
    let mut reader = MftReader::new(&src, geometry(), far, 4096, ReaderLimits::default()).unwrap();
    assert!(matches!(reader.next(), Some(Err(_))));
    assert!(reader.next().is_none());
    // No extents: nothing to read.
    let mut reader = MftReader::new(&src, geometry(), vec![], 4096, ReaderLimits::default()).unwrap();
    assert!(reader.next().is_none());
}

#[test]
fn geometry_validation() {
    assert!(MftGeometry::new(512, 4096, 1024).is_ok());
    assert!(MftGeometry::new(4096, 4096, 4096).is_ok());
    for (s, c, r) in [
        (0, 4096, 1024),
        (512, 256, 1024),
        (512, 4096, 100),
        (500, 4096, 1024),
        (512, 4096, 1 << 20),
        (512, 1 << 30, 1024),
    ] {
        assert_eq!(MftGeometry::new(s, c, r), Err(ParseError::BadGeometry), "{s} {c} {r}");
    }
}

#[test]
fn mem_source_bounds() {
    let m = MemSource(vec![1, 2, 3, 4]);
    assert_eq!(m.read_at(1, 2), Ok(vec![2, 3]));
    assert_eq!(m.read_at(3, 2), Err(VolumeError::ShortRead));
    assert_eq!(m.read_at(u64::MAX, 2), Err(VolumeError::InvalidRange));
    assert_eq!(m.read_at(1, usize::MAX), Err(VolumeError::InvalidRange));
}

#[test]
fn bounded_by_rejects_extents_outside_or_larger_than_the_volume() {
    let (src, extents, size) = image(12);
    let total = 16 * CLUSTER as u64;
    // The real layout fits.
    let ok = MftReader::new(&src, geometry(), extents.clone(), size, ReaderLimits::default()).unwrap();
    assert!(ok.bounded_by(total).is_ok());
    // A volume that ends before the second fragment.
    let r = MftReader::new(&src, geometry(), extents, size, ReaderLimits::default()).unwrap();
    assert_eq!(r.bounded_by(5 * CLUSTER as u64).err(), Some(ParseError::ExtentOutOfRange));
    // Forged: two overlapping extents that together claim more than the volume holds.
    let overlap = vec![Extent { vcn: 0, lcn: 1, clusters: 10 }, Extent { vcn: 10, lcn: 1, clusters: 10 }];
    let r = MftReader::new(&src, geometry(), overlap, size, ReaderLimits::default()).unwrap();
    assert_eq!(r.bounded_by(16 * CLUSTER as u64).err(), Some(ParseError::ExtentOutOfRange));
    // Forged: one gigantic extent.
    let huge = vec![Extent { vcn: 0, lcn: 1, clusters: 1 << 30 }];
    let r = MftReader::new(&src, geometry(), huge, size, ReaderLimits::default()).unwrap();
    assert_eq!(r.bounded_by(16 * CLUSTER as u64).err(), Some(ParseError::ExtentOutOfRange));
}

#[test]
fn bounded_by_caps_the_record_count() {
    let (src, extents, size) = image(12);
    let r = MftReader::new(&src, geometry(), extents, size, ReaderLimits::default()).unwrap();
    // Room for only 4 records on the volume: at most 4 are produced.
    let r = r.bounded_by(4 * RECORD as u64 + 16 * CLUSTER as u64 - 4 * RECORD as u64).unwrap();
    assert!(r.count() <= 12);
}

#[test]
fn stop_callback_ends_iteration_between_chunks_and_inside_empty_runs() {
    let (src, extents, size) = image(12);
    let mut reader = MftReader::new(&src, geometry(), extents.clone(), size, ReaderLimits::default()).unwrap();
    assert!(reader.next_cancellable(&mut || true).is_none());
    assert!(reader.was_stopped());
    assert!(reader.next().is_none());
    // A stop that never fires behaves like `next`.
    let mut reader = MftReader::new(&src, geometry(), extents, size, ReaderLimits::default()).unwrap();
    let mut n = 0;
    while reader.next_cancellable(&mut || false).is_some() {
        n += 1;
    }
    assert_eq!((n, reader.was_stopped()), (11, false));
    // A long run of zero-filled slots polls the callback without any chunk boundary.
    let slots = 3 * 4096;
    let zero_src = MemSource(vec![0u8; slots * RECORD]);
    let ext = vec![Extent { vcn: 0, lcn: 0, clusters: (slots * RECORD / CLUSTER) as u64 }];
    let limits = ReaderLimits { chunk_bytes: slots * RECORD, ..ReaderLimits::default() };
    let mut reader = MftReader::new(&zero_src, geometry(), ext, (slots * RECORD) as u64, limits).unwrap();
    let mut polls = 0;
    let out = reader.next_cancellable(&mut || {
        polls += 1;
        polls > 1
    });
    assert!(out.is_none() && reader.was_stopped());
    assert!(reader.skipped_empty() <= 4096 + 1);
}

fn batcher(max_records: u64) -> RecordBatcher {
    RecordBatcher::new(geometry(), max_records).unwrap()
}

#[test]
fn batcher_carries_partial_records_across_chunks() {
    let mut b = batcher(100);
    let bytes: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
    let mut out = Vec::new();
    let mut first_numbers = Vec::new();
    // Chunk sizes that never line up with the 1024-byte records.
    for chunk in bytes.chunks(700) {
        if let Some(batch) = b.push(chunk.to_vec()) {
            assert_eq!(batch.record_size, 1024);
            assert_eq!(batch.bytes.len() % 1024, 0);
            first_numbers.push(batch.first_number);
            out.extend_from_slice(&batch.bytes);
        }
    }
    assert_eq!(out, bytes[..2048]);
    assert_eq!(first_numbers, vec![0, 1]);
}

#[test]
fn batcher_stops_at_the_record_cap_and_drops_the_rest() {
    let mut b = batcher(3);
    let batch = b.push(vec![1u8; 10 * 1024 + 5]).unwrap();
    assert_eq!((batch.first_number, batch.bytes.len()), (0, 3 * 1024));
    assert!(b.is_full());
    assert!(b.push(vec![1u8; 4096]).is_none());
    let mut zero = batcher(0);
    assert!(zero.is_full());
    assert!(zero.push(vec![1u8; 4096]).is_none());
}

#[test]
fn batcher_rejects_zero_record_size_and_handles_tiny_chunks() {
    let bad = MftGeometry { bytes_per_sector: 512, bytes_per_cluster: 4096, bytes_per_record: 0 };
    assert_eq!(RecordBatcher::new(bad, 1).unwrap_err(), ParseError::BadGeometry);
    let mut b = batcher(10);
    assert!(b.push(Vec::new()).is_none());
    assert!(b.push(vec![0u8; 1]).is_none());
    assert_eq!(b.push(vec![0u8; 1023]).unwrap().bytes.len(), 1024);
}

#[test]
fn reader_accounts_for_its_io() {
    let (src, extents, size) = image(16);
    let mut reader = MftReader::new(&src, geometry(), extents, size, ReaderLimits::default()).unwrap();
    while reader.next().is_some() {}
    let io = reader.io();
    assert_eq!(io.read_calls, 2);
    assert_eq!(io.bytes_read, 4 * 4096);
    assert_eq!(reader.max_records(), 16);
    assert_eq!(reader.extents().len(), 2);
}
