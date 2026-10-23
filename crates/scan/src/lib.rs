//! L1: filesystem scanning. Builds `dirustat_model::Tree`.
//!
//! The portable scanner reads directories in parallel on a rayon pool; a single builder thread owns
//! the `Tree` and turns directory listings into nodes, so there are no locks around the arena and the
//! walk is iterative (a work counter instead of recursion). Reparse points and symlinks become
//! `NodeKind::Link` nodes and are never followed, so cycles cannot occur. Unreadable directories are
//! flagged with `NodeData::error` and the walk continues.
//!
//! # Size policy (the two backends)
//!
//! Logical size is the data size on both backends. Hardlinks are charged once (to the first name in
//! traversal order) on both: via `st_dev`/`st_ino` on Unix and `GetFileInformationByHandle` through
//! `dirustat_ntfs::file_identity` on Windows.
//!
//! Allocated size ("size on disk") is an estimate on the portable path and exact on the MFT path:
//!
//! | file | portable | MFT |
//! |---|---|---|
//! | normal, non-resident | logical rounded up to `ScanOptions::cluster_size` | recorded allocation |
//! | empty | 0 | 0 |
//! | tiny, resident in its MFT record | one cluster (`std` cannot tell) | 0 (occupies no clusters) |
//! | sparse / compressed | logical rounded up (flags not inspected) | stored size, when recorded |
//!
//! The MFT figures are the truth on NTFS. Consequently the portable allocated total is an upper bound
//! that is higher than the MFT one by about one cluster per resident file, and by the holes of sparse
//! files and the savings of compressed ones. Closing this needs a per-file `GetCompressedFileSize`
//! query on Windows (cost comparable to the hardlink query), tracked as gap G-045.
//!
//! Backend choice: when the root is a whole volume (`C:` / `C:\`) and `ScanOptions::prefer_fast_path` is
//! set, [`start`] first tries the MFT path. Any failure (no administrator rights, not NTFS, damaged or
//! fragmented `$MFT`, I/O error, even a panic) is logged and the portable walk runs instead; the outcome
//! says which path produced the tree (`ScanOutcome::backend`) and why the fast path was abandoned
//! (`ScanOutcome::fallback_reason`). Sub-folders of a volume always use the portable walk.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::UNIX_EPOCH;

use dirustat_model::{NodeData, NodeId, NodeKind, Tree};

mod glob;
mod mft;

pub use glob::{
    Excludes, Glob, GlobError, MAX_EXCLUDES, MAX_EXCLUDES_TOTAL_CHARS, MAX_PATTERN_CHARS, excludes_total_len_ok,
};
#[doc(hidden)]
pub use mft::bench_scan_mft_mem;
pub use mft::{MftError, MftLimits, MftScan, MftStats, MftTimings};

/// Builds a tree with a single root and no disk walk — a skeleton for testing higher layers.
pub fn empty_tree(root: &str) -> Tree {
    let mut tree = Tree::new(root);
    tree.aggregate();
    tree
}

/// Whether the MFT fast path is available for the volume.
pub fn mft_available(volume: &str) -> bool {
    dirustat_ntfs::probe(volume) != dirustat_ntfs::MftSupport::Unavailable
}

/// Scan configuration.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Reader threads; `None` lets rayon pick.
    pub threads: Option<usize>,
    /// Allocation unit used to estimate allocated size (clamped to at least 1).
    pub cluster_size: u64,
    /// Try the NTFS MFT fast path first when the root is a whole volume; fall back to the portable
    /// walk on any failure.
    pub prefer_fast_path: bool,
    /// Exclusion patterns (see [`Excludes`]): matching files and directories are skipped entirely, a
    /// skipped directory is not descended and nothing in it is counted. The MFT fast path cannot
    /// filter, so a non-empty list makes the scan use the portable walk.
    pub exclude: Vec<String>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { threads: None, cluster_size: 4096, prefer_fast_path: true, exclude: Vec::new() }
    }
}

/// Which code path produced (or is producing) a tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanBackend {
    /// Directory walk through `std::fs`.
    #[default]
    Portable,
    /// Direct read of the NTFS master file table.
    Mft,
}

impl ScanBackend {
    /// Stable machine-readable name (`portable` / `mft`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Mft => "mft",
        }
    }

    fn to_raw(self) -> u8 {
        match self {
            Self::Portable => 0,
            Self::Mft => 1,
        }
    }

    fn from_raw(raw: u8) -> Self {
        if raw == 1 { Self::Mft } else { Self::Portable }
    }
}

