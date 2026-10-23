//! `snapshot.*` commands (G-019): save the tree to a `.dirsnap` file, load one back as a read-only
//! view, compare two trees by path, list and close loaded snapshots.
//!
//! Like `export.*`, every command takes explicit paths and never opens a dialog (the shell's File
//! menu picks files with native dialogs and then calls these). Saving, loading and comparing are
//! [`SnapshotJob`]s: the checks run under the session lock, the slow part (file I/O, the tree walk)
//! runs without it on `Arc<Tree>` handles, and a load installs its tree under the lock again
//! ([`finish`]). A snapshot shown as the current tree is flagged (`session.state.snapshot`) and every
//! destructive command is refused while it is, because the tree is not the live disk.

use super::{CommandSpec, always, parse_params};
use crate::export::{self, HARD_MAX_NODES};
use crate::snapshot::compare::{self, ChangeKind, Options};
use crate::snapshot::format::{self, DEFAULT_LOAD_NODES, SnapshotError};
use crate::snapshot::{EXTENSION, MAX_LOADED, Meta};
use crate::{CommandError, Session, SizeModeName, guard, is_local_path, is_unc_path, normalize_scan_root};
use dirustat_model::{NodeKind, SizeMode, Tree};
use dirustat_platform::PlatformServices;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_LIMIT: u64 = 100;
const MAX_LIMIT: u64 = 10_000;
const MAX_PREFIX_SEGMENTS: usize = 4096;
const MAX_PATH_LEN: usize = 32_000;

fn has_tree(session: &Session) -> bool {
    session.tree().len() > 1
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "snapshot.save",
            label: "Save snapshot",
            menu: "",
            shortcut: None,
            params_doc: "{\"path\": string (the .dirsnap file to write; a local drive, in an existing directory), \
                \"overwrite\"?: bool (default false: an existing file is refused), \"allow_inside_scan\"?: bool \
                (default false: a target inside the scanned folder is refused)}. Writes the current tree as a \
                compact versioned binary file (magic, version, BLAKE3 checksum), atomically (temp file + \
                rename). Needs a scanned tree. Returns {path, nodes, bytes, revision}.",
            destructive: false,
            enabled: has_tree,
            run: run_save,
        },
        CommandSpec {
            id: "snapshot.load",
            label: "Open snapshot",
            menu: "",
            shortcut: None,
            params_doc: "{\"path\": string (a .dirsnap file on a local drive), \"show\"?: bool (default true: the \
                snapshot becomes the current tree, a read-only view of a past scan that is flagged in \
                session.state.snapshot; destructive commands are refused until a rescan or snapshot.close; false: \
                keep it only for snapshot.compare), \"max_nodes\"?: 1..=20000000 (default 5000000)}. Validates \
                everything (magic, version, checksum, ids, sizes, node count against the file length); a bad \
                file is an error and changes nothing. At most 4 snapshots are held. Node ids of a shown snapshot \
                are the ids of the saved tree. Returns {id, path, nodes, root, created, partial, showing, \
                revision}.",
            destructive: false,
            enabled: always,
            run: run_load,
        },
        CommandSpec {
            id: "snapshot.compare",
            label: "Compare snapshots",
            menu: "",
            shortcut: None,
            params_doc: "{\"base\": \"live\" | u64 (snapshot id; the older side), \"other\"?: \"live\" | u64 \
                (default \"live\"; the newer side), \"prefix\"?: string (compare only this path below each root, \
                separators / or \\), \"limit\"?: 1..=10000 (default 100), \"min_delta\"?: u64 (default 0), \
                \"kinds\"?: [\"added\" | \"removed\" | \"grown\" | \"shrunk\"] (default all; filters the list, not \
                the counts), \"include_dirs\"?: bool (default false: also list changed directories with their \
                subtree delta), \"size_mode\"?: \"logical\" | \"allocated\" (default: session mode)}. Pairs nodes \
                by path relative to the roots (exact names); an added or removed subtree is one entry with the \
                size of the whole subtree. delta = other - base. Returns {base, other, size_mode, prefix, \
                base_total, other_total, delta, identical, counts: {added, removed, grown, shrunk, unchanged, \
                added_bytes, removed_bytes, grown_bytes, shrunk_bytes, changed_dirs}, matching, truncated, \
                changes: [{kind, path, node_kind, old_size, new_size, delta, old_id, new_id}]} sorted by absolute \
                delta; old_id/new_id are node ids in the base/other tree (null when absent); an id is usable with \
                select.node only for the side that is the live tree (and for its revision).",
            destructive: false,
            enabled: always,
            run: run_compare,
        },
        CommandSpec {
            id: "snapshot.list",
            label: "List snapshots",
            menu: "",
            shortcut: None,
            params_doc: "none. Returns {max_loaded, viewing (id or null), snapshots: [{id, path, created, partial, \
                nodes, root, viewing}]}.",
            destructive: false,
            enabled: always,
            run: run_list,
        },
        CommandSpec {
            id: "snapshot.close",
            label: "Close snapshot",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": u64}. Releases a loaded snapshot. If it is the shown tree, the session goes back \
                to an empty tree (scan again to get a live one). Returns {closed, was_viewing}.",
            destructive: false,
            enabled: always,
            run: run_close,
        },
    ]
}

