//! Duplicate finder core (G-020): read-only, cancellable, bounded memory.
//!
//! The pipeline narrows candidates stage by stage so most files are never read at all:
//!
//! 1. **sizes**: regular files of the tree grouped by logical size; zero-size files, hardlink
//!    names that carry no size (`ATTR_HARDLINK_DUPLICATE`), errored nodes and links are dropped;
//! 2. **partial hash**: first and last 64 KiB (the whole file when it is at most 128 KiB);
//! 3. **full hash**: BLAKE3 over the whole content of the files whose partial hashes collide;
//! 4. **verify** (optional): byte-by-byte comparison inside every hash group;
//! 5. **identity**: names of one file (hardlinks that the scan did not flag) collapse into one.
//!
//! Every file problem (vanished, unreadable, changed size, reparse point, locked) costs that one
//! file its place in the result and is counted; the run continues. Nothing is ever written.
//! Memory is `O(files)` for ids and sizes plus one 256 KiB buffer per worker; paths are built on
//! demand from the tree and only the requested number of groups is kept.

use dirustat_model::{ATTR_HARDLINK_DUPLICATE, NodeId, NodeKind, SizeMode, Tree};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::thread::JoinHandle;

/// Bytes hashed from the head and from the tail by the partial hash.
pub const CHUNK: u64 = 64 * 1024;
/// Files up to this size are hashed completely by the partial stage.
const WHOLE_FILE_MAX: u64 = 2 * CHUNK;
const BUF_LEN: usize = 256 * 1024;
const MAX_WORKERS: usize = 4;
/// Messages kept in [`Outcome::problem_samples`].
pub const MAX_PROBLEM_SAMPLES: usize = 50;

/// What to look for.
#[derive(Clone, Debug)]
pub struct Params {
    /// Smallest logical size considered (zero-size files are never duplicates).
    pub min_size: u64,
    /// Only files under this path (scan-root spelling, case-insensitive on Windows).
    pub path_prefix: Option<String>,
    /// Compare group members byte by byte after the hash match.
    pub verify: bool,
    /// Number of groups kept, largest wasted space first. Totals stay exact.
    pub keep: usize,
}

/// The stage a running job is in (`duplicates.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Collecting,
    PartialHash,
    FullHash,
    Verify,
    Finishing,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Collecting => "collecting",
            Self::PartialHash => "partial_hash",
            Self::FullHash => "full_hash",
            Self::Verify => "verify",
            Self::Finishing => "finishing",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Collecting => 0,
            Self::PartialHash => 1,
            Self::FullHash => 2,
            Self::Verify => 3,
            Self::Finishing => 4,
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Collecting,
            1 => Self::PartialHash,
            2 => Self::FullHash,
            3 => Self::Verify,
            _ => Self::Finishing,
        }
    }
}

/// Live counters of a running job; cheap to read from any thread.
#[derive(Debug, Default)]
pub struct Progress {
    stage: AtomicU8,
    /// Items of the current stage, and how many are done.
    total: AtomicU64,
    done: AtomicU64,
    bytes_read: AtomicU64,
    problems: AtomicU64,
}

/// A copy of [`Progress`] at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub stage: Stage,
    pub stage_total: u64,
    pub stage_done: u64,
    pub bytes_read: u64,
    pub problems: u64,
}

impl Progress {
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            stage: Stage::from_code(self.stage.load(Ordering::Relaxed)),
            stage_total: self.total.load(Ordering::Relaxed),
            stage_done: self.done.load(Ordering::Relaxed),
            bytes_read: self.bytes_read.load(Ordering::Relaxed),
            problems: self.problems.load(Ordering::Relaxed),
        }
    }

    fn enter(&self, stage: Stage, total: usize) {
        self.stage.store(stage.code(), Ordering::Relaxed);
        self.total.store(u64::try_from(total).unwrap_or(u64::MAX), Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
    }
}