/// The drive-letter volume (`C:`) when `path` names exactly a volume root (`C:`, `c:\`, `C:/`).
pub fn volume_root_of(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    let rest: String = chars.collect();
    matches!(rest.as_str(), "" | "\\" | "/").then(|| format!("{}:", letter.to_ascii_uppercase()))
}

/// Errors that prevent a scan from starting or finishing.
#[derive(Debug)]
pub enum ScanError {
    /// The root cannot be read or is not a directory.
    Root(io::Error),
    /// The root exists but is not a directory.
    NotADirectory,
    /// The thread pool could not be created.
    Pool(String),
    /// The scan thread panicked (the session survives).
    Panicked,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(err) => write!(f, "cannot read scan root: {err}"),
            Self::NotADirectory => f.write_str("scan root is not a directory"),
            Self::Pool(msg) => write!(f, "cannot create scan thread pool: {msg}"),
            Self::Panicked => f.write_str("scan thread panicked"),
        }
    }
}

impl std::error::Error for ScanError {}

/// Live counters, readable while the scan runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub dirs: u64,
    pub files: u64,
    /// Sum of logical sizes seen so far (saturating).
    pub bytes: u64,
    pub errors: u64,
    /// The path that is running (or ran). `Mft` while the fast path is being tried; back to `Portable`
    /// if it fell back.
    pub backend: ScanBackend,
}

#[derive(Debug, Default)]
struct Counters {
    dirs: AtomicU64,
    files: AtomicU64,
    bytes: AtomicU64,
    errors: AtomicU64,
    backend: AtomicU8,
    fallback_reason: Mutex<Option<String>>,
}

impl Counters {
    /// Adds to the byte counter, saturating (hostile sizes must not wrap). Single writer per scan.
    fn add_bytes(&self, n: u64) {
        let total = self.bytes.load(Ordering::Relaxed).saturating_add(n);
        self.bytes.store(total, Ordering::Relaxed);
    }

    /// Overwrites the progress with exact final figures.
    fn store_totals(&self, dirs: u64, files: u64, bytes: u64, errors: u64) {
        self.dirs.store(dirs, Ordering::Relaxed);
        self.files.store(files, Ordering::Relaxed);
        self.bytes.store(bytes, Ordering::Relaxed);
        self.errors.store(errors, Ordering::Relaxed);
    }

    /// Clears the progress before another path starts over.
    fn reset(&self) {
        self.store_totals(0, 0, 0, 0);
    }

    fn set_backend(&self, backend: ScanBackend) {
        self.backend.store(backend.to_raw(), Ordering::Relaxed);
    }

    fn set_fallback_reason(&self, reason: Option<String>) {
        *self.fallback_reason.lock().unwrap_or_else(PoisonError::into_inner) = reason;
    }
}

/// Result of a finished (or cancelled) scan. The tree is aggregated.
#[derive(Debug)]
pub struct ScanOutcome {
    pub tree: Tree,
    /// `true` when the scan was cancelled; the tree is then partial.
    pub cancelled: bool,
    /// The path that produced the tree.
    pub backend: ScanBackend,
    /// Why the MFT fast path was abandoned for the portable walk; `None` when it was not tried or worked.
    pub fallback_reason: Option<String>,
    /// Counters and phase timings of the MFT pass, when it produced this tree.
    pub mft_stats: Option<MftStats>,
}

impl ScanOutcome {
    fn new(tree: Tree, cancelled: bool, backend: ScanBackend) -> Self {
        Self { tree, cancelled, backend, fallback_reason: None, mft_stats: None }
    }
}

/// Handle to a running scan.
#[derive(Debug)]
pub struct ScanHandle {
    cancel: Arc<AtomicBool>,
    counters: Arc<Counters>,
    thread: JoinHandle<Result<ScanOutcome, ScanError>>,
    /// The scan was configured with a non-empty exclusion list, so its tree omits entries that exist.
    used_exclusions: bool,
}

impl ScanHandle {
    /// Whether this scan skips entries on purpose (a non-empty exclusion list), which makes its
    /// tree incomplete compared with the disk.
    pub fn used_exclusions(&self) -> bool {
        self.used_exclusions
    }

