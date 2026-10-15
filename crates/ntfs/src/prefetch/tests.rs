use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

use super::*;

const CLUSTER: u32 = 512;

/// A byte pattern where every byte reveals its position, so misordered or misplaced chunks are caught.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from((i * 7 + i / 251) % 256).unwrap()).collect()
}

#[derive(Clone)]
struct TestSource {
    data: Arc<Vec<u8>>,
    calls: Arc<AtomicUsize>,
    /// Every n-th call sleeps, to scramble the completion order (0 = never).
    slow_every: usize,
    fail_at_offset: Option<u64>,
}

impl TestSource {
    fn new(len: usize) -> Self {
        Self { data: Arc::new(pattern(len)), calls: Arc::new(AtomicUsize::new(0)), slow_every: 0, fail_at_offset: None }
    }
}

impl BlockSource for TestSource {
    fn read_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, VolumeError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.slow_every > 0 && n.is_multiple_of(self.slow_every) {
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.fail_at_offset == Some(offset) {
            return Err(VolumeError::Io(5));
        }
        let start = usize::try_from(offset).map_err(|_| VolumeError::InvalidRange)?;
        self.data.get(start..start + len).map(<[u8]>::to_vec).ok_or(VolumeError::ShortRead)
    }

    fn duplicate(&self) -> Option<Self> {
        Some(self.clone())
    }
}

fn plan(extents: Vec<Extent>, chunk: usize, limit: u64) -> StreamPlan {
    StreamPlan { extents, bytes_per_cluster: CLUSTER, chunk_bytes: chunk, limit_bytes: limit }
}

fn one_extent(clusters: u64) -> Vec<Extent> {
    vec![Extent { vcn: 0, lcn: 0, clusters }]
}

fn never() -> bool {
    false
}

fn sources(src: &TestSource, n: usize) -> Vec<TestSource> {
    (0..n).map(|_| src.clone()).collect()
}

fn collect_stream(srcs: Vec<TestSource>, p: StreamPlan) -> (Result<StreamSummary, VolumeError>, Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut chunks = 0usize;
    let res = read_mft_stream(srcs, p, &never, &mut |c| {
        chunks += 1;
        out.extend_from_slice(&c);
        true
    });
    (res, out, chunks)
}

#[test]
fn delivers_chunks_in_order_with_many_workers_and_scrambled_timing() {
    let total = 512 * 200;
    let mut src = TestSource::new(total);
    src.slow_every = 3;
    let (res, out, chunks) = collect_stream(sources(&src, 4), plan(one_extent(200), 512 * 8, u64::MAX));
    let summary = res.unwrap();
    assert_eq!(out, *src.data);
    assert_eq!(chunks, 25);
    assert_eq!(summary.workers, 4);
    assert_eq!(summary.io.read_calls, 25);
    assert_eq!(summary.io.bytes_read, total as u64);
    assert!(!summary.stopped);
}

#[test]
fn single_worker_reads_everything() {
    let src = TestSource::new(512 * 11);
    let (res, out, chunks) = collect_stream(sources(&src, 1), plan(one_extent(11), 512 * 4, u64::MAX));
    assert_eq!(res.unwrap().workers, 1);
    assert_eq!(out, *src.data);
    assert_eq!(chunks, 3);
}

#[test]
fn follows_fragmented_extents_in_file_order() {
    let src = TestSource::new(512 * 40);
    let extents = vec![
        Extent { vcn: 0, lcn: 30, clusters: 5 },
        Extent { vcn: 5, lcn: 2, clusters: 7 },
        Extent { vcn: 12, lcn: 20, clusters: 3 },
    ];
    let mut expected = Vec::new();
    for e in &extents {
        let start = usize::try_from(e.lcn).unwrap() * 512;
        expected.extend_from_slice(&src.data[start..start + usize::try_from(e.clusters).unwrap() * 512]);
    }
    let (res, out, _) = collect_stream(sources(&src, 2), plan(extents, 512 * 2, u64::MAX));
    res.unwrap();
    assert_eq!(out, expected);
}

