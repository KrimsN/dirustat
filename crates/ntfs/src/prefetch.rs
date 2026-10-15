//! Read-ahead of the MFT byte stream: worker threads read consecutive chunks of the `$MFT` extents at the
//! same time (several requests in flight keep a virtual or network disk busy) and hand them to the caller
//! strictly in file order, so parsing overlaps with I/O and the result is deterministic.
//!
//! Memory is bounded: a worker only starts chunk `n` while fewer than `window` chunks are waiting for the
//! consumer. Cancellation is polled before every chunk and while waiting. Errors surface in stream order.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::reader::{BlockSource, ReaderIo, nanos_since};
use crate::runs::Extent;
use crate::volume::{MAX_READ_BYTES, VolumeError};

/// Most worker threads (and so requests in flight) used for one stream.
pub const MAX_READ_WORKERS: usize = 4;

/// How long a blocked worker sleeps before it looks at the stop callback again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What to read: the extents of the `$MFT` in file order.
#[derive(Clone, Debug)]
pub struct StreamPlan {
    pub extents: Vec<Extent>,
    pub bytes_per_cluster: u32,
    /// Preferred bytes per read; rounded down to whole clusters and capped at `MAX_READ_BYTES`.
    pub chunk_bytes: usize,
    /// Planning stops once at least this many bytes are scheduled (the MFT may be shorter than its extents).
    pub limit_bytes: u64,
}

/// How a stream ended and what it cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamSummary {
    /// The stop callback fired; the stream is incomplete.
    pub stopped: bool,
    /// Sum over all workers: `read_nanos` is thread time, not wall time.
    pub io: ReaderIo,
    /// Time the consumer sat idle waiting for the next chunk.
    pub wait_nanos: u64,
    /// Workers that ran.
    pub workers: u32,
}

struct Job {
    seq: u64,
    offset: u64,
    len: usize,
}

struct Planner {
    extents: Vec<Extent>,
    bytes_per_cluster: u32,
    chunk: usize,
    limit: u64,
    ext_index: usize,
    ext_offset: u64,
    planned: u64,
    next_seq: u64,
}

impl Planner {
    fn new(plan: StreamPlan) -> Result<Self, VolumeError> {
        let cluster = usize::try_from(plan.bytes_per_cluster).map_err(|_| VolumeError::InvalidRange)?;
        if cluster == 0 {
            return Err(VolumeError::InvalidRange);
        }
        let whole = plan.chunk_bytes.min(MAX_READ_BYTES) / cluster * cluster;
        Ok(Self {
            extents: plan.extents,
            bytes_per_cluster: plan.bytes_per_cluster,
            chunk: whole.max(cluster),
            limit: plan.limit_bytes,
            ext_index: 0,
            ext_offset: 0,
            planned: 0,
            next_seq: 0,
        })
    }

    /// The next chunk to read; `Ok(None)` when the extents or the byte limit are exhausted.
    fn next_job(&mut self) -> Result<Option<Job>, VolumeError> {
        while let Some(extent) = self.extents.get(self.ext_index) {
            if self.planned >= self.limit {
                return Ok(None);
            }
            let total = extent.byte_len(self.bytes_per_cluster).ok_or(VolumeError::InvalidRange)?;
            let remaining = total.saturating_sub(self.ext_offset);
            if remaining == 0 {
                self.ext_index = self.ext_index.saturating_add(1);
                self.ext_offset = 0;
                continue;
            }
            let len = usize::try_from(remaining).map_or(self.chunk, |r| r.min(self.chunk));
            let len64 = u64::try_from(len).map_err(|_| VolumeError::InvalidRange)?;
            let offset = extent
                .byte_offset(self.bytes_per_cluster)
                .and_then(|o| o.checked_add(self.ext_offset))
                .ok_or(VolumeError::InvalidRange)?;
            self.ext_offset = self.ext_offset.saturating_add(len64);
            self.planned = self.planned.saturating_add(len64);
            let seq = self.next_seq;
            self.next_seq = self.next_seq.saturating_add(1);
            return Ok(Some(Job { seq, offset, len }));
        }
        Ok(None)
    }
}

struct Shared {
    planner: Planner,
    /// Chunks the consumer has taken so far (they no longer count against the window).
    consumed: u64,
    planning_done: bool,
    abort: bool,
    stopped: bool,
    panicked: bool,
}

struct Coordinator {
    state: Mutex<Shared>,
    wake: Condvar,
    window: u64,
}

impl Coordinator {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn abort(&self) {
        self.lock().abort = true;
        self.wake.notify_all();
    }
}

/// Marks the stream as failed when a worker unwinds, so the others and the consumer do not wait forever.
struct PanicGuard<'a>(&'a Coordinator);

impl Drop for PanicGuard<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut g = self.0.lock();
            g.abort = true;
            g.panicked = true;
            drop(g);
            self.0.wake.notify_all();
        }
    }
}

type Chunk = (u64, Result<Vec<u8>, VolumeError>);

#[derive(Default)]
struct WorkerIo {
    read_nanos: AtomicU64,
    read_calls: AtomicU64,
    bytes: AtomicU64,
}