/// Files with identical content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Logical size of each member.
    pub size: u64,
    /// Members in tree order; at least two.
    pub files: Vec<NodeId>,
}

impl Group {
    /// Space a user gets back by keeping one copy.
    pub fn wasted(&self) -> u64 {
        let extra = u64::try_from(self.files.len().saturating_sub(1)).unwrap_or(u64::MAX);
        self.size.saturating_mul(extra)
    }
}

/// What a finished job found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The job was cancelled; every other field is then partial and must not be shown.
    pub cancelled: bool,
    /// The largest `keep` groups by wasted space.
    pub groups: Vec<Group>,
    /// Groups found (>= `groups.len()`).
    pub total_groups: u64,
    /// Files in all groups.
    pub total_files: u64,
    /// Wasted bytes over all groups.
    pub wasted_bytes: u64,
    /// Files that shared their size with another file (what stage 2 started with).
    pub candidates: u64,
    /// Names dropped because they are another name of a file already in the group.
    pub hardlinks_collapsed: u64,
    /// Bytes read from disk.
    pub bytes_read: u64,
    /// Files skipped because of an error.
    pub problems: u64,
    /// The first [`MAX_PROBLEM_SAMPLES`] problems, `path: reason`.
    pub problem_samples: Vec<String>,
}

/// A running duplicate search on its own thread.
#[derive(Debug)]
pub struct Job {
    handle: JoinHandle<Outcome>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
}

impl Job {
    /// Starts the search on `tree` (shared, read-only).
    pub fn start(tree: Arc<Tree>, params: Params) -> io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let (c, p) = (Arc::clone(&cancel), Arc::clone(&progress));
        let handle = std::thread::Builder::new()
            .name("dirustat-duplicates".to_owned())
            .spawn(move || find(&tree, &params, &c, &p))?;
        Ok(Self { handle, cancel, progress })
    }

    /// Asks the job to stop; it ends within one buffer read per worker.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn progress(&self) -> ProgressSnapshot {
        self.progress.snapshot()
    }

    /// Waits for the job. `Err` carries a panic message (the job never panics by design).
    pub fn join(self) -> Result<Outcome, String> {
        self.handle.join().map_err(|payload| {
            if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "non-string panic payload".to_owned()
            }
        })
    }
}

/// One file under consideration.
#[derive(Clone, Copy, Debug)]
struct Item {
    id: NodeId,
    size: u64,
}

/// Why a file dropped out.
#[derive(Debug)]
enum Fail {
    Cancelled,
    Bad(String),
}

type Digest = [u8; 32];

/// Result of the partial stage.
#[derive(Clone, Copy, Debug)]
struct Partial {
    hash: Digest,
    /// The hash covers the whole file, so no full hash is needed.
    complete: bool,
}

struct Ctx<'a> {
    tree: &'a Tree,
    cancel: &'a AtomicBool,
    progress: &'a Progress,
}

impl Ctx<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn path(&self, id: NodeId) -> Result<String, Fail> {
        crate::cmd::tree_node_path(self.tree, id).ok_or_else(|| Fail::Bad("node has no path".to_owned()))
    }
}