    /// Asks the scan to stop; `join` then returns the partial tree.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    pub fn progress(&self) -> Progress {
        Progress {
            dirs: self.counters.dirs.load(Ordering::Relaxed),
            files: self.counters.files.load(Ordering::Relaxed),
            bytes: self.counters.bytes.load(Ordering::Relaxed),
            errors: self.counters.errors.load(Ordering::Relaxed),
            backend: ScanBackend::from_raw(self.counters.backend.load(Ordering::Relaxed)),
        }
    }

    /// Why the fast path was abandoned, once known (set before the portable walk starts).
    pub fn fallback_reason(&self) -> Option<String> {
        self.counters.fallback_reason.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Waits for the scan and returns its tree.
    pub fn join(self) -> Result<ScanOutcome, ScanError> {
        self.thread.join().map_err(|_| ScanError::Panicked)?
    }
}

/// One directory entry as read by a worker.
#[derive(Debug)]
struct Entry {
    name: String,
    kind: NodeKind,
    data: NodeData,
    /// `(device, inode)` of a file with more than one hard link, when the platform exposes it.
    link_id: Option<(u64, u64)>,
    /// The exact on-disk name when it is not valid Unicode (`name` then holds the U+FFFD form), so
    /// a sub-directory is read through its real path and not through the lossy spelling.
    raw_name: Option<std::ffi::OsString>,
}

/// What a worker reports about one directory.
#[derive(Debug)]
struct Listing {
    dir: NodeId,
    path: PathBuf,
    entries: Vec<Entry>,
    /// The directory (or some entry) could not be read.
    error: bool,
}

/// Starts a scan of `root` on a background thread. The root is checked first, so a missing or
/// non-directory root is reported at once.
pub fn start(root: &Path, opts: ScanOptions) -> Result<ScanHandle, ScanError> {
    let root = normalize_root(root);
    check_root(&root)?;
    spawn_scan(&root, opts, false)
}

/// Like [`start`] but returns immediately without touching the filesystem: the root check runs on
/// the scan thread and its failure is reported by [`ScanHandle::join`]. For callers that hold a
/// lock while starting a scan, because a dead network path can block `metadata` for a long time.
pub fn start_deferred(root: &Path, opts: ScanOptions) -> Result<ScanHandle, ScanError> {
    spawn_scan(&normalize_root(root), opts, true)
}

/// A bare drive (`C:`) means "the current directory of drive C" to the OS; a scan of a volume root must mean the
/// root itself, so a bare drive, a drive with a forward slash and a drive with a backslash all become
/// the drive with a trailing backslash. Other paths are returned unchanged.
fn normalize_root(root: &Path) -> PathBuf {
    match volume_root_of(&root.to_string_lossy()) {
        Some(volume) => PathBuf::from(format!("{volume}\\")),
        None => root.to_path_buf(),
    }
}

fn check_root(root: &Path) -> Result<(), ScanError> {
    let meta = fs::metadata(root).map_err(ScanError::Root)?;
    if !meta.is_dir() {
        return Err(ScanError::NotADirectory);
    }
    Ok(())
}

fn spawn_scan(root: &Path, opts: ScanOptions, check_in_thread: bool) -> Result<ScanHandle, ScanError> {
    let mut builder = rayon::ThreadPoolBuilder::new();
    if let Some(n) = opts.threads {
        builder = builder.num_threads(n.max(1));
    }
    let pool = builder.build().map_err(|e| ScanError::Pool(e.to_string()))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let counters = Arc::new(Counters::default());
    let root = root.to_path_buf();
    let used_exclusions = !opts.exclude.is_empty();
    let thread = {
        let (cancel, counters) = (Arc::clone(&cancel), Arc::clone(&counters));
        std::thread::spawn(move || {
            if check_in_thread {
                check_root(&root)?;
            }
            let volume = if opts.prefer_fast_path && opts.exclude.is_empty() {
                volume_root_of(&root.to_string_lossy())
            } else {
                None
            };
            Ok(match volume {
                Some(volume) => {
                    let label = root.to_string_lossy().into_owned();
                    let attempt = || mft::scan_volume(&volume, &label, &cancel, &counters);
                    run_auto(&pool, &root, &opts, &cancel, &counters, Some(&attempt))
                }
                None => run_auto(&pool, &root, &opts, &cancel, &counters, None),
            })
        })
    };
    Ok(ScanHandle { cancel, counters, thread, used_exclusions })
}

/// The MFT attempt of [`run_auto`]; injectable so fallbacks are testable without a real volume.
type FastAttempt<'a> = &'a dyn Fn() -> Result<mft::MftScan, MftError>;