fn worker<S: BlockSource>(
    source: &S,
    shared: &Coordinator,
    tx: &Sender<Chunk>,
    stop: &(dyn Fn() -> bool + Sync),
    io: &WorkerIo,
) {
    let _guard = PanicGuard(shared);
    loop {
        let job = {
            let mut g = shared.lock();
            loop {
                if g.abort || g.planning_done {
                    return;
                }
                if stop() {
                    g.abort = true;
                    g.stopped = true;
                    shared.wake.notify_all();
                    return;
                }
                if g.planner.next_seq < g.consumed.saturating_add(shared.window) {
                    break;
                }
                g = shared.wake.wait_timeout(g, POLL_INTERVAL).unwrap_or_else(PoisonError::into_inner).0;
            }
            match g.planner.next_job() {
                Ok(Some(job)) => job,
                Ok(None) => {
                    g.planning_done = true;
                    shared.wake.notify_all();
                    return;
                }
                Err(e) => {
                    g.planning_done = true;
                    let seq = g.planner.next_seq;
                    drop(g);
                    shared.wake.notify_all();
                    let _ = tx.send((seq, Err(e)));
                    return;
                }
            }
        };
        let started = Instant::now();
        let result = source.read_at(job.offset, job.len);
        io.read_nanos.fetch_add(nanos_since(started), Ordering::Relaxed);
        io.read_calls.fetch_add(1, Ordering::Relaxed);
        let result = match result {
            Ok(data) if data.len() == job.len => {
                io.bytes.fetch_add(u64::try_from(job.len).unwrap_or(u64::MAX), Ordering::Relaxed);
                Ok(data)
            }
            Ok(_) => Err(VolumeError::ShortRead),
            Err(e) => Err(e),
        };
        let failed = result.is_err();
        if tx.send((job.seq, result)).is_err() {
            return;
        }
        if failed {
            // Nothing after a failed chunk is useful; let the others stop early.
            return;
        }
    }
}

/// Reads the planned chunks with one worker per source (all in flight together) and passes them to
/// `on_chunk` in stream order on the calling thread. `on_chunk` returns `false` to end the stream early.
///
/// `stop` is polled by every worker before each chunk; when it fires the summary has `stopped` set and the
/// chunks delivered so far are all that will come.
pub fn read_mft_stream<S: BlockSource + Send>(
    sources: Vec<S>,
    plan: StreamPlan,
    stop: &(dyn Fn() -> bool + Sync),
    on_chunk: &mut dyn FnMut(Vec<u8>) -> bool,
) -> Result<StreamSummary, VolumeError> {
    if sources.is_empty() {
        return Err(VolumeError::InvalidRange);
    }
    let workers = sources.len().min(MAX_READ_WORKERS);
    let window = u64::try_from(workers).unwrap_or(1).saturating_add(1);
    let shared = Coordinator {
        state: Mutex::new(Shared {
            planner: Planner::new(plan)?,
            consumed: 0,
            planning_done: false,
            abort: false,
            stopped: false,
            panicked: false,
        }),
        wake: Condvar::new(),
        window,
    };
    let io = WorkerIo::default();
    let (tx, rx) = mpsc::channel::<Chunk>();
    let mut wait_nanos = 0u64;
    let mut failure: Option<VolumeError> = None;
    let mut spawned = 0u32;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for source in sources.into_iter().take(workers) {
            let tx = tx.clone();
            let (shared, io) = (&shared, &io);
            let spawn = std::thread::Builder::new()
                .name("mft-read".to_owned())
                .spawn_scoped(scope, move || worker(&source, shared, &tx, stop, io));
            match spawn {
                Ok(handle) => {
                    spawned = spawned.saturating_add(1);
                    handles.push(handle);
                }
                Err(_) => break,
            }
        }
        // The consumer's own sender must go, or `recv` would never see the channel close.
        drop(tx);
        if spawned == 0 {
            failure = Some(VolumeError::Io(u32::MAX));
            return;
        }

        let mut pending: BTreeMap<u64, Result<Vec<u8>, VolumeError>> = BTreeMap::new();
        let mut next = 0u64;
        'stream: loop {
            let waited = Instant::now();
            let message = rx.recv();
            wait_nanos = wait_nanos.saturating_add(nanos_since(waited));
            let Ok((seq, result)) = message else { break };
            pending.insert(seq, result);
            while let Some(result) = pending.remove(&next) {
                next = next.saturating_add(1);
                shared.lock().consumed = next;
                shared.wake.notify_all();
                match result {
                    Ok(chunk) => {
                        if !on_chunk(chunk) {
                            shared.abort();
                            break 'stream;
                        }
                    }
                    Err(e) => {
                        failure = Some(e);
                        shared.abort();
                        break 'stream;
                    }
                }
            }
        }
        // Dropping the receiver fails the sends of workers that are still reading.
        drop(rx);
        for handle in handles {
            if handle.join().is_err() {
                failure.get_or_insert(VolumeError::Io(u32::MAX));
            }
        }
    });

    let g = shared.lock();
    if g.panicked {
        failure.get_or_insert(VolumeError::Io(u32::MAX));
    }
    if let Some(e) = failure {
        return Err(e);
    }
    Ok(StreamSummary {
        stopped: g.stopped,
        io: ReaderIo {
            read_nanos: io.read_nanos.load(Ordering::Relaxed),
            read_calls: io.read_calls.load(Ordering::Relaxed),
            bytes_read: io.bytes.load(Ordering::Relaxed),
        },
        wait_nanos,
        workers: spawned,
    })
}

#[cfg(test)]
mod tests;