/// The whole pipeline. Never panics; a cancel returns early with `cancelled` set.
pub fn find(tree: &Tree, params: &Params, cancel: &AtomicBool, progress: &Progress) -> Outcome {
    let ctx = Ctx { tree, cancel, progress };
    let mut problems = Problems::default();
    let cancelled = |problems: &Problems| Outcome {
        cancelled: true,
        problems: problems.count,
        bytes_read: progress.bytes_read.load(Ordering::Relaxed),
        ..Outcome::default()
    };

    progress.enter(Stage::Collecting, 0);
    let Some(items) = size_candidates(&ctx, params) else { return cancelled(&problems) };
    let candidates = u64::try_from(items.len()).unwrap_or(u64::MAX);

    // Stage 2: partial hashes.
    progress.enter(Stage::PartialHash, items.len());
    let partials = run_parallel(&ctx, &items, partial_hash);
    if ctx.cancelled() {
        return cancelled(&problems);
    }
    let mut keyed: Vec<((u64, Digest), Item, bool)> = Vec::new();
    for (item, result) in items.iter().zip(partials) {
        match result {
            Some(Ok(p)) => keyed.push(((item.size, p.hash), *item, p.complete)),
            Some(Err(Fail::Bad(why))) => problems.add(&ctx, *item, &why),
            Some(Err(Fail::Cancelled)) | None => {}
        }
    }
    let narrowed = collisions(keyed);

    // Stage 3: full hashes for what the partial hash could not settle.
    let (complete, open): (Vec<_>, Vec<_>) = narrowed.into_iter().partition(|(_, _, complete)| *complete);
    let open_items: Vec<Item> = open.iter().map(|(_, item, _)| *item).collect();
    progress.enter(Stage::FullHash, open_items.len());
    let fulls = run_parallel(&ctx, &open_items, full_hash);
    if ctx.cancelled() {
        return cancelled(&problems);
    }
    let mut keyed: Vec<((u64, Digest), Item, bool)> = complete;
    for (item, result) in open_items.iter().zip(fulls) {
        match result {
            Some(Ok(hash)) => keyed.push(((item.size, hash), *item, true)),
            Some(Err(Fail::Bad(why))) => problems.add(&ctx, *item, &why),
            Some(Err(Fail::Cancelled)) | None => {}
        }
    }
    let mut groups: Vec<Vec<Item>> = group_runs(keyed);

    // Stage 4: byte comparison inside each hash group.
    if params.verify {
        let total = groups.iter().map(Vec::len).sum();
        progress.enter(Stage::Verify, total);
        let mut verified = Vec::new();
        for group in groups {
            let Some(parts) = verify_group(&ctx, group, &mut problems) else { return cancelled(&problems) };
            verified.extend(parts.into_iter().filter(|g| g.len() >= 2));
        }
        groups = verified;
    }

    // Stage 5: names of one file collapse.
    progress.enter(Stage::Finishing, groups.len());
    let Some((groups, collapsed)) = collapse_identities(&ctx, groups, &mut problems) else {
        return cancelled(&problems);
    };
    assemble(groups, params.keep, candidates, collapsed, &ctx, problems)
}

/// Problem counting with bounded samples.
#[derive(Default)]
struct Problems {
    count: u64,
    samples: Vec<String>,
}

impl Problems {
    fn add(&mut self, ctx: &Ctx<'_>, item: Item, why: &str) {
        self.count = self.count.saturating_add(1);
        ctx.progress.problems.fetch_add(1, Ordering::Relaxed);
        if self.samples.len() < MAX_PROBLEM_SAMPLES {
            let path = crate::cmd::tree_node_path(ctx.tree, item.id).unwrap_or_else(|| "<unknown>".to_owned());
            self.samples.push(format!("{path}: {why}"));
        }
    }
}

fn assemble(
    mut groups: Vec<Vec<Item>>,
    keep: usize,
    candidates: u64,
    collapsed: u64,
    ctx: &Ctx<'_>,
    problems: Problems,
) -> Outcome {
    let mut found: Vec<Group> = groups
        .drain(..)
        .filter(|g| g.len() >= 2)
        .filter_map(|mut g| {
            g.sort_by_key(|i| i.id);
            let size = g.first()?.size;
            Some(Group { size, files: g.iter().map(|i| i.id).collect() })
        })
        .collect();
    let total_groups = u64::try_from(found.len()).unwrap_or(u64::MAX);
    let mut total_files = 0u64;
    let mut wasted_bytes = 0u64;
    for g in &found {
        total_files = total_files.saturating_add(u64::try_from(g.files.len()).unwrap_or(u64::MAX));
        wasted_bytes = wasted_bytes.saturating_add(g.wasted());
    }
    found.sort_by(|a, b| {
        b.wasted()
            .cmp(&a.wasted())
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.files.first().cmp(&b.files.first()))
    });
    found.truncate(keep);
    Outcome {
        cancelled: false,
        groups: found,
        total_groups,
        total_files,
        wasted_bytes,
        candidates,
        hardlinks_collapsed: collapsed,
        bytes_read: ctx.progress.bytes_read.load(Ordering::Relaxed),
        problems: problems.count,
        problem_samples: problems.samples,
    }
}