#[test]
fn the_byte_limit_stops_planning() {
    let src = TestSource::new(512 * 100);
    // Planning continues while fewer than `limit` bytes are scheduled: 3 chunks cover 6144 bytes.
    let (res, out, chunks) = collect_stream(sources(&src, 2), plan(one_extent(100), 512 * 4, 512 * 4 * 3 - 1));
    res.unwrap();
    assert_eq!(chunks, 3);
    assert_eq!(out.len(), 512 * 4 * 3);
    assert_eq!(src.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn a_zero_limit_reads_nothing() {
    let src = TestSource::new(512 * 10);
    let (res, out, chunks) = collect_stream(sources(&src, 1), plan(one_extent(10), 512 * 4, 0));
    assert!(res.is_ok());
    assert!(out.is_empty());
    assert_eq!(chunks, 0);
    assert_eq!(src.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn stop_callback_ends_the_stream_and_reports_it() {
    let mut src = TestSource::new(512 * 400);
    src.slow_every = 1;
    let flag = AtomicBool::new(false);
    let stop = || flag.load(Ordering::SeqCst);
    let mut seen = 0usize;
    let res = read_mft_stream(sources(&src, 2), plan(one_extent(400), 512 * 2, u64::MAX), &stop, &mut |_| {
        seen += 1;
        if seen == 2 {
            flag.store(true, Ordering::SeqCst);
        }
        true
    })
    .unwrap();
    assert!(res.stopped);
    assert!((2..200).contains(&seen), "{seen}");
}

#[test]
fn the_consumer_can_end_the_stream_early() {
    let src = TestSource::new(512 * 400);
    let mut seen = 0usize;
    let res = read_mft_stream(sources(&src, 3), plan(one_extent(400), 512, u64::MAX), &never, &mut |_| {
        seen += 1;
        seen < 5
    })
    .unwrap();
    assert_eq!(seen, 5);
    assert!(!res.stopped);
    // Reading ahead is bounded by the window, not by the size of the MFT.
    let calls = src.calls.load(Ordering::SeqCst);
    assert!(calls < 5 + 6, "{calls}");
}

#[test]
fn a_read_error_is_reported_in_stream_order() {
    let mut src = TestSource::new(512 * 40);
    src.fail_at_offset = Some(512 * 4 * 3);
    let mut delivered = 0usize;
    let res = read_mft_stream(sources(&src, 2), plan(one_extent(40), 512 * 4, u64::MAX), &never, &mut |_| {
        delivered += 1;
        true
    });
    assert_eq!(res, Err(VolumeError::Io(5)));
    // The chunks before the failing one (0, 1, 2) were handed over first.
    assert_eq!(delivered, 3);
}

#[test]
fn the_reader_is_bounded_while_the_consumer_is_slow() {
    let src = TestSource::new(512 * 200);
    let calls = Arc::clone(&src.calls);
    let mut first = true;
    let mut at_first_chunk = 0usize;
    read_mft_stream(sources(&src, 4), plan(one_extent(200), 512, u64::MAX), &never, &mut |_| {
        if first {
            first = false;
            std::thread::sleep(Duration::from_millis(300));
            at_first_chunk = calls.load(Ordering::SeqCst);
        }
        true
    })
    .unwrap();
    // window = workers + 1 chunks may be outstanding beyond the one being consumed.
    assert!(at_first_chunk <= 1 + 5, "{at_first_chunk}");
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let src = TestSource::new(512 * 4);
    let empty: Vec<TestSource> = Vec::new();
    assert_eq!(
        read_mft_stream(empty, plan(one_extent(4), 512, u64::MAX), &never, &mut |_| true),
        Err(VolumeError::InvalidRange)
    );
    let zero_cluster = StreamPlan { bytes_per_cluster: 0, ..plan(one_extent(4), 512, u64::MAX) };
    assert_eq!(read_mft_stream(sources(&src, 1), zero_cluster, &never, &mut |_| true), Err(VolumeError::InvalidRange));
    // An extent whose byte length overflows is reported, not wrapped.
    let huge = vec![Extent { vcn: 0, lcn: 0, clusters: u64::MAX }];
    assert_eq!(
        read_mft_stream(sources(&src, 1), plan(huge, 512, u64::MAX), &never, &mut |_| true),
        Err(VolumeError::InvalidRange)
    );
    // An extent beyond the source is a read error from the source.
    let beyond = vec![Extent { vcn: 0, lcn: 1000, clusters: 2 }];
    assert_eq!(
        read_mft_stream(sources(&src, 1), plan(beyond, 512, u64::MAX), &never, &mut |_| true),
        Err(VolumeError::ShortRead)
    );
}

#[test]
fn chunk_size_is_rounded_to_clusters_and_capped() {
    assert_eq!(Planner::new(plan(one_extent(1), 1000, u64::MAX)).unwrap().chunk, 512);
    assert_eq!(Planner::new(plan(one_extent(1), 100, u64::MAX)).unwrap().chunk, 512);
    assert_eq!(Planner::new(plan(one_extent(1), usize::MAX, u64::MAX)).unwrap().chunk, MAX_READ_BYTES);
}