/// Runs the MFT attempt (when given) and falls back to the portable walk on any failure.
fn run_auto(
    pool: &rayon::ThreadPool,
    root: &Path,
    opts: &ScanOptions,
    cancel: &Arc<AtomicBool>,
    counters: &Arc<Counters>,
    attempt: Option<FastAttempt<'_>>,
) -> ScanOutcome {
    let mut reason = None;
    if let Some(attempt) = attempt {
        counters.set_backend(ScanBackend::Mft);
        match panic::catch_unwind(AssertUnwindSafe(attempt)) {
            Ok(Ok(scan)) => return scan.outcome,
            Ok(Err(err)) => reason = Some(err.to_string()),
            Err(_) => reason = Some("MFT scan panicked".to_owned()),
        }
        tracing::warn!(reason = reason.as_deref(), "MFT fast path failed, falling back to the portable walk");
        counters.reset();
        counters.set_backend(ScanBackend::Portable);
        counters.set_fallback_reason(reason.clone());
    }
    let mut outcome = run(pool, root, opts, cancel, counters);
    outcome.fallback_reason = reason;
    outcome
}

fn run(
    pool: &rayon::ThreadPool,
    root: &Path,
    opts: &ScanOptions,
    cancel: &Arc<AtomicBool>,
    counters: &Arc<Counters>,
) -> ScanOutcome {
    let mut tree = Tree::new(&root.to_string_lossy());
    let cluster = opts.cluster_size.max(1);
    let excludes = Arc::new(Excludes::new(&opts.exclude));
    let (tx, rx) = mpsc::channel::<Listing>();
    let mut pending = 0usize;
    let spawn = |dir: NodeId, path: PathBuf, pending: &mut usize| {
        *pending += 1;
        let (tx, cancel, excludes) = (tx.clone(), Arc::clone(cancel), Arc::clone(&excludes));
        pool.spawn(move || {
            let fallback = path.clone();
            let listing = panic::catch_unwind(AssertUnwindSafe(|| read_dir(dir, path, cluster, &cancel, &excludes)))
                .unwrap_or_else(|_| Listing { dir, path: fallback, entries: Vec::new(), error: true });
            // The receiver outlives every task; a failed send means the builder is gone.
            let _ = tx.send(listing);
        });
    };
    counters.dirs.fetch_add(1, Ordering::Relaxed);
    spawn(tree.root(), root.to_path_buf(), &mut pending);

    let mut seen_links = HashSet::new();
    while pending > 0 {
        let Ok(listing) = rx.recv() else { break };
        pending -= 1;
        let subdirs = apply(&mut tree, &mut seen_links, listing, counters);
        if !cancel.load(Ordering::Relaxed) {
            for (dir, path) in subdirs {
                spawn(dir, path, &mut pending);
            }
        }
    }
    tree.aggregate();
    ScanOutcome::new(tree, cancel.load(Ordering::Relaxed), ScanBackend::Portable)
}

/// Adds the entries of a listing to the tree; returns the sub-directories still to be read.
fn apply(
    tree: &mut Tree,
    seen_links: &mut HashSet<(u64, u64)>,
    listing: Listing,
    counters: &Counters,
) -> Vec<(NodeId, PathBuf)> {
    let mut subdirs = Vec::new();
    if listing.error {
        let _ = tree.mark_error(listing.dir);
        counters.errors.fetch_add(1, Ordering::Relaxed);
    }
    for entry in listing.entries {
        let mut data = entry.data;
        // A hardlinked file is counted once: later names of the same file keep a node but no size.
        if entry.link_id.is_some_and(|id| !seen_links.insert(id)) {
            data.logical_size = 0;
            data.allocated_size = 0;
            data.attributes |= dirustat_model::ATTR_HARDLINK_DUPLICATE;
        }
        let Ok(id) = tree.add_node(listing.dir, &entry.name, entry.kind, data) else {
            // Node limit reached: stop adding, the rest of the disk is not representable.
            counters.errors.fetch_add(1, Ordering::Relaxed);
            break;
        };
        if data.error {
            counters.errors.fetch_add(1, Ordering::Relaxed);
        }
        match entry.kind {
            NodeKind::Dir => {
                counters.dirs.fetch_add(1, Ordering::Relaxed);
                let child = match &entry.raw_name {
                    Some(raw) => listing.path.join(raw),
                    None => listing.path.join(&entry.name),
                };
                subdirs.push((id, child));
            }
            NodeKind::File | NodeKind::Link => {
                counters.files.fetch_add(1, Ordering::Relaxed);
                let total = counters.bytes.load(Ordering::Relaxed).saturating_add(data.logical_size);
                counters.bytes.store(total, Ordering::Relaxed);
            }
        }
    }
    subdirs
}