// ---------------------------------------------------------------- stage 1: sizes

/// Normalised form for prefix tests: one separator, case folded where the file system folds.
fn fold(path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let unified: String = if cfg!(windows) { path.replace('/', "\\") } else { path.to_owned() };
    let trimmed = unified.trim_end_matches(sep);
    if cfg!(windows) { trimmed.to_ascii_lowercase() } else { trimmed.to_owned() }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Relation {
    Inside,
    Ancestor,
    Outside,
}

/// How the (folded) directory `dir` relates to the (folded) prefix, at component boundaries.
fn relation(dir: &str, prefix: &str) -> Relation {
    let sep = std::path::MAIN_SEPARATOR;
    if prefix.is_empty() {
        return Relation::Inside;
    }
    if let Some(rest) = dir.strip_prefix(prefix)
        && (rest.is_empty() || rest.starts_with(sep))
    {
        return Relation::Inside;
    }
    if let Some(rest) = prefix.strip_prefix(dir)
        && rest.starts_with(sep)
    {
        return Relation::Ancestor;
    }
    Relation::Outside
}

/// Files that share their logical size with at least one other eligible file, sorted by size
/// (largest first) then id. `None` when cancelled.
fn size_candidates(ctx: &Ctx<'_>, params: &Params) -> Option<Vec<Item>> {
    let tree = ctx.tree;
    let min = params.min_size.max(1);
    let prefix = params.path_prefix.as_deref().map(fold);
    let sep = std::path::MAIN_SEPARATOR;
    let mut files: Vec<Item> = Vec::new();
    // Directories to visit; the folded path is only tracked when a prefix restricts the walk.
    let root = tree.root();
    let root_path = prefix.as_ref().map(|_| fold(tree.get(root).map_or("", |n| n.name())));
    let mut stack: Vec<(NodeId, Option<String>, bool)> = vec![(root, root_path, prefix.is_none())];
    let mut visited = 0u64;
    while let Some((dir, path, inside)) = stack.pop() {
        visited += 1;
        if visited.is_multiple_of(4096) && ctx.cancelled() {
            return None;
        }
        for child in tree.children(dir) {
            let Some(node) = tree.get(child) else { continue };
            match node.kind() {
                NodeKind::Dir => {
                    let Some(prefix) = prefix.as_deref() else {
                        stack.push((child, None, true));
                        continue;
                    };
                    let child_path = format!("{}{sep}{}", path.as_deref().unwrap_or_default(), fold(node.name()));
                    match relation(&child_path, prefix) {
                        Relation::Outside => {}
                        rel => stack.push((child, Some(child_path), rel == Relation::Inside)),
                    }
                }
                NodeKind::File if inside => {
                    let data = node.data();
                    let size = node.own_size(SizeMode::Logical);
                    if !data.error && data.attributes & ATTR_HARDLINK_DUPLICATE == 0 && size >= min {
                        files.push(Item { id: child, size });
                    }
                }
                NodeKind::File | NodeKind::Link => {}
            }
        }
    }
    files.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.id.cmp(&b.id)));
    // Keep only runs of equal size with at least two members.
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < files.len() {
        let size = files.get(start).map_or(0, |i| i.size);
        let end = start + files.get(start..).map_or(0, |rest| rest.iter().take_while(|i| i.size == size).count());
        if end - start >= 2
            && let Some(run) = files.get(start..end)
        {
            out.extend_from_slice(run);
        }
        start = end.max(start + 1);
    }
    Some(out)
}