// ------------------------------------------------------------------------------ parameters

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveParams {
    path: String,
    overwrite: Option<bool>,
    allow_inside_scan: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadParams {
    path: String,
    show: Option<bool>,
    max_nodes: Option<u64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SourceParam {
    Name(String),
    Id(u64),
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum KindParam {
    Added,
    Removed,
    Grown,
    Shrunk,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareParams {
    base: SourceParam,
    other: Option<SourceParam>,
    prefix: Option<String>,
    limit: Option<u64>,
    min_delta: Option<u64>,
    kinds: Option<Vec<KindParam>>,
    include_dirs: Option<bool>,
    size_mode: Option<SizeModeName>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseParams {
    id: u64,
}

// ------------------------------------------------------------------------------ jobs

/// A checked snapshot command, ready to run without the session.
#[derive(Debug)]
pub(super) enum SnapshotJob {
    Save(SaveJob),
    Load(LoadJob),
    Compare(Box<CompareJob>),
}

#[derive(Debug)]
pub(super) struct SaveJob {
    tree: Arc<Tree>,
    revision: u64,
    partial: bool,
    target: PathBuf,
    overwrite: bool,
    platform: Arc<dyn PlatformServices>,
}

#[derive(Debug)]
pub(super) struct LoadJob {
    path: PathBuf,
    max_nodes: u64,
    show: bool,
}

#[derive(Debug)]
pub(super) struct CompareJob {
    base: Arc<Tree>,
    other: Arc<Tree>,
    base_info: Value,
    other_info: Value,
    opts: Options,
    prefix: String,
}

/// What a [`SnapshotJob`] produced. A load still has to be installed under the session lock.
#[derive(Debug)]
pub(super) enum Output {
    Done(Result<Value, CommandError>),
    Loaded { path: String, show: bool, loaded: Result<format::Loaded, CommandError> },
}

fn from_snapshot_error(path: &Path, e: SnapshotError) -> CommandError {
    match e {
        SnapshotError::Io(io) => CommandError::Platform(format!("cannot read {}: {io}", path.display())),
        other => CommandError::InvalidParams(format!("{}: {other}", path.display())),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

impl SnapshotJob {
    /// Does the slow work. Needs no session; a panic becomes an error.
    pub(super) fn run(self) -> Output {
        let is_load = matches!(self, Self::Load(_));
        let (path, show) = match &self {
            Self::Load(j) => (j.path.to_string_lossy().into_owned(), j.show),
            _ => (String::new(), false),
        };
        match guard::guarded("snapshot", || Ok(self.run_inner())) {
            Ok(out) => out,
            Err(e) if is_load => Output::Loaded { path, show, loaded: Err(e) },
            Err(e) => Output::Done(Err(e)),
        }
    }

    fn run_inner(self) -> Output {
        match self {
            Self::Save(job) => Output::Done(job.run()),
            Self::Compare(job) => Output::Done(Ok(job.run())),
            Self::Load(job) => {
                let path = job.path.to_string_lossy().into_owned();
                let show = job.show;
                Output::Loaded { path, show, loaded: job.run() }
            }
        }
    }
}

impl SaveJob {
    fn run(self) -> Result<Value, CommandError> {
        let Self { tree, revision, partial, target, overwrite, platform } = self;
        let created = unix_now();
        let bytes = export::write_atomic(&target, overwrite, platform.as_ref(), |file| {
            format::write(file, &tree, partial, created).map_err(|e| match e {
                SnapshotError::Io(io) => io,
                other => std::io::Error::other(other.to_string()),
            })
        })?;
        Ok(json!({
            "path": target.to_string_lossy(),
            "nodes": tree.len(),
            "bytes": bytes,
            "revision": revision,
        }))
    }
}

impl LoadJob {
    fn run(self) -> Result<format::Loaded, CommandError> {
        let io_err = |e: std::io::Error| CommandError::Platform(format!("cannot read {}: {e}", self.path.display()));
        let file = std::fs::File::open(&self.path).map_err(io_err)?;
        let meta = file.metadata().map_err(io_err)?;
        if !meta.is_file() {
            return Err(CommandError::InvalidParams(format!("{} is not a plain file", self.path.display())));
        }
        format::read(file, meta.len(), self.max_nodes).map_err(|e| from_snapshot_error(&self.path, e))
    }
}

impl CompareJob {
    fn run(self) -> Value {
        let report = compare::compare(&self.base, &self.other, &self.opts);
        let c = report.counts;
        let changes: Vec<Value> = report
            .changes
            .iter()
            .map(|ch| {
                json!({
                    "kind": ch.kind.name(),
                    "path": ch.path,
                    "node_kind": node_kind_name(ch.node_kind),
                    "old_size": ch.old_size,
                    "new_size": ch.new_size,
                    "delta": ch.delta,
                    "old_id": ch.old_id.map(|n| n.index()),
                    "new_id": ch.new_id.map(|n| n.index()),
                })
            })
            .collect();
        json!({
            "base": self.base_info,
            "other": self.other_info,
            "size_mode": if self.opts.mode == SizeMode::Logical { "logical" } else { "allocated" },
            "prefix": self.prefix,
            "base_total": report.base_total,
            "other_total": report.other_total,
            "delta": compare::saturating_delta(report.base_total, report.other_total),
            "identical": report.identical,
            "counts": {
                "added": c.added, "removed": c.removed, "grown": c.grown, "shrunk": c.shrunk,
                "unchanged": c.unchanged, "added_bytes": c.added_bytes, "removed_bytes": c.removed_bytes,
                "grown_bytes": c.grown_bytes, "shrunk_bytes": c.shrunk_bytes, "changed_dirs": c.changed_dirs,
            },
            "matching": report.matching,
            "truncated": report.matching > u64::try_from(report.changes.len()).unwrap_or(u64::MAX),
            "changes": changes,
        })
    }
}

fn node_kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Link => "link",
    }
}

// ------------------------------------------------------------------------------ begin (under the lock)

fn invalid(msg: impl Into<String>) -> CommandError {
    CommandError::InvalidParams(msg.into())
}

fn scan_root_name(session: &Session) -> String {
    let tree = session.tree();
    tree.get(tree.root()).map(|n| n.name().to_owned()).unwrap_or_default()
}

pub(super) fn begin_save(session: &Session, params: &Value) -> Result<SnapshotJob, CommandError> {
    let p: SaveParams = parse_params(params)?;
    if u64::try_from(session.tree().len()).unwrap_or(u64::MAX) > HARD_MAX_NODES {
        return Err(invalid("the tree has more nodes than a snapshot can hold"));
    }
    let overwrite = p.overwrite.unwrap_or(false);
    let target = export::validate_target(
        &p.path,
        EXTENSION,
        &scan_root_name(session),
        overwrite,
        p.allow_inside_scan.unwrap_or(false),
    )?;
    Ok(SnapshotJob::Save(SaveJob {
        tree: session.tree_arc(),
        revision: session.revision(),
        partial: session.partial_scan,
        target,
        overwrite,
        platform: session.platform(),
    }))
}

/// A show replaces the tree: not while a scan or a detached deletion is in flight.
fn check_can_show(session: &Session) -> Result<(), CommandError> {
    if session.is_scanning() {
        return Err(CommandError::NotEnabled("snapshot.load: a scan is running; wait or pass show: false".to_owned()));
    }
    if session.removal_in_progress() {
        return Err(CommandError::NotEnabled("snapshot.load: a deletion is still running".to_owned()));
    }
    Ok(())
}

pub(super) fn begin_load(session: &Session, params: &Value) -> Result<SnapshotJob, CommandError> {
    let p: LoadParams = parse_params(params)?;
    let show = p.show.unwrap_or(true);
    if show {
        check_can_show(session)?;
    }
    if session.snapshots.items.len() >= MAX_LOADED {
        return Err(invalid(format!("{MAX_LOADED} snapshots are loaded; close one with snapshot.close first")));
    }
    let max_nodes = match p.max_nodes {
        None => DEFAULT_LOAD_NODES,
        Some(n) if (1..=HARD_MAX_NODES).contains(&n) => n,
        Some(_) => return Err(invalid(format!("max_nodes must be 1..={HARD_MAX_NODES}"))),
    };
    if p.path.is_empty() || p.path.contains('\0') || p.path.len() > MAX_PATH_LEN {
        return Err(invalid("path must be 1..=32000 characters without NUL"));
    }
    let absolute = std::path::absolute(Path::new(&p.path)).map_err(|e| invalid(format!("bad path: {e}")))?;
    let text = normalize_scan_root(&absolute.to_string_lossy());
    if is_unc_path(&text) || !is_local_path(&text) {
        return Err(invalid("snapshots are read from a local drive (no network or device paths)"));
    }
    Ok(SnapshotJob::Load(LoadJob { path: PathBuf::from(text), max_nodes, show }))
}

struct Resolved {
    tree: Arc<Tree>,
    info: Value,
}

fn resolve_source(session: &Session, source: &SourceParam) -> Result<Resolved, CommandError> {
    match source {
        SourceParam::Name(name) if name == "live" => Ok(Resolved {
            tree: session.tree_arc(),
            info: json!({
                "source": "live",
                "revision": session.revision(),
                "root": scan_root_name(session),
                "snapshot": session.snapshots.viewing,
            }),
        }),
        SourceParam::Name(other) => Err(invalid(format!("unknown source {other:?}: use \"live\" or a snapshot id"))),
        SourceParam::Id(id) => {
            let snap = session.snapshots.get(*id).ok_or_else(|| invalid(format!("no loaded snapshot with id {id}")))?;
            Ok(Resolved {
                tree: Arc::clone(&snap.tree),
                info: json!({
                    "source": "snapshot",
                    "id": snap.id,
                    "path": snap.path,
                    "created": snap.meta.created,
                    "root": snap.tree.get(snap.tree.root()).map(|n| n.name()),
                }),
            })
        }
    }
}

fn parse_prefix(prefix: &str) -> Result<Vec<String>, CommandError> {
    if prefix.contains('\0') || prefix.len() > MAX_PATH_LEN {
        return Err(invalid("prefix must be at most 32000 characters without NUL"));
    }
    let parts: Vec<String> = prefix.split(['/', '\\']).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    if parts.len() > MAX_PREFIX_SEGMENTS {
        return Err(invalid("prefix has too many segments"));
    }
    Ok(parts)
}

pub(super) fn begin_compare(session: &Session, params: &Value) -> Result<SnapshotJob, CommandError> {
    let p: CompareParams = parse_params(params)?;
    let base = resolve_source(session, &p.base)?;
    let other = resolve_source(session, p.other.as_ref().unwrap_or(&SourceParam::Name("live".to_owned())))?;
    let limit = match p.limit {
        None => DEFAULT_LIMIT,
        Some(n) if (1..=MAX_LIMIT).contains(&n) => n,
        Some(_) => return Err(invalid(format!("limit must be 1..={MAX_LIMIT}"))),
    };
    let kinds = p.kinds.map_or_else(
        || ChangeKind::ALL.to_vec(),
        |list| {
            list.into_iter()
                .map(|k| match k {
                    KindParam::Added => ChangeKind::Added,
                    KindParam::Removed => ChangeKind::Removed,
                    KindParam::Grown => ChangeKind::Grown,
                    KindParam::Shrunk => ChangeKind::Shrunk,
                })
                .collect()
        },
    );
    let prefix = parse_prefix(p.prefix.as_deref().unwrap_or(""))?;
    let opts = Options {
        mode: p.size_mode.map_or_else(|| session.map_size_mode(), SizeModeName::mode),
        limit: usize::try_from(limit).unwrap_or(usize::MAX),
        include_dirs: p.include_dirs.unwrap_or(false),
        min_delta: p.min_delta.unwrap_or(0),
        kinds,
        prefix: prefix.clone(),
    };
    Ok(SnapshotJob::Compare(Box::new(CompareJob {
        base: base.tree,
        other: other.tree,
        base_info: base.info,
        other_info: other.info,
        opts,
        prefix: prefix.join("/"),
    })))
}

// ------------------------------------------------------------------------------ finish (under the lock)

/// Completes a job under the session lock: a load is stored (and shown), the rest just reports.
pub(super) fn finish(session: &mut Session, output: Output) -> Result<Value, CommandError> {
    match output {
        Output::Done(result) => result,
        Output::Loaded { path, show, loaded } => {
            let loaded = loaded?;
            if session.snapshots.items.len() >= MAX_LOADED {
                return Err(invalid(format!("{MAX_LOADED} snapshots are loaded; close one first")));
            }
            if show {
                check_can_show(session)?;
            }
            let format::Loaded { tree, meta } = loaded;
            let id = session.snapshots.add(path.clone(), meta, tree);
            if show {
                session.show_snapshot(id);
            }
            let root = session.snapshots.get(id).and_then(|s| s.tree.get(s.tree.root()).map(|n| n.name().to_owned()));
            Ok(json!({
                "id": id,
                "path": path,
                "nodes": meta.nodes,
                "root": root,
                "created": meta.created,
                "partial": meta.partial,
                "showing": show,
                "revision": session.revision(),
            }))
        }
    }
}

fn run_job(session: &mut Session, job: SnapshotJob) -> Result<Value, CommandError> {
    finish(session, job.run())
}

fn run_save(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let job = begin_save(session, params)?;
    run_job(session, job)
}

fn run_load(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let job = begin_load(session, params)?;
    run_job(session, job)
}

fn run_compare(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let job = begin_compare(session, params)?;
    run_job(session, job)
}

fn run_list(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    super::no_params(params)?;
    let items: Vec<Value> = session
        .snapshots
        .items
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "path": s.path,
                "created": s.meta.created,
                "partial": s.meta.partial,
                "nodes": s.meta.nodes,
                "root": s.tree.get(s.tree.root()).map(|n| n.name()),
                "viewing": session.snapshots.viewing == Some(s.id),
            })
        })
        .collect();
    Ok(json!({ "max_loaded": MAX_LOADED, "viewing": session.snapshots.viewing, "snapshots": items }))
}

fn run_close(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: CloseParams = parse_params(params)?;
    let pos = session
        .snapshots
        .items
        .iter()
        .position(|s| s.id == p.id)
        .ok_or_else(|| invalid(format!("no loaded snapshot with id {}", p.id)))?;
    let was_viewing = session.snapshots.viewing == Some(p.id);
    if was_viewing {
        check_can_show(session)?;
    }
    session.snapshots.items.remove(pos);
    if was_viewing {
        session.replace_tree(dirustat_scan::empty_tree(""));
    }
    Ok(json!({ "closed": p.id, "was_viewing": was_viewing }))
}

/// The snapshot shown as the current tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotView {
    pub id: u64,
    pub path: String,
    pub meta: Meta,
}

impl Session {
    /// The snapshot that is the current tree, if any. While it is, the tree is not the live disk and
    /// destructive commands are refused.
    pub fn snapshot_view(&self) -> Option<SnapshotView> {
        let id = self.snapshots.viewing?;
        self.snapshots.get(id).map(|s| SnapshotView { id, path: s.path.clone(), meta: s.meta })
    }

    pub(crate) fn snapshot_view_json(&self) -> Value {
        self.snapshot_view().map_or(
            Value::Null,
            |v| json!({ "id": v.id, "path": v.path, "created": v.meta.created, "partial": v.meta.partial }),
        )
    }

    /// Makes loaded snapshot `id` the current tree and flags the session.
    fn show_snapshot(&mut self, id: u64) {
        let Some(snap) = self.snapshots.get(id) else { return };
        let (tree, partial) = (Arc::clone(&snap.tree), snap.meta.partial);
        self.tree = tree;
        self.last_scan = None;
        self.tree_replaced();
        self.partial_scan = partial;
        self.rescan_recommended = false;
        self.snapshots.viewing = Some(id);
    }
}

#[cfg(test)]
mod tests;