fn read_dir(dir: NodeId, path: PathBuf, cluster: u64, cancel: &AtomicBool, excludes: &Excludes) -> Listing {
    let mut listing = Listing { dir, path, entries: Vec::new(), error: false };
    if cancel.load(Ordering::Relaxed) {
        return listing;
    }
    let iter = match fs::read_dir(&listing.path) {
        Ok(iter) => iter,
        Err(_) => {
            listing.error = true;
            return listing;
        }
    };
    for item in iter {
        let Ok(item) = item else {
            listing.error = true;
            continue;
        };
        let raw_name = item.file_name();
        // A name that is not valid Unicode (a lone surrogate on Windows) is stored with U+FFFD and flagged,
        // so two distinct on-disk names that collapse to one string are never mistaken for each other.
        let lossy = raw_name.to_str().is_none();
        let name = raw_name.to_string_lossy().into_owned();
        if !excludes.is_empty() && excludes.is_excluded(&name, &item.path()) {
            continue;
        }
        let mut entry = match item.metadata() {
            Ok(meta) => entry_from(name, lossy, &item.path(), &meta, cluster),
            Err(_) => Entry {
                name,
                kind: NodeKind::File,
                data: NodeData { error: true, name_lossy: lossy, ..NodeData::default() },
                link_id: None,
                raw_name: None,
            },
        };
        entry.raw_name = lossy.then_some(raw_name);
        listing.entries.push(entry);
    }
    listing.entries.sort_by(|a, b| a.name.cmp(&b.name));
    listing
}

fn entry_from(name: String, name_lossy: bool, path: &Path, meta: &fs::Metadata, cluster: u64) -> Entry {
    let attributes = attributes_of(meta);
    let kind = if meta.file_type().is_symlink() || is_reparse_point(attributes) {
        NodeKind::Link
    } else if meta.is_dir() {
        NodeKind::Dir
    } else {
        NodeKind::File
    };
    let mut logical = if kind == NodeKind::File { meta.len() } else { 0 };
    let mut mtime = mtime_of(meta);
    // An empty file adds nothing to any total, so it never needs the (on Windows costly) identity query.
    let link_id = if kind == NodeKind::File && logical > 0 { link_id_of(meta, path) } else { None };
    // G-065 (QA-149): NTFS refreshes the size and time copies in a directory listing only for the
    // hard-link name that was used for a write, so a name of a multi-link file can list stale values.
    // The destructive commands compare the node with the file itself (`fsops::verify_matches`) and
    // stay strict (a size or time mismatch is `stale_tree`, never tolerated). The scan therefore
    // reads the file itself for the names of multi-link files, which keeps the tree and that
    // re-check in agreement and shows the real size. Residual: a file that was empty in the listing
    // gets no identity query (it would cost an open per empty file) and stays stale until a rescan;
    // a destructive command on it is refused as `stale_tree`, which is the safe outcome.
    if link_id.is_some()
        && let Ok(fresh) = fs::symlink_metadata(path)
        && fresh.is_file()
    {
        logical = fresh.len();
        mtime = mtime_of(&fresh);
    }
    let data = NodeData {
        logical_size: logical,
        allocated_size: round_up(logical, cluster),
        mtime,
        attributes,
        error: false,
        name_lossy,
    };
    Entry { name, kind, data, link_id, raw_name: None }
}

/// Rounds `size` up to a multiple of `cluster` (saturating); zero stays zero.
fn round_up(size: u64, cluster: u64) -> u64 {
    size.div_ceil(cluster.max(1)).saturating_mul(cluster.max(1))
}

fn mtime_of(meta: &fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok(),
        Err(before) => i64::try_from(before.duration().as_secs()).ok().map(i64::wrapping_neg),
    }
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

fn is_reparse_point(attributes: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(windows)]
fn attributes_of(meta: &fs::Metadata) -> u32 {
    std::os::windows::fs::MetadataExt::file_attributes(meta)
}