/// Sorts by key, keeps runs of equal keys with at least two members and flattens them.
fn collisions(keyed: Vec<((u64, Digest), Item, bool)>) -> Vec<((u64, Digest), Item, bool)> {
    let mut keyed = keyed;
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < keyed.len() {
        let key = keyed.get(start).map(|k| k.0);
        let end = start + keyed.get(start..).map_or(0, |rest| rest.iter().take_while(|k| Some(k.0) == key).count());
        if end - start >= 2
            && let Some(run) = keyed.get(start..end)
        {
            out.extend_from_slice(run);
        }
        start = end.max(start + 1);
    }
    out
}

/// Like [`collisions`], but returns the runs as groups.
fn group_runs(keyed: Vec<((u64, Digest), Item, bool)>) -> Vec<Vec<Item>> {
    let flat = collisions(keyed);
    let mut groups: Vec<Vec<Item>> = Vec::new();
    let mut last: Option<(u64, Digest)> = None;
    for (key, item, _) in flat {
        if last == Some(key) {
            if let Some(g) = groups.last_mut() {
                g.push(item);
            }
        } else {
            groups.push(vec![item]);
            last = Some(key);
        }
    }
    groups
}

// ---------------------------------------------------------------- parallel helper

fn worker_count(items: usize) -> usize {
    let cpus = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);
    cpus.clamp(1, MAX_WORKERS).min(items.max(1))
}

/// Runs `f` for every item on a few worker threads. The result at index `i` belongs to `items[i]`;
/// `None` when the item was not processed (cancel) or its worker died.
fn run_parallel<R: Send>(
    ctx: &Ctx<'_>,
    items: &[Item],
    f: impl Fn(&Ctx<'_>, Item) -> Result<R, Fail> + Sync,
) -> Vec<Option<Result<R, Fail>>> {
    let next = AtomicU64::new(0);
    let mut results: Vec<Option<Result<R, Fail>>> = Vec::new();
    results.resize_with(items.len(), || None);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count(items.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut local: Vec<(usize, Result<R, Fail>)> = Vec::new();
                    loop {
                        if ctx.cancelled() {
                            break;
                        }
                        let i = usize::try_from(next.fetch_add(1, Ordering::Relaxed)).unwrap_or(usize::MAX);
                        let Some(&item) = items.get(i) else { break };
                        let r = f(ctx, item);
                        ctx.progress.done.fetch_add(1, Ordering::Relaxed);
                        local.push((i, r));
                    }
                    local
                })
            })
            .collect();
        for handle in handles {
            for (i, r) in handle.join().unwrap_or_default() {
                if let Some(slot) = results.get_mut(i) {
                    *slot = Some(r);
                }
            }
        }
    });
    results
}

// ---------------------------------------------------------------- file access

/// Opens a regular file whose size is still `expected`. Reparse points and symlinks are refused
/// (never followed); a directory or device is not a regular file.
fn open_regular(path: &str, expected: u64) -> Result<File, Fail> {
    let p = Path::new(path);
    let meta = fs::symlink_metadata(p).map_err(|e| Fail::Bad(describe(&e)))?;
    if meta.file_type().is_symlink() || is_reparse(&meta) {
        return Err(Fail::Bad("reparse point (not followed)".to_owned()));
    }
    if !meta.is_file() {
        return Err(Fail::Bad("not a regular file".to_owned()));
    }
    let file = File::open(p).map_err(|e| Fail::Bad(describe(&e)))?;
    let opened = file.metadata().map_err(|e| Fail::Bad(describe(&e)))?;
    if !opened.is_file() {
        return Err(Fail::Bad("not a regular file".to_owned()));
    }
    if opened.len() != expected {
        return Err(Fail::Bad(format!("size changed since the scan ({} -> {} bytes)", expected, opened.len())));
    }
    Ok(file)
}

#[cfg(windows)]
fn is_reparse(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse(_: &fs::Metadata) -> bool {
    false
}

fn describe(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "vanished".to_owned(),
        io::ErrorKind::PermissionDenied => "access denied".to_owned(),
        _ => format!("read error: {e}"),
    }
}

/// Feeds exactly `len` bytes from the current position into `hasher`. A short read (the file
/// shrank) is a problem.
fn feed(ctx: &Ctx<'_>, file: &mut File, len: u64, buf: &mut [u8], hasher: &mut blake3::Hasher) -> Result<(), Fail> {
    let mut remaining = len;
    while remaining > 0 {
        if ctx.cancelled() {
            return Err(Fail::Cancelled);
        }
        let want = usize::try_from(remaining).map_or(buf.len(), |r| r.min(buf.len()));
        let Some(slice) = buf.get_mut(..want) else { return Err(Fail::Bad("internal buffer error".to_owned())) };
        let n = match file.read(slice) {
            Ok(0) => return Err(Fail::Bad("file shrank while it was read".to_owned())),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Fail::Bad(describe(&e))),
        };
        let Some(got) = slice.get(..n) else { return Err(Fail::Bad("internal buffer error".to_owned())) };
        hasher.update(got);
        let n64 = u64::try_from(n).unwrap_or(0);
        remaining = remaining.saturating_sub(n64);
        ctx.progress.bytes_read.fetch_add(n64, Ordering::Relaxed);
    }
    Ok(())
}

/// After a whole-file read: the file must end here and still have the size we expected.
fn confirm_unchanged(file: &mut File, expected: u64) -> Result<(), Fail> {
    let mut probe = [0u8; 1];
    match file.read(&mut probe) {
        Ok(0) => {}
        Ok(_) => return Err(Fail::Bad("file grew while it was read".to_owned())),
        Err(e) => return Err(Fail::Bad(describe(&e))),
    }
    match file.metadata() {
        Ok(m) if m.len() == expected => Ok(()),
        Ok(_) => Err(Fail::Bad("size changed while it was read".to_owned())),
        Err(e) => Err(Fail::Bad(describe(&e))),
    }
}

fn partial_hash(ctx: &Ctx<'_>, item: Item) -> Result<Partial, Fail> {
    let path = ctx.path(item.id)?;
    let mut file = open_regular(&path, item.size)?;
    let mut buf = vec![0u8; BUF_LEN];
    let mut hasher = blake3::Hasher::new();
    if item.size <= WHOLE_FILE_MAX {
        feed(ctx, &mut file, item.size, &mut buf, &mut hasher)?;
        confirm_unchanged(&mut file, item.size)?;
        return Ok(Partial { hash: *hasher.finalize().as_bytes(), complete: true });
    }
    feed(ctx, &mut file, CHUNK, &mut buf, &mut hasher)?;
    file.seek(SeekFrom::Start(item.size.saturating_sub(CHUNK))).map_err(|e| Fail::Bad(describe(&e)))?;
    feed(ctx, &mut file, CHUNK, &mut buf, &mut hasher)?;
    confirm_unchanged(&mut file, item.size)?;
    Ok(Partial { hash: *hasher.finalize().as_bytes(), complete: false })
}

fn full_hash(ctx: &Ctx<'_>, item: Item) -> Result<Digest, Fail> {
    let path = ctx.path(item.id)?;
    let mut file = open_regular(&path, item.size)?;
    let mut buf = vec![0u8; BUF_LEN];
    let mut hasher = blake3::Hasher::new();
    feed(ctx, &mut file, item.size, &mut buf, &mut hasher)?;
    confirm_unchanged(&mut file, item.size)?;
    Ok(*hasher.finalize().as_bytes())
}

// ---------------------------------------------------------------- stage 4: byte compare