#[cfg(not(windows))]
fn attributes_of(_meta: &fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn link_id_of(meta: &fs::Metadata, _path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    (meta.nlink() > 1).then(|| (meta.dev(), meta.ino()))
}

/// `std` does not expose the file index on Windows without nightly features, so ask the safe wrapper in
/// `dirustat-ntfs`. It opens every regular file once (attribute access only); only files with more than
/// one link get an id.
#[cfg(not(unix))]
fn link_id_of(_meta: &fs::Metadata, path: &Path) -> Option<(u64, u64)> {
    let identity = dirustat_ntfs::file_identity(path)?;
    (identity.links > 1).then_some((identity.volume_serial, identity.file_index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::SizeMode;

    fn write(path: &Path, size: u64) {
        fs::File::create(path).unwrap().set_len(size).unwrap();
    }

    fn names(tree: &Tree, id: NodeId) -> Vec<String> {
        let mut out: Vec<String> = tree.children(id).filter_map(|c| tree.get(c).map(|n| n.name().to_owned())).collect();
        out.sort();
        out
    }

    fn scan(path: &Path) -> ScanOutcome {
        start(path, ScanOptions::default()).unwrap().join().unwrap()
    }

    #[test]
    fn empty_tree_has_only_root() {
        let t = empty_tree("C:\\");
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(t.root()).map(|n| n.kind()), Some(NodeKind::Dir));
        // availability of a real volume depends on elevation (CI runners are admins), so probe a bad name
        assert!(!mft_available("not a volume"));
    }

    #[test]
    fn scans_nested_tree_with_sizes_and_counts() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::create_dir(tmp.path().join("empty")).unwrap();
        write(&tmp.path().join("top.bin"), 10);
        write(&tmp.path().join("a/mid.bin"), 5000);
        write(&tmp.path().join("a/b/deep.bin"), 0);
        write(&tmp.path().join("файл_日本語.txt"), 1);

        let out = scan(tmp.path());
        assert!(!out.cancelled);
        let root = out.tree.get(out.tree.root()).unwrap();
        assert_eq!(root.total_size(SizeMode::Logical), 5011);
        // 10 -> 4096, 5000 -> 8192, 0 -> 0, 1 -> 4096
        assert_eq!(root.total_size(SizeMode::Allocated), 4096 + 8192 + 4096);
        assert_eq!((root.dir_count(), root.file_count(), root.error_count()), (4, 4, 0));
        assert!(names(&out.tree, out.tree.root()).contains(&"файл_日本語.txt".to_owned()));
    }

    #[test]
    fn progress_matches_final_tree() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("d")).unwrap();
        write(&tmp.path().join("d/f"), 100);
        let handle = start(tmp.path(), ScanOptions::default()).unwrap();
        while !handle.is_finished() {
            std::thread::yield_now();
        }
        let progress = handle.progress();
        assert_eq!(progress, Progress { dirs: 2, files: 1, bytes: 100, errors: 0, backend: ScanBackend::Portable });
        handle.join().unwrap();
    }

    #[test]
    fn deferred_start_reports_a_bad_root_through_join() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = start_deferred(&tmp.path().join("nope"), ScanOptions::default()).unwrap();
        assert!(matches!(handle.join(), Err(ScanError::Root(_))));
        let file = tmp.path().join("f");
        write(&file, 1);
        let handle = start_deferred(&file, ScanOptions::default()).unwrap();
        assert!(matches!(handle.join(), Err(ScanError::NotADirectory)));
        let handle = start_deferred(tmp.path(), ScanOptions::default()).unwrap();
        assert!(handle.join().is_ok());
    }

    #[test]
    fn rejects_missing_root_and_file_root() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(start(&tmp.path().join("nope"), ScanOptions::default()), Err(ScanError::Root(_))));
        let file = tmp.path().join("f");
        write(&file, 1);
        assert!(matches!(start(&file, ScanOptions::default()), Err(ScanError::NotADirectory)));
    }

    #[test]
    fn cancel_returns_a_valid_partial_tree() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..50 {
            fs::create_dir_all(tmp.path().join(format!("d{i}/sub"))).unwrap();
            write(&tmp.path().join(format!("d{i}/sub/f")), 1);
        }
        let handle = start(tmp.path(), ScanOptions { threads: Some(2), ..ScanOptions::default() }).unwrap();
        handle.cancel();
        let out = handle.join().unwrap();
        assert!(out.cancelled);
        assert!(out.tree.len() <= 1 + 50 * 3);
    }

    #[test]
    fn links_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("real")).unwrap();
        write(&tmp.path().join("real/f"), 10);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(tmp.path(), tmp.path().join("real/loop")).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(tmp.path(), tmp.path().join("real/loop")).is_ok();
        if !made {
            return; // no privilege to create symlinks on this machine
        }
        let out = scan(tmp.path());
        let real = out.tree.children(out.tree.root()).next().unwrap();
        let link = out.tree.children(real).find(|c| out.tree.get(*c).map(|n| n.name()) == Some("loop")).unwrap();
        assert_eq!(out.tree.get(link).unwrap().kind(), NodeKind::Link);
        assert_eq!(out.tree.children(link).count(), 0);
        assert_eq!(out.tree.len(), 4);
    }

    #[test]
    fn portable_allocated_size_policy_is_cluster_rounding() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("empty"), 0);
        write(&tmp.path().join("tiny"), 100);
        write(&tmp.path().join("exact"), 4096);
        write(&tmp.path().join("over"), 4097);
        let out =
            start(tmp.path(), ScanOptions { cluster_size: 4096, ..ScanOptions::default() }).unwrap().join().unwrap();
        let tree = &out.tree;
        let alloc = |name: &str| {
            let id = tree.children(tree.root()).find(|&c| tree.get(c).is_some_and(|n| n.name() == name)).unwrap();
            tree.get(id).unwrap().data().allocated_size
        };
        assert_eq!([alloc("empty"), alloc("tiny"), alloc("exact"), alloc("over")], [0, 4096, 4096, 8192]);
    }

    #[test]
    fn real_hardlinks_on_disk_are_counted_once() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.bin"), 1 << 20);
        fs::hard_link(tmp.path().join("a.bin"), tmp.path().join("b.bin")).unwrap();
        fs::hard_link(tmp.path().join("a.bin"), tmp.path().join("c.bin")).unwrap();
        write(&tmp.path().join("solo.bin"), 100);
        let out = scan(tmp.path());
        let root = out.tree.root();
        assert_eq!(out.tree.children(root).count(), 4);
        assert_eq!(out.tree.get(root).unwrap().total_size(SizeMode::Logical), (1 << 20) + 100);
    }

    /// G-065 (QA-149): a file written through one hard-link name lists stale size and time under
    /// the other names on NTFS; the tree must hold the real values for every name, in either
    /// scan order (`a.bin` sorts before `z.bin`, and each case is written through the other name).
    #[test]
    fn hardlinked_file_written_through_another_name_shows_real_size_and_time() {
        use std::io::Write;
        for (original, link, writer) in [("z.bin", "a.bin", "z.bin"), ("a.bin", "b.bin", "a.bin")] {
            let tmp = tempfile::tempdir().unwrap();
            write(&tmp.path().join(original), 10);
            if fs::hard_link(tmp.path().join(original), tmp.path().join(link)).is_err() {
                return; // hardlinks unavailable on this file system
            }
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let mut f = fs::OpenOptions::new().append(true).open(tmp.path().join(writer)).unwrap();
            f.write_all(b"12345").unwrap();
            drop(f);
            let real = fs::metadata(tmp.path().join(writer)).unwrap();
            let out = scan(tmp.path());
            let tree = &out.tree;
            let mut nonzero = 0;
            for name in [original, link] {
                let id = tree.children(tree.root()).find(|&c| tree.get(c).is_some_and(|n| n.name() == name)).unwrap();
                let data = tree.get(id).unwrap().data();
                assert_eq!(data.mtime, mtime_of(&real), "{name}: time of the file itself");
                if data.logical_size != 0 {
                    nonzero += 1;
                    assert_eq!(data.logical_size, 15, "{name}: size of the file itself");
                }
            }
            assert_eq!(nonzero, 1, "the file is charged once");
            assert_eq!(tree.get(tree.root()).unwrap().total_size(SizeMode::Logical), 15);
        }
    }

    #[test]
    fn hardlinked_file_is_counted_once() {
        let mut tree = Tree::new("r");
        let mut seen = HashSet::new();
        let mk = |name: &str| Entry {
            name: name.to_owned(),
            kind: NodeKind::File,
            data: NodeData::with_size(100),
            link_id: Some((1, 7)),
            raw_name: None,
        };
        let listing = Listing {
            dir: tree.root(),
            path: PathBuf::from("r"),
            entries: vec![mk("a"), mk("b"), mk("c")],
            error: false,
        };
        apply(&mut tree, &mut seen, listing, &Counters::default());
        tree.aggregate();
        let root = tree.get(tree.root()).unwrap();
        assert_eq!(root.total_size(SizeMode::Logical), 100);
        assert_eq!(root.file_count(), 3);
    }

    #[test]
    fn unreadable_directory_is_flagged_and_walk_continues() {
        let mut tree = Tree::new("r");
        let mut seen = HashSet::new();
        let dir = tree.add_child(tree.root(), "locked", NodeKind::Dir, 0).unwrap();
        let counters = Counters::default();
        let bad = Listing { dir, path: PathBuf::from("r/locked"), entries: Vec::new(), error: true };
        assert!(apply(&mut tree, &mut seen, bad, &counters).is_empty());
        tree.aggregate();
        assert_eq!(tree.get(tree.root()).unwrap().error_count(), 1);
        assert_eq!(counters.errors.load(Ordering::Relaxed), 1);
    }

    fn scan_excluding(path: &Path, patterns: &[&str]) -> ScanOutcome {
        let exclude = patterns.iter().map(|p| (*p).to_owned()).collect();
        start(path, ScanOptions { exclude, ..ScanOptions::default() }).unwrap().join().unwrap()
    }

    #[test]
    fn excluded_directories_are_not_descended_or_counted() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("keep/node_modules/deep")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
        write(&tmp.path().join("node_modules/pkg/big.bin"), 9000);
        write(&tmp.path().join("keep/node_modules/deep/x.bin"), 100);
        write(&tmp.path().join("keep/a.txt"), 10);
        write(&tmp.path().join("scratch.tmp"), 5);
        let out = scan_excluding(tmp.path(), &["node_modules", "*.tmp"]);
        assert!(!out.cancelled);
        let root = out.tree.get(out.tree.root()).unwrap();
        // Only `keep` and `keep/a.txt` remain besides the root.
        assert_eq!(out.tree.len(), 3);
        assert_eq!(root.total_size(SizeMode::Logical), 10);
        assert_eq!((root.dir_count(), root.file_count(), root.error_count()), (2, 1, 0));
        let all = scan(tmp.path());
        assert_eq!(all.tree.len(), 10);
    }

    #[test]
    fn path_patterns_and_hostile_lists_do_not_fail_the_scan() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/skipme")).unwrap();
        fs::create_dir_all(tmp.path().join("b/skipme")).unwrap();
        write(&tmp.path().join("a/skipme/f"), 1);
        write(&tmp.path().join("b/skipme/f"), 1);
        let out = scan_excluding(
            tmp.path(),
            &["*/a/skipme", "", "x\0y", &"*".repeat(5000), &format!("{}z", "*".repeat(900))],
        );
        // `a/skipme` is gone, `b/skipme` stays.
        assert_eq!(out.tree.len(), 1 + 2 + 2);
        assert_eq!(out.tree.get(out.tree.root()).unwrap().error_count(), 0);
    }

    /// A name that is not valid Unicode, and the literal U+FFFD name its lossy text equals.
    #[cfg(any(windows, unix))]
    fn lone_and_literal(stem: &str) -> (std::ffi::OsString, std::ffi::OsString) {
        #[cfg(windows)]
        let lone = {
            use std::os::windows::ffi::OsStringExt;
            let wide: Vec<u16> = stem.encode_utf16().chain([0xD800]).collect();
            std::ffi::OsString::from_wide(&wide)
        };
        #[cfg(unix)]
        let lone = {
            use std::os::unix::ffi::OsStringExt;
            let mut bytes = stem.as_bytes().to_vec();
            bytes.push(0xFF);
            std::ffi::OsString::from_vec(bytes)
        };
        (lone, std::ffi::OsString::from(format!("{stem}\u{FFFD}")))
    }

    /// A folder with a non-Unicode name is read through its real path: its children are its own,
    /// not those of a literal-U+FFFD twin that shares the lossy spelling.
    #[cfg(any(windows, unix))]
    #[test]
    fn non_unicode_folder_is_read_through_its_real_name() {
        let tmp = tempfile::tempdir().unwrap();
        let (lone, literal) = lone_and_literal("d");
        if fs::create_dir(tmp.path().join(&lone)).is_err() || fs::create_dir(tmp.path().join(&literal)).is_err() {
            return; // this file system cannot hold such names
        }
        write(&tmp.path().join(&lone).join("only_in_lone.bin"), 10);
        write(&tmp.path().join(&literal).join("only_in_literal.bin"), 20);
        let out = scan(tmp.path());
        let tree = &out.tree;
        let of = |lossy: bool| {
            let dir = tree
                .children(tree.root())
                .find(|&c| tree.get(c).is_some_and(|n| n.data().name_lossy == lossy))
                .unwrap();
            names(tree, dir)
        };
        assert_eq!(of(true), ["only_in_lone.bin"]);
        assert_eq!(of(false), ["only_in_literal.bin"]);
        assert_eq!(out.tree.get(out.tree.root()).unwrap().error_count(), 0);
    }

    #[test]
    fn round_up_is_saturating_and_zero_safe() {
        assert_eq!(round_up(0, 4096), 0);
        assert_eq!(round_up(1, 4096), 4096);
        assert_eq!(round_up(4096, 4096), 4096);
        assert_eq!(round_up(u64::MAX, 4096), u64::MAX);
        assert_eq!(round_up(5, 0), 5);
    }
}

#[cfg(test)]
mod fallback_tests;