/// Reads until `buf` is full or the file ends; returns the bytes read.
fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0usize;
    while filled < buf.len() {
        let Some(rest) = buf.get_mut(filled..) else { break };
        match file.read(rest) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Byte-for-byte equality of two files of `size` bytes.
fn same_bytes(ctx: &Ctx<'_>, a: &mut File, b: &mut File) -> Result<bool, Fail> {
    let mut left = vec![0u8; CHUNK_USIZE];
    let mut right = vec![0u8; CHUNK_USIZE];
    loop {
        if ctx.cancelled() {
            return Err(Fail::Cancelled);
        }
        let n = read_full(a, &mut left).map_err(|e| Fail::Bad(describe(&e)))?;
        let m = read_full(b, &mut right).map_err(|e| Fail::Bad(describe(&e)))?;
        ctx.progress.bytes_read.fetch_add(u64::try_from(n.saturating_add(m)).unwrap_or(0), Ordering::Relaxed);
        if n != m || left.get(..n) != right.get(..m) {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

const CHUNK_USIZE: usize = 64 * 1024;

/// Splits one hash group into classes of byte-identical files (normally exactly one class).
/// `None` when cancelled.
fn verify_group(ctx: &Ctx<'_>, group: Vec<Item>, problems: &mut Problems) -> Option<Vec<Vec<Item>>> {
    let mut classes: Vec<Vec<Item>> = Vec::new();
    for item in group {
        if ctx.cancelled() {
            return None;
        }
        let placed = place(ctx, &mut classes, item);
        ctx.progress.done.fetch_add(1, Ordering::Relaxed);
        match placed {
            Ok(()) => {}
            Err(Fail::Cancelled) => return None,
            Err(Fail::Bad(why)) => problems.add(ctx, item, &why),
        }
    }
    Some(classes)
}

/// Adds `item` to the class whose first member it equals byte for byte, or opens a new class.
fn place(ctx: &Ctx<'_>, classes: &mut Vec<Vec<Item>>, item: Item) -> Result<(), Fail> {
    let path = ctx.path(item.id)?;
    for class in classes.iter_mut() {
        let Some(&first) = class.first() else { continue };
        let first_path = ctx.path(first.id)?;
        let mut a = open_regular(&first_path, first.size)?;
        let mut b = open_regular(&path, item.size)?;
        if same_bytes(ctx, &mut a, &mut b)? {
            class.push(item);
            return Ok(());
        }
    }
    // Make sure the file can still be read at all before it starts a class of its own.
    open_regular(&path, item.size)?;
    classes.push(vec![item]);
    Ok(())
}

// ---------------------------------------------------------------- stage 5: identity

/// Volume and file number of the file at `path`, for telling names of one file apart.
#[cfg(unix)]
fn identity_of(path: &str) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn identity_of(path: &str) -> Option<(u64, u64)> {
    let id = dirustat_ntfs::file_identity(Path::new(path))?;
    Some((id.volume_serial, id.file_index))
}

#[cfg(not(any(unix, windows)))]
fn identity_of(_: &str) -> Option<(u64, u64)> {
    None
}

/// Keeps one name per file identity inside every group. Returns the groups and how many names
/// were dropped; `None` when cancelled.
fn collapse_identities(
    ctx: &Ctx<'_>,
    groups: Vec<Vec<Item>>,
    problems: &mut Problems,
) -> Option<(Vec<Vec<Item>>, u64)> {
    let mut out = Vec::with_capacity(groups.len());
    let mut collapsed = 0u64;
    for group in groups {
        if ctx.cancelled() {
            return None;
        }
        let mut seen: HashSet<(u64, u64)> = HashSet::new();
        let mut kept = Vec::with_capacity(group.len());
        for item in group {
            let id = match ctx.path(item.id) {
                Ok(path) => identity_of(&path),
                Err(Fail::Bad(why)) => {
                    problems.add(ctx, item, &why);
                    continue;
                }
                Err(Fail::Cancelled) => return None,
            };
            if id.is_some_and(|id| !seen.insert(id)) {
                collapsed = collapsed.saturating_add(1);
            } else {
                kept.push(item);
            }
        }
        ctx.progress.done.fetch_add(1, Ordering::Relaxed);
        out.push(kept);
    }
    Some((out, collapsed))
}
