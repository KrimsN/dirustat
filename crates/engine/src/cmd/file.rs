//! `file.*` commands: the destructive operations of `docs/safety.md`.
//!
//! * `file.delete_preview` (read-only): the dry-run report of what a deletion would affect.
//! * `file.delete`: recycle-bin deletion through the platform layer (the only reversible mode).
//! * `file.delete_permanent`: permanent deletion; needs `confirm` and `i_understand_permanent`.
//!   A UI must show an explicit warning before sending it.
//! * `file.move`: `rename` to a new place that does not exist yet.
//! * `file.journal` (read-only): the session journal of destructive actions.
//!
//! Every destructive command runs the same pipeline before touching anything: resolve the target
//! to a node of the *current* tree, refuse protected paths (lexically and again on the real
//! location), re-validate the object on disk against the tree (`stale_tree` on any difference),
//! build the dry-run report, and only then act. After a successful action the node is cut out of
//! the session tree (`prune`), the revision is bumped (arena ids change, selection is cleared)
//! and the action is journaled.

use super::detached::BusyGuard;
use super::platform::platform_error;
use super::select::node_by_index;
use super::tree::{kind_name, page_limit};
use super::{CommandSpec, always, parse_params};
use crate::journal::{JournalEntry, now_secs};
use crate::protected::{ProtectedPaths, key_exact, key_of, name_matches};
use crate::{CommandError, Session, fsops, guard, prune};
use dirustat_model::{NodeId, NodeKind, SizeMode};
use dirustat_platform::PlatformServices;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// Paths listed in a report; the counts always cover the whole subtree.
const MAX_LISTED: usize = 200;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "file.delete_preview",
            label: "Preview deletion",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\"+\"revision\"|\"path\": see file.delete, \"mode\"?: \"recycle\" (default) | \"permanent\"}. \
                Read-only: runs every check of the real command (protected path, stale tree, link rules) and returns \
                the report of what would be affected: {dry_run: true, mode, path, kind, count, files, dirs, links, \
                size, size_logical, size_allocated, paths: [string] (at most 200, the target first), truncated, \
                partial_tree}. partial_tree: the scan used exclusions or the subtree had read errors, so the tree may \
                omit entries that exist on disk. Needs no confirm.",
            destructive: false,
            enabled: has_tree,
            run: run_preview,
        },
        CommandSpec {
            id: "file.delete",
            label: "Delete (recycle bin)",
            menu: "File/Delete",
            shortcut: Some("Delete"),
            params_doc: "{\"id\": u64, \"revision\": u64} arena index of a node with the Session revision at which it \
                was read (revision is required with id; any other current revision is stale_tree and nothing \
                happens), or {\"path\": string} an absolute path that resolves to a node of the current tree \
                (exactly one of id/path), \"dry_run\"?: bool (default false; changes nothing and \
                returns the report). Moves the item to the recycle bin. Needs confirm: true even for dry_run (use \
                file.delete_preview to look first). Refuses protected paths (protected_path), a tree that no longer \
                matches the disk (stale_tree) and links/junctions (use file.delete_permanent, which removes the link \
                itself). Returns {dry_run, mode, path, kind, count, files, dirs, links, size, size_logical, \
                size_allocated, paths, truncated, partial_tree} plus, when done: {done, tree_updated, revision, ids_invalidated}; \
                node ids are invalid after a successful deletion.",
            destructive: true,
            enabled: has_tree,
            run: run_delete,
        },
        CommandSpec {
            id: "file.delete_permanent",
            label: "Delete permanently",
            menu: "File/Delete permanently",
            shortcut: Some("Shift+Delete"),
            params_doc: "{\"id\": u64, \"revision\": u64} or {\"path\": string} (exactly one, as file.delete), \
                \"dry_run\"?: bool, \"i_understand_permanent\": bool must be true unless dry_run (otherwise \
                confirmation_required), \"i_understand_partial\"?: bool, required to delete a folder whose report \
                says partial_tree: true (entries missing from the tree would be deleted too). \
                Deletes WITHOUT the recycle bin; cannot be undone. UIs must show an explicit warning first. Links are \
                removed themselves, never followed. Same refusals and result as file.delete with mode \"permanent\".",
            destructive: true,
            enabled: has_tree,
            run: run_delete_permanent,
        },
        CommandSpec {
            id: "file.move",
            label: "Move",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": u64, \"revision\": u64} or {\"path\": string} (exactly one, as file.delete), \
                \"destination\": string absolute local path the item gets (must not exist, its parent must be an \
                existing directory, neither may be protected, not inside the item itself, no `:` and no trailing \
                dot or space in any name, no network path), \"dry_run\"?: bool. No-replace rename on one volume \
                (MoveFileExW without REPLACE_EXISTING; no overwrite, no copy fallback; unsupported where the \
                platform has no such primitive). Returns the delete report with mode \"move\" plus {destination, \
                rescan_recommended} (true when the destination is inside the scan root). Same refusals as file.delete.",
            destructive: true,
            enabled: has_tree,
            run: run_move,
        },
        CommandSpec {
            id: "file.journal",
            label: "Action journal",
            menu: "",
            shortcut: None,
            params_doc: "{\"limit\"?: 1..=10000 (default 100), \"offset\"?: u64 (default 0)}. Read-only. Destructive \
                actions of this session, newest first: {total, offset, entries: [{time, op, path, destination, size, \
                mode, ok, error}]}; at most 10000 entries are kept.",
            destructive: false,
            enabled: always,
            run: run_journal,
        },
    ]
}

fn has_tree(session: &Session) -> bool {
    session.tree().len() > 1 && !session.is_scanning()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Recycle,
    Permanent,
    Move,
}

impl Mode {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Recycle => "recycle",
            Self::Permanent => "permanent",
            Self::Move => "move",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
    id: Option<u64>,
    revision: Option<u64>,
    path: Option<String>,
    dry_run: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermanentParams {
    id: Option<u64>,
    revision: Option<u64>,
    path: Option<String>,
    dry_run: Option<bool>,
    i_understand_permanent: Option<bool>,
    i_understand_partial: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewParams {
    id: Option<u64>,
    revision: Option<u64>,
    path: Option<String>,
    mode: Option<PreviewMode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PreviewMode {
    Recycle,
    Permanent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveParams {
    id: Option<u64>,
    revision: Option<u64>,
    path: Option<String>,
    destination: String,
    dry_run: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalParams {
    limit: Option<u64>,
    offset: Option<u64>,
}

/// Everything known about an operation before it touches the disk.
struct Plan {
    node: NodeId,
    path: String,
    scan_root: String,
    kind: NodeKind,
    /// The tree may omit entries of this subtree that exist on disk (exclusions, read errors).
    partial: bool,
    /// Recycle mode only: how the real folder differs from the tree (a warning, not a refusal).
    drift: Option<String>,
    report: Report,
}

#[derive(Default)]
struct Report {
    files: u64,
    dirs: u64,
    links: u64,
    logical: u64,
    allocated: u64,
    paths: Vec<String>,
    truncated: bool,
    /// How many path strings were built (never more than `MAX_LISTED`).
    built_paths: usize,
}

impl Report {
    fn count(&self) -> u64 {
        self.files.saturating_add(self.dirs).saturating_add(self.links)
    }

    fn size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Logical => self.logical,
            SizeMode::Allocated => self.allocated,
        }
    }
}

/// How a destructive command names its target: `path`, or `id` together with the `revision` at
/// which the id was read (arena ids change with every prune).
#[derive(Clone, Copy)]
struct Addr<'a> {
    id: Option<u64>,
    revision: Option<u64>,
    path: Option<&'a str>,
}

/// Resolves `{id + revision | path}` to a node of the current tree. A `revision` that is not the
/// session's current one is `stale_tree` whatever the addressing; an `id` without `revision` is
/// `invalid_params`. Nothing is touched.
fn resolve_node(session: &Session, addr: Addr<'_>) -> Result<NodeId, CommandError> {
    if let Some(rev) = addr.revision
        && rev != session.revision()
    {
        return Err(CommandError::StaleTree(format!(
            "node ids were read at revision {rev} but the tree is at revision {}; re-read the ids",
            session.revision()
        )));
    }
    match (addr.id, addr.path) {
        (Some(_), None) if addr.revision.is_none() => Err(CommandError::InvalidParams(
            "`id` needs `revision` (the revision at which the id was obtained); or address by `path`".to_owned(),
        )),
        (Some(index), None) => node_by_index(session.tree(), index)
            .ok_or_else(|| CommandError::InvalidParams(format!("no node with id {index}"))),
        (None, Some(path)) => node_for_path(session, path),
        _ => Err(CommandError::InvalidParams("give exactly one of `id` or `path`".to_owned())),
    }
}

/// Whether the tree may omit entries of `node`'s subtree that exist on disk: the scan used
/// exclusions, or some entry below could not be read.
pub(super) fn subtree_partial(session: &Session, node: NodeId) -> bool {
    session.partial_scan || session.tree().get(node).is_some_and(|n| n.error_count() > 0 || n.data().error)
}

/// Walks the tree from the root along `path`, comparing components **exactly** (QA-139): no
/// trimming, no Unicode case folding; on Windows only ASCII letters are case-insensitive. A
/// component that matches two siblings is `stale_tree` (ambiguous), never a first-match guess.
/// Used by every command that can act on the result, so the node a caller names is the node acted on.
pub(super) fn node_for_path(session: &Session, path: &str) -> Result<NodeId, CommandError> {
    node_for_path_in(session, path, cfg!(windows))
}

fn node_for_path_in(session: &Session, path: &str, windows: bool) -> Result<NodeId, CommandError> {
    let tree = session.tree();
    let outside = || CommandError::InvalidParams("path does not resolve to a node of the scanned tree".to_owned());
    let key =
        key_exact(path, windows).ok_or_else(|| CommandError::InvalidParams("path must be absolute".to_owned()))?;
    let root_path = session.node_path(tree.root()).ok_or_else(outside)?;
    let root_key = key_exact(&root_path, windows).ok_or_else(outside)?;
    let same = |a: &str, b: &str| name_matches(a, b, windows);
    let below_root = key.comps.len() >= root_key.comps.len()
        && key.root == root_key.root
        && key.comps.iter().zip(&root_key.comps).all(|(a, b)| same(a, b));
    if !below_root {
        return Err(outside());
    }
    let mut cur = tree.root();
    for comp in key.comps.get(root_key.comps.len()..).unwrap_or_default() {
        let mut hits = tree.children(cur).filter(|&c| tree.get(c).is_some_and(|n| same(n.name(), comp)));
        let (first, second) = (hits.next().ok_or_else(outside)?, hits.next());
        if second.is_some() {
            return Err(CommandError::StaleTree(format!(
                "more than one entry matches the path component {comp:?} (names that differ only in case); nothing \
                 was touched, address the item by id and revision"
            )));
        }
        cur = first;
    }
    Ok(cur)
}

/// Iterative walk of the subtree: counts, saturating sums and a capped path listing. Path strings
/// are built lazily, only for the first `MAX_LISTED` nodes in walk order, so a huge directory
/// costs counters and a stack of ids, not a string per child.
fn build_report(session: &Session, node: NodeId) -> Report {
    let tree = session.tree();
    let mut report = Report::default();
    let mut stack: Vec<NodeId> = vec![node];
    while let Some(id) = stack.pop() {
        let Some(n) = tree.get(id) else { continue };
        match n.kind() {
            NodeKind::Dir => report.dirs = report.dirs.saturating_add(1),
            NodeKind::File => report.files = report.files.saturating_add(1),
            NodeKind::Link => report.links = report.links.saturating_add(1),
        }
        report.logical = report.logical.saturating_add(n.own_size(SizeMode::Logical));
        report.allocated = report.allocated.saturating_add(n.own_size(SizeMode::Allocated));
        if report.paths.len() < MAX_LISTED {
            if let Some(p) = session.node_path(id) {
                report.paths.push(p);
                report.built_paths = report.built_paths.saturating_add(1);
            }
        } else {
            report.truncated = true;
        }
        if n.kind() == NodeKind::Dir {
            stack.extend(tree.children(id));
        }
    }
    report
}

/// Destructive commands only work below a scan root on a local lettered drive: a network (UNC)
/// share, a device namespace or a relative root is refused (`docs/safety.md`).
pub(super) fn require_local_root(scan_root: &str) -> Result<(), CommandError> {
    if crate::is_local_path(scan_root) {
        return Ok(());
    }
    Err(CommandError::ProtectedPath(
        "destructive commands are disabled unless the scan root is a local drive path; a network (UNC), device or \
         relative root is refused, scan a path such as C:\\data"
            .to_owned(),
    ))
}

/// Every name from the node up to (not including) the scan root must be exactly representable as
/// a path component: not lossy (the stored name replaced invalid UTF-16 with U+FFFD, so the path
/// built from it could reach a different file) and, on Windows, free of the spellings Win32
/// rewrites (QA-116). Anything else is refused as unaddressable.
fn verify_addressable(session: &Session, node: NodeId) -> Result<(), CommandError> {
    let tree = session.tree();
    let refuse = |why: &str| {
        CommandError::Unsupported(format!(
            "refused: {why}; this item cannot be addressed by a unique path, so it was not touched"
        ))
    };
    let mut cur = node;
    for _ in 0..=tree.len() {
        let n = tree.get(cur).ok_or_else(|| CommandError::InvalidParams("unknown node".to_owned()))?;
        let Some(parent) = n.parent() else { return Ok(()) };
        if n.data().name_lossy {
            return Err(refuse("a name on the path is not valid Unicode (it was stored with U+FFFD)"));
        }
        if let Some(why) = fsops::name_problem(n.name(), cfg!(windows)) {
            return Err(refuse(why));
        }
        cur = parent;
    }
    Err(CommandError::InvalidParams("the node's parent chain is broken".to_owned()))
}

/// Re-reads the real subtree of the folder `node` at `path` and compares it with the tree
/// (QA-118). `Err` describes the first difference. A partial tree tolerates extra disk entries.
fn subtree_drift(session: &Session, node: NodeId, path: &str) -> Result<(), String> {
    crate::subtree::verify(session.tree(), node, std::path::PathBuf::from(path), subtree_partial(session, node))
}

/// The safety checks every destructive operation passes before anything is touched: protected
/// paths (lexical and real location), then the stale-tree re-check of the object on disk and the
/// link rule. Returns the node's path. Reads the disk, never changes it.
pub(super) fn preflight(
    session: &Session,
    node: NodeId,
    protected: &ProtectedPaths,
    mode: Mode,
) -> Result<String, CommandError> {
    let tree = session.tree();
    let target = tree.get(node).ok_or_else(|| CommandError::InvalidParams("unknown node".to_owned()))?;
    let node_path =
        session.node_path(node).ok_or_else(|| CommandError::InvalidParams("node has no path".to_owned()))?;
    let scan_root = session
        .node_path(tree.root())
        .ok_or_else(|| CommandError::InvalidParams("the tree has no root path".to_owned()))?;
    require_local_root(&scan_root)?;
    protected.check(&node_path, &scan_root).map_err(CommandError::ProtectedPath)?;
    verify_addressable(session, node)?;
    fsops::verify_location(&node_path, &scan_root, protected, cfg!(windows))?;
    fsops::verify_identity(Path::new(&node_path), target.kind())?;
    fsops::verify_matches(Path::new(&node_path), target.kind(), target.data())?;
    if target.kind() == NodeKind::Dir && mode != Mode::Recycle {
        // Permanent removal and moves take everything below the folder along: re-read the real
        // subtree and require it to be what the preview showed (QA-118).
        subtree_drift(session, node, &node_path).map_err(CommandError::StaleTree)?;
    }
    if mode == Mode::Recycle && target.kind() == NodeKind::Link {
        return Err(CommandError::InvalidParams(
            "links and junctions are not sent to the recycle bin; use file.delete_permanent, which removes the link \
             itself and never its target"
                .to_owned(),
        ));
    }
    Ok(node_path)
}

/// The shared pre-flight of every destructive command; touches nothing.
fn plan(session: &Session, addr: Addr<'_>, mode: Mode) -> Result<Plan, CommandError> {
    let node = resolve_node(session, addr)?;
    let tree = session.tree();
    let kind = tree.get(node).ok_or_else(|| CommandError::InvalidParams("unknown node".to_owned()))?.kind();
    let scan_root = session
        .node_path(tree.root())
        .ok_or_else(|| CommandError::InvalidParams("the tree has no root path".to_owned()))?;
    let node_path = preflight(session, node, &ProtectedPaths::from_env(), mode)?;
    let report = build_report(session, node);
    let partial = subtree_partial(session, node);
    // Recycling keeps the lighter check (the data is recoverable) but still reports drift.
    let drift = if mode == Mode::Recycle && kind == NodeKind::Dir {
        subtree_drift(session, node, &node_path).err()
    } else {
        None
    };
    Ok(Plan { node, path: node_path, scan_root, kind, partial, drift, report })
}

fn report_json(session: &Session, plan: &Plan, mode: Mode, dry_run: bool) -> Value {
    let size = plan.report.size(session.map_size_mode());
    json!({
        "dry_run": dry_run,
        "mode": mode.name(),
        "path": plan.path,
        "kind": kind_name(plan.kind),
        "count": plan.report.count(),
        "files": plan.report.files,
        "dirs": plan.report.dirs,
        "links": plan.report.links,
        "size": size,
        "size_logical": plan.report.logical,
        "size_allocated": plan.report.allocated,
        "paths": plan.report.paths,
        "truncated": plan.report.truncated,
        "partial_tree": plan.partial,
        "subtree_changed": plan.drift,
    })
}

impl Session {
    /// Cuts `node` out of the session tree and bumps the revision (ids change, selection is
    /// cleared). `false` when the node could not be removed; the tree is then left as it was.
    fn forget_node(&mut self, node: NodeId) -> bool {
        let Some(pruned) = prune::without_subtree(self.tree(), node) else { return false };
        self.tree = Arc::new(pruned);
        self.tree_replaced();
        true
    }
}

fn run_preview(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: PreviewParams = parse_params(params)?;
    let mode = match p.mode {
        None | Some(PreviewMode::Recycle) => Mode::Recycle,
        Some(PreviewMode::Permanent) => Mode::Permanent,
    };
    let plan = plan(session, Addr { id: p.id, revision: p.revision, path: p.path.as_deref() }, mode)?;
    Ok(report_json(session, &plan, mode, true))
}

fn run_delete(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: DeleteParams = parse_params(params)?;
    let addr = Addr { id: p.id, revision: p.revision, path: p.path.as_deref() };
    execute(session, "file.delete", Mode::Recycle, addr, p.dry_run.unwrap_or(false), None, false)
}

fn run_delete_permanent(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: PermanentParams = parse_params(params)?;
    let (addr, dry_run, allow_partial) = permanent_args(&p)?;
    execute(session, "file.delete_permanent", Mode::Permanent, addr, dry_run, None, allow_partial)
}

fn run_move(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: MoveParams = parse_params(params)?;
    let addr = Addr { id: p.id, revision: p.revision, path: p.path.as_deref() };
    execute(session, "file.move", Mode::Move, addr, p.dry_run.unwrap_or(false), Some(&p.destination), false)
}

/// The first problem with the components of a move destination, for Windows names: `:` would
/// address an alternate data stream, trailing dots and spaces are silently dropped by Win32, and
/// the reserved characters cannot occur in a name at all.
pub(super) fn destination_component_problem(comps: &[String], windows: bool) -> Option<&'static str> {
    comps.iter().find_map(|c| fsops::name_problem(c, windows))
}

/// Validates the destination of a move; returns it as given. Nothing is touched.
fn check_destination(plan: &Plan, destination: &str) -> Result<(), CommandError> {
    let windows = cfg!(windows);
    let bad = |why: &str| CommandError::InvalidParams(format!("destination: {why}"));
    match crate::classify_path(destination) {
        crate::PathClass::Network => {
            return Err(CommandError::ProtectedPath("destination is a network (UNC) path".to_owned()));
        }
        crate::PathClass::Device => return Err(bad("device namespace paths are not supported")),
        crate::PathClass::Local | crate::PathClass::Other => {}
    }
    if windows && destination.ends_with(['.', ' ']) {
        return Err(bad("a name ends with a dot or a space"));
    }
    let dest_key = key_of(destination, windows).ok_or_else(|| bad("must be an absolute path"))?;
    let src_key = key_of(&plan.path, windows).ok_or_else(|| bad("the source has no absolute path"))?;
    if dest_key.starts_with(&src_key) {
        return Err(bad("cannot move an item into itself"));
    }
    if let Some(why) = destination_component_problem(&dest_key.comps, windows) {
        return Err(bad(why));
    }
    let protected = ProtectedPaths::from_env();
    protected.check(destination, "").map_err(|why| CommandError::ProtectedPath(format!("destination is {why}")))?;
    let mut parent_key = dest_key.clone();
    parent_key.comps.pop();
    if parent_key == dest_key {
        return Err(bad("must name a new item inside a directory"));
    }
    let parent = Path::new(destination).parent().ok_or_else(|| bad("has no parent directory"))?;
    protected
        .check(&parent.to_string_lossy(), "")
        .map_err(|why| CommandError::ProtectedPath(format!("the destination directory is {why}")))?;
    if std::fs::symlink_metadata(destination).is_ok() {
        return Err(bad("already exists; nothing is overwritten"));
    }
    if !std::fs::metadata(parent).is_ok_and(|m| m.is_dir()) {
        return Err(bad("the parent directory does not exist"));
    }
    let real_parent = std::fs::canonicalize(parent).map_err(|_| bad("the parent directory cannot be resolved"))?;
    protected
        .check(&real_parent.to_string_lossy(), "")
        .map_err(|why| CommandError::ProtectedPath(format!("the real destination directory is {why}")))?;
    Ok(())
}

/// Everything decided before the disk is touched: the checks passed, the report is built.
#[derive(Debug)]
struct Ready {
    node: NodeId,
    op: String,
    mode: Mode,
    path: String,
    destination: Option<String>,
    size: u64,
    /// The session revision the plan was made against; the tree is pruned only if it is unchanged.
    revision: u64,
    out: Value,
}

/// The result of staging a destructive command.
enum Staged {
    /// Nothing to do on disk (a dry run): this is the answer.
    Report(Value),
    /// All checks passed; act, then call [`complete`].
    Act(Box<Ready>),
}

/// Runs every check of a destructive command and builds its report. Reads the disk, changes nothing.
fn stage(
    session: &Session,
    op: &str,
    mode: Mode,
    addr: Addr<'_>,
    dry_run: bool,
    destination: Option<&str>,
    allow_partial: bool,
) -> Result<Staged, CommandError> {
    let plan = plan(session, addr, mode)?;
    if let Some(dest) = destination {
        check_destination(&plan, dest)?;
    }
    let mut out = report_json(session, &plan, mode, dry_run);
    if let (Some(dest), Some(map)) = (destination, out.as_object_mut()) {
        let inside = key_of(dest, cfg!(windows))
            .zip(key_of(&plan.scan_root, cfg!(windows)))
            .is_some_and(|(d, r)| d.starts_with(&r));
        map.insert("destination".to_owned(), json!(dest));
        map.insert("rescan_recommended".to_owned(), json!(inside));
    }
    if dry_run {
        return Ok(Staged::Report(out));
    }
    if mode == Mode::Permanent && plan.kind == NodeKind::Dir && plan.partial && !allow_partial {
        return Err(CommandError::ConfirmationRequired(
            "file.delete_permanent: i_understand_partial (the tree may omit entries of this folder that exist on \
             disk, they would be deleted too)"
                .to_owned(),
        ));
    }
    // Last look immediately before acting: the same checks again, so the window between the plan
    // and the action is as small as it can be (the residual race is documented in docs/safety.md).
    let path = preflight(session, plan.node, &ProtectedPaths::from_env(), mode)?;
    let size = plan.report.size(session.map_size_mode());
    Ok(Staged::Act(Box::new(Ready {
        node: plan.node,
        op: op.to_owned(),
        mode,
        path,
        destination: destination.map(str::to_owned),
        size,
        revision: session.revision(),
        out,
    })))
}

/// Journals the outcome of the action on `ready.path` and updates the tree: prunes the node on
/// success (or when a failed removal left it gone), flags `rescan_recommended` when the tree was
/// replaced under a detached deletion, and completes the report.
fn complete(session: &mut Session, ready: Ready, result: Result<(), CommandError>) -> Result<Value, CommandError> {
    let Ready { node, op, mode, path, destination, size, revision, mut out } = ready;
    record(session, &op, mode, &path, destination.as_deref(), size, &result);
    let current = session.revision() == revision;
    if let Err(err) = result {
        let gone = std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
        if mode == Mode::Permanent && gone {
            // It is gone after all (removed half-way or by someone else): keep the tree honest.
            if current {
                session.forget_node(node);
            } else {
                session.rescan_recommended = true;
            }
        }
        return Err(err);
    }
    let updated = current && session.forget_node(node);
    if !current {
        // The tree was replaced while the deletion ran without the lock: its ids are not ours.
        session.rescan_recommended = true;
    }
    if let Some(map) = out.as_object_mut() {
        map.insert("done".to_owned(), json!(true));
        map.insert("tree_updated".to_owned(), json!(updated));
        map.insert("revision".to_owned(), json!(session.revision()));
        map.insert("ids_invalidated".to_owned(), json!(updated));
        if !current {
            map.insert("rescan_recommended".to_owned(), json!(true));
        }
    }
    Ok(out)
}

fn execute(
    session: &mut Session,
    op: &str,
    mode: Mode,
    addr: Addr<'_>,
    dry_run: bool,
    destination: Option<&str>,
    allow_partial: bool,
) -> Result<Value, CommandError> {
    match stage(session, op, mode, addr, dry_run, destination, allow_partial)? {
        Staged::Report(report) => Ok(report),
        Staged::Act(ready) => {
            let result = act(session, ready.mode, &ready.path, ready.destination.as_deref());
            complete(session, *ready, result)
        }
    }
}

/// The first phase of `file.delete_permanent` for a host that runs the removal without the
/// session lock (see `cmd::detached`).
pub(super) enum Begun {
    /// A dry run: the report is the whole answer.
    Report(Value),
    /// The checks passed; run the job, then [`finish_removal`].
    Job(RemovalJob),
}

/// A checked removal (permanent or to the recycle bin) that owns everything it needs to run
/// without the session.
#[derive(Debug)]
pub(super) struct RemovalJob {
    ready: Box<Ready>,
    platform: Arc<dyn PlatformServices>,
    busy: BusyGuard,
}

/// The removal's result, to be completed under the lock.
#[derive(Debug)]
pub(super) struct RemovalOutcome {
    ready: Box<Ready>,
    result: Result<(), CommandError>,
    busy: BusyGuard,
}

impl RemovalJob {
    /// Removes the item (the slow part). Needs no session; a panic becomes an error.
    pub(super) fn run(self) -> RemovalOutcome {
        let Self { ready, platform, busy } = self;
        let result = guard::guarded(&ready.op, || act_with(platform.as_ref(), ready.mode, &ready.path, None));
        RemovalOutcome { ready, result, busy }
    }
}

/// Journals and prunes after a detached removal, then lets other deletions proceed again.
pub(super) fn finish_removal(session: &mut Session, done: RemovalOutcome) -> Result<Value, CommandError> {
    let RemovalOutcome { ready, result, busy } = done;
    let answer = complete(session, *ready, result);
    drop(busy);
    answer
}

/// `file.delete_permanent` up to the point where the disk is touched, claiming the session's
/// removal slot so no other destructive command or scan can start meanwhile.
pub(super) fn begin_permanent(session: &Session, params: &Value) -> Result<Begun, CommandError> {
    let p: PermanentParams = parse_params(params)?;
    let (addr, dry_run, allow_partial) = permanent_args(&p)?;
    begin_removal(session, "file.delete_permanent", Mode::Permanent, addr, dry_run, allow_partial)
}

/// `file.delete` (recycle bin) up to the point where the disk is touched; the shell can take long
/// for a big folder, so it runs without the session too (G-062).
pub(super) fn begin_recycle(session: &Session, params: &Value) -> Result<Begun, CommandError> {
    let p: DeleteParams = parse_params(params)?;
    let addr = Addr { id: p.id, revision: p.revision, path: p.path.as_deref() };
    begin_removal(session, "file.delete", Mode::Recycle, addr, p.dry_run.unwrap_or(false), false)
}

fn begin_removal(
    session: &Session,
    op: &str,
    mode: Mode,
    addr: Addr<'_>,
    dry_run: bool,
    allow_partial: bool,
) -> Result<Begun, CommandError> {
    match stage(session, op, mode, addr, dry_run, None, allow_partial)? {
        Staged::Report(report) => Ok(Begun::Report(report)),
        Staged::Act(ready) => {
            let busy = BusyGuard::claim(&session.removal_busy).ok_or_else(|| {
                CommandError::NotEnabled(format!("{op}: a deletion is still running; wait for it to finish"))
            })?;
            Ok(Begun::Job(RemovalJob { ready, platform: session.platform(), busy }))
        }
    }
}

/// Parameters of `file.delete_permanent` after the confirmation check: the address, `dry_run` and
/// `i_understand_partial`.
fn permanent_args(p: &PermanentParams) -> Result<(Addr<'_>, bool, bool), CommandError> {
    let dry_run = p.dry_run.unwrap_or(false);
    if !dry_run && p.i_understand_permanent != Some(true) {
        return Err(CommandError::ConfirmationRequired("file.delete_permanent: i_understand_permanent".to_owned()));
    }
    let addr = Addr { id: p.id, revision: p.revision, path: p.path.as_deref() };
    Ok((addr, dry_run, p.i_understand_partial == Some(true)))
}

fn remove_on_disk(path: &Path) -> Result<(), CommandError> {
    fsops::remove_permanently(path).map_err(|e| CommandError::Platform(format!("permanent deletion failed: {e}")))
}

/// Does one already-checked operation on `path` (recycle, permanent removal or move). The tree and
/// the journal are not touched.
fn act(session: &Session, mode: Mode, path: &str, destination: Option<&str>) -> Result<(), CommandError> {
    act_with(session.platform().as_ref(), mode, path, destination)
}

/// [`act`] with the platform passed in, for callers that run without the session.
pub(super) fn act_with(
    platform: &dyn PlatformServices,
    mode: Mode,
    path: &str,
    destination: Option<&str>,
) -> Result<(), CommandError> {
    let target = Path::new(path);
    match mode {
        Mode::Recycle => recycle(platform, target),
        Mode::Permanent => remove_on_disk(target),
        Mode::Move => match destination {
            Some(dest) => platform.rename_no_replace(target, Path::new(dest)).map_err(platform_error),
            None => Ok(()),
        },
    }
}

/// Journals the outcome of an action, success or failure. A failed permanent removal may have
/// deleted part of a folder: the session is then flagged `rescan_recommended`.
pub(super) fn record(
    session: &mut Session,
    op: &str,
    mode: Mode,
    path: &str,
    destination: Option<&str>,
    size: u64,
    result: &Result<(), CommandError>,
) {
    if result.is_err() && mode == Mode::Permanent {
        session.rescan_recommended = true;
    }
    session.journal.record(JournalEntry {
        time: now_secs(),
        op: op.to_owned(),
        path: path.to_owned(),
        destination: destination.map(str::to_owned),
        size,
        mode: mode.name().to_owned(),
        ok: result.is_ok(),
        error: result.as_ref().err().map(ToString::to_string),
    });
}

/// Sends `target` to the recycle bin. A platform without a bin for the volume is `unsupported`
/// with a pointer to `file.delete_permanent`; a "success" that leaves the item in place is an error.
fn recycle(platform: &dyn PlatformServices, target: &Path) -> Result<(), CommandError> {
    platform.move_to_recycle_bin(target).map_err(|e| match platform_error(e) {
        CommandError::Unsupported(what) => CommandError::Unsupported(format!(
            "{what}; the item was not touched (use file.delete_permanent to delete it for good)"
        )),
        other => other,
    })?;
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(CommandError::Platform("the recycle bin reported success but the item is still there".to_owned()));
    }
    Ok(())
}

fn run_journal(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: JournalParams = parse_params(params)?;
    let limit = page_limit(p.limit)?;
    let offset = usize::try_from(p.offset.unwrap_or(0)).unwrap_or(usize::MAX);
    let entries = session.journal.newest_first(offset, limit);
    Ok(json!({ "total": session.journal.len(), "offset": offset, "entries": entries }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_platform::{FakePlatform, PlatformCall};
    use std::path::PathBuf;

    struct Fixture {
        dir: tempfile::TempDir,
        s: Session,
        fake: Arc<FakePlatform>,
    }

    /// Temp dir with `root/{keep.txt, big.bin, sub/{a.txt, deep/b.txt}}`, scanned, with a fake platform.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();
        std::fs::write(root.join("big.bin"), vec![7u8; 3000]).unwrap();
        std::fs::write(root.join("sub").join("a.txt"), b"aaaa").unwrap();
        std::fs::write(root.join("sub").join("deep").join("b.txt"), b"bb").unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(&root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        Fixture { dir, s, fake }
    }

    impl Fixture {
        fn root(&self) -> PathBuf {
            self.dir.path().join("root")
        }
        fn path_of(&self, rel: &str) -> String {
            rel.split('/').fold(self.root(), |p, c| p.join(c)).to_string_lossy().into_owned()
        }
        fn id_of(&self, name: &str) -> u64 {
            let t = self.s.tree();
            (0..t.len())
                .find(|&i| t.node_at(i).and_then(|n| t.get(n)).is_some_and(|n| n.name() == name))
                .map(|i| u64::try_from(i).unwrap())
                .unwrap()
        }
        fn code(&mut self, id: &str, params: Value) -> String {
            self.s.execute(id, &params).unwrap_err().code().to_owned()
        }
    }

    const DESTRUCTIVE: [&str; 3] = ["file.delete", "file.delete_permanent", "file.move"];

    #[test]
    fn destructive_ids_are_flagged_and_refuse_without_confirm() {
        let mut f = fixture();
        let by_id: Vec<_> = f.s.commands().into_iter().filter(|c| c.id.starts_with("file.")).collect();
        for id in DESTRUCTIVE {
            assert!(by_id.iter().any(|c| c.id == id && c.destructive), "{id}");
        }
        for id in ["file.delete_preview", "file.journal"] {
            assert!(by_id.iter().any(|c| c.id == id && !c.destructive), "{id}");
        }
        let path = f.path_of("keep.txt");
        for id in DESTRUCTIVE {
            for p in
                [json!({"path": path}), json!({"path": path, "confirm": false}), json!({"path": path, "confirm": 1})]
            {
                assert_eq!(f.code(id, p), "confirmation_required", "{id}");
            }
        }
        assert!(Path::new(&path).exists());
        assert!(f.fake.calls().is_empty());
    }

    #[test]
    fn dry_run_and_preview_change_nothing() {
        let mut f = fixture();
        let sub = f.path_of("sub");
        let dry = f.s.execute("file.delete", &json!({"path": sub, "dry_run": true, "confirm": true})).unwrap();
        assert_eq!(dry["dry_run"], true);
        assert_eq!(dry["mode"], "recycle");
        assert_eq!((dry["files"].as_u64(), dry["dirs"].as_u64(), dry["count"].as_u64()), (Some(2), Some(2), Some(4)));
        assert_eq!(dry["size_logical"], 6);
        assert_eq!(dry["paths"][0], sub.as_str());
        assert_eq!(dry["paths"].as_array().unwrap().len(), 4);
        assert_eq!(dry["truncated"], false);
        let preview =
            f.s.execute("file.delete_preview", &json!({"id": f.id_of("sub"), "revision": f.s.revision()})).unwrap();
        assert_eq!(preview, dry);
        let perm = f.s.execute("file.delete_preview", &json!({"path": sub, "mode": "permanent"})).unwrap();
        assert_eq!(perm["mode"], "permanent");
        let perm_dry =
            f.s.execute("file.delete_permanent", &json!({"path": sub, "dry_run": true, "confirm": true})).unwrap();
        assert_eq!(perm_dry["dry_run"], true);
        let dest = f.path_of("moved");
        let mv =
            f.s.execute("file.move", &json!({"path": sub, "destination": dest, "dry_run": true, "confirm": true}))
                .unwrap();
        assert_eq!(mv["mode"], "move");
        assert_eq!(mv["rescan_recommended"], true);
        assert!(Path::new(&sub).exists() && !Path::new(&dest).exists());
        assert!(f.fake.calls().is_empty());
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["total"], 0);
        assert_eq!(f.s.tree().len(), 7);
    }

    #[test]
    fn recycle_goes_through_the_platform_and_updates_the_tree() {
        let mut f = fixture();
        let big = f.path_of("big.bin");
        f.s.select_node(f.s.tree().node_at(f.id_of("big.bin") as usize).unwrap());
        let before = f.s.revision();
        let out = f.s.execute("file.delete", &json!({"path": big, "confirm": true})).unwrap();
        assert_eq!(out["done"], true);
        assert_eq!(out["tree_updated"], true);
        assert_eq!(out["ids_invalidated"], true);
        assert!(f.s.revision() > before);
        assert_eq!(f.fake.calls(), vec![PlatformCall::MoveToRecycleBin(PathBuf::from(&big))]);
        assert!(!Path::new(&big).exists());
        assert_eq!(f.fake.binned().len(), 1);
        assert!(f.s.selection().is_none());
        let t = f.s.tree();
        assert_eq!(t.len(), 6);
        assert_eq!(t.get(t.root()).unwrap().file_count(), 3);
        // The same path no longer resolves to a node.
        assert_eq!(f.code("file.delete", json!({"path": big, "confirm": true})), "invalid_params");
        let journal = f.s.execute("file.journal", &json!({})).unwrap();
        assert_eq!(journal["total"], 1);
        assert_eq!(journal["entries"][0]["op"], "file.delete");
        assert_eq!(journal["entries"][0]["mode"], "recycle");
        assert_eq!(journal["entries"][0]["ok"], true);
    }

    #[test]
    fn platform_failure_is_journaled_and_keeps_the_tree() {
        let mut f = fixture();
        f.fake.set_failure(Some(dirustat_platform::PlatformError::PermissionDenied("x".into())));
        let keep = f.path_of("keep.txt");
        assert_eq!(f.code("file.delete", json!({"path": keep, "confirm": true})), "internal");
        assert!(Path::new(&keep).exists());
        assert_eq!(f.s.tree().len(), 7);
        let journal = f.s.execute("file.journal", &json!({"limit": 5})).unwrap();
        assert_eq!(journal["entries"][0]["ok"], false);
        assert!(journal["entries"][0]["error"].is_string());
    }

    #[test]
    fn permanent_needs_both_flags() {
        let mut f = fixture();
        let sub = f.path_of("sub");
        for p in [
            json!({"path": sub, "confirm": true}),
            json!({"path": sub, "confirm": true, "i_understand_permanent": false}),
            json!({"path": sub, "i_understand_permanent": true}),
        ] {
            assert_eq!(f.code("file.delete_permanent", p), "confirmation_required");
        }
        let wrong_type = json!({"path": sub, "confirm": true, "i_understand_permanent": "yes"});
        assert_eq!(f.code("file.delete_permanent", wrong_type), "invalid_params");
        assert!(Path::new(&sub).exists());
        let out = f
            .s
            .execute("file.delete_permanent", &json!({"path": sub, "confirm": true, "i_understand_permanent": true}))
            .unwrap();
        assert_eq!(out["mode"], "permanent");
        assert_eq!(out["done"], true);
        assert!(!Path::new(&sub).exists());
        assert!(f.fake.calls().is_empty());
        assert_eq!(f.s.tree().len(), 3);
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["entries"][0]["mode"], "permanent");
    }

    #[test]
    fn protected_paths_are_refused_and_nothing_is_touched() {
        let mut f = fixture();
        // The scan root itself, by id and by path with decorations.
        let root = f.root().to_string_lossy().into_owned();
        for p in [
            json!({"id": 0, "revision": f.s.revision()}),
            json!({"path": root}),
            json!({"path": format!("{root}{}", std::path::MAIN_SEPARATOR)}),
            json!({"path": format!("{root}/sub/..")}),
        ] {
            let mut p = p;
            p["confirm"] = json!(true);
            assert_eq!(f.code("file.delete", p.clone()), "protected_path", "{p}");
            p["i_understand_permanent"] = json!(true);
            assert_eq!(f.code("file.delete_permanent", p), "protected_path");
        }
        let dest = f.path_of("elsewhere");
        assert_eq!(
            f.code("file.move", json!({"id": 0, "revision": f.s.revision(), "destination": dest, "confirm": true})),
            "protected_path"
        );
        assert!(f.root().join("keep.txt").exists());
        assert!(f.fake.calls().is_empty());
    }

    #[test]
    fn paths_outside_the_tree_are_refused() {
        let mut f = fixture();
        let outside = f.dir.path().join("outside.txt");
        std::fs::write(&outside, b"x").unwrap();
        for path in [
            outside.to_string_lossy().into_owned(),
            f.path_of("nope.txt"),
            f.path_of("sub/../../outside.txt"),
            "relative.txt".to_owned(),
            String::new(),
        ] {
            assert_eq!(f.code("file.delete", json!({"path": path, "confirm": true})), "invalid_params", "{path}");
        }
        assert!(outside.exists());
        assert!(f.fake.calls().is_empty());
    }

    #[test]
    fn stale_tree_is_refused_without_side_effects() {
        let mut f = fixture();
        // Changed after the scan.
        let keep = f.path_of("keep.txt");
        std::fs::write(&keep, b"changed content").unwrap();
        assert_eq!(f.code("file.delete", json!({"path": keep, "confirm": true})), "stale_tree");
        // Removed after the scan.
        let big = f.path_of("big.bin");
        std::fs::remove_file(&big).unwrap();
        assert_eq!(f.code("file.delete", json!({"path": big, "confirm": true})), "stale_tree");
        assert_eq!(f.code("file.delete_preview", json!({"path": big})), "stale_tree");
        // A file replaced by a directory of the same name.
        let a = f.path_of("sub/a.txt");
        std::fs::remove_file(&a).unwrap();
        std::fs::create_dir(&a).unwrap();
        assert_eq!(
            f.code("file.delete_permanent", json!({"path": a, "confirm": true, "i_understand_permanent": true})),
            "stale_tree"
        );
        assert!(Path::new(&a).is_dir() && Path::new(&keep).exists());
        assert!(f.fake.calls().is_empty());
        assert_eq!(f.s.tree().len(), 7);
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["total"], 0);
    }

    #[test]
    fn move_renames_without_overwrite() {
        let mut f = fixture();
        let src = f.path_of("keep.txt");
        let dest = f.dir.path().join("moved.txt").to_string_lossy().into_owned();
        let existing = f.path_of("big.bin");
        assert_eq!(
            f.code("file.move", json!({"path": src, "destination": existing, "confirm": true})),
            "invalid_params"
        );
        assert_eq!(
            f.code("file.move", json!({"path": src, "destination": "rel.txt", "confirm": true})),
            "invalid_params"
        );
        let missing_parent = f.dir.path().join("no").join("x.txt").to_string_lossy().into_owned();
        assert_eq!(
            f.code("file.move", json!({"path": src, "destination": missing_parent, "confirm": true})),
            "invalid_params"
        );
        let sub = f.path_of("sub");
        let into_self = f.path_of("sub/deep/sub2");
        assert_eq!(
            f.code("file.move", json!({"path": sub, "destination": into_self, "confirm": true})),
            "invalid_params"
        );
        assert!(Path::new(&src).exists());
        let out = f.s.execute("file.move", &json!({"path": src, "destination": dest, "confirm": true})).unwrap();
        assert_eq!(out["done"], true);
        assert_eq!(out["rescan_recommended"], false);
        assert!(!Path::new(&src).exists() && Path::new(&dest).exists());
        assert_eq!(f.s.tree().len(), 6);
        let journal = f.s.execute("file.journal", &json!({})).unwrap();
        assert_eq!(journal["entries"][0]["mode"], "move");
        assert_eq!(journal["entries"][0]["destination"], dest.as_str());
    }

    #[test]
    fn journal_file_is_written_when_configured() {
        let mut f = fixture();
        let file = f.dir.path().join("journal.jsonl");
        f.s.set_journal_file(Some(file.clone()));
        let keep = f.path_of("keep.txt");
        f.s.execute("file.delete", &json!({"path": keep, "confirm": true})).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("file.delete"));
    }

    #[test]
    fn journal_paging_is_validated() {
        let mut f = fixture();
        for p in [json!({"limit": 0}), json!({"limit": 10_001}), json!({"limit": -1}), json!({"bogus": 1})] {
            assert_eq!(f.code("file.journal", p), "invalid_params");
        }
        assert!(f.s.execute("file.journal", &json!({"offset": u64::MAX})).is_ok());
    }

    #[test]
    fn hostile_params_never_delete_anything() {
        let mut f = fixture();
        let long = "x".repeat(100_000);
        let hostile = [
            json!({}),
            json!({"id": u64::MAX}),
            json!({"id": -1}),
            json!({"id": 0, "revision": 0}),
            json!({"id": 1, "path": "x"}),
            json!({"path": ""}),
            json!({"path": long}),
            json!({"path": "a\u{0}b"}),
            json!({"path": "..\\..\\..\\Windows"}),
            json!({"path": "C:\\"}),
            json!({"path": "/"}),
            json!({"path": 5}),
            json!({"path": f.path_of("keep.txt"), "destination": 5}),
            json!({"path": f.path_of("keep.txt"), "destination": ""}),
            json!({"path": f.path_of("keep.txt"), "destination": "\\\\?\\GLOBALROOT\\x"}),
            json!({"unknown": 1}),
        ];
        for id in DESTRUCTIVE {
            for mut p in hostile.iter().cloned() {
                if let Some(map) = p.as_object_mut() {
                    map.insert("confirm".to_owned(), json!(true));
                    map.insert("i_understand_permanent".to_owned(), json!(true));
                    if id != "file.delete_permanent" {
                        map.remove("i_understand_permanent");
                    }
                }
                match f.s.execute(id, &p) {
                    Err(CommandError::Panicked { .. }) => panic!("{id} panicked on {p}"),
                    Ok(v) => panic!("{id} succeeded on {p}: {v}"),
                    Err(_) => {}
                }
            }
        }
        assert!(f.root().join("keep.txt").exists() && f.root().join("sub").exists());
        assert!(f.fake.calls().is_empty());
        assert_eq!(f.s.tree().len(), 7);
    }

    #[test]
    fn empty_session_disables_the_commands() {
        let mut s = Session::new();
        for c in s.commands().iter().filter(|c| DESTRUCTIVE.contains(&c.id) || c.id == "file.delete_preview") {
            assert!(!c.enabled, "{}", c.id);
        }
        let err = s.execute("file.delete", &json!({"path": "C:\\x", "confirm": true})).unwrap_err();
        assert_eq!(err.code(), "not_enabled");
    }

    #[test]
    fn report_listing_is_capped_but_counts_are_complete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let many = root.join("many");
        std::fs::create_dir_all(&many).unwrap();
        for i in 0..(MAX_LISTED + 25) {
            std::fs::write(many.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let mut s = Session::new();
        let handle = dirustat_scan::start(&root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let out = s.execute("file.delete_preview", &json!({"path": many.to_string_lossy()})).unwrap();
        assert_eq!(out["files"], MAX_LISTED + 25);
        assert_eq!(out["paths"].as_array().unwrap().len(), MAX_LISTED);
        assert_eq!(out["truncated"], true);
    }

    #[test]
    fn ids_need_the_revision_they_were_read_at() {
        let mut f = fixture();
        let rev = f.s.revision();
        let (keep, big) = (f.id_of("keep.txt"), f.id_of("big.bin"));
        // No revision, or a wrong one: refused without side effects, for every command.
        for id in ["file.delete", "file.delete_permanent", "file.move", "file.delete_preview"] {
            let mut p = json!({"id": keep, "confirm": true, "i_understand_permanent": true});
            if id == "file.delete_preview" {
                p = json!({"id": keep});
            }
            if id == "file.move" {
                p["destination"] = json!(f.path_of("moved.txt"));
            }
            if id != "file.delete_permanent" {
                p.as_object_mut().unwrap().remove("i_understand_permanent");
            }
            assert_eq!(f.code(id, p.clone()), "invalid_params", "{id} without revision");
            p["revision"] = json!(rev + 1);
            assert_eq!(f.code(id, p), "stale_tree", "{id} with a wrong revision");
        }
        // A wrong revision is stale for path addressing too.
        let path = f.path_of("keep.txt");
        assert_eq!(f.code("file.delete", json!({"path": path, "revision": rev + 7, "confirm": true})), "stale_tree");
        assert!(f.fake.calls().is_empty() && Path::new(&path).exists());
        // Delete id A at the current revision; the old revision is then refused for id B.
        let out = f.s.execute("file.delete", &json!({"id": keep, "revision": rev, "confirm": true})).unwrap();
        assert_eq!(out["done"], true);
        assert!(f.s.revision() > rev);
        let big_path = f.path_of("big.bin");
        assert_eq!(f.code("file.delete", json!({"id": big, "revision": rev, "confirm": true})), "stale_tree");
        assert!(Path::new(&big_path).exists());
        assert_eq!(f.fake.calls().len(), 1);
        // The fresh revision works again.
        let id = f.id_of("big.bin");
        let now = f.s.revision();
        f.s.execute("file.delete", &json!({"id": id, "revision": now, "confirm": true})).unwrap();
        assert!(!Path::new(&big_path).exists());
    }

    #[test]
    fn listings_expose_the_revision() {
        let mut f = fixture();
        let rev = f.s.revision();
        assert_eq!(f.s.execute("tree.children", &json!({})).unwrap()["revision"], rev);
        assert_eq!(f.s.execute("list.largest_files", &json!({})).unwrap()["revision"], rev);
        assert_eq!(f.s.execute("search.find", &json!({"query": "a"})).unwrap()["revision"], rev);
        assert_eq!(f.s.execute("session.state", &json!({})).unwrap()["revision"], rev);
    }

    #[test]
    fn report_work_is_bounded_for_huge_folders() {
        let mut tree = dirustat_model::Tree::new("root");
        let many = tree.add_child(tree.root(), "many", NodeKind::Dir, 0).unwrap();
        let n = 200_000usize;
        for i in 0..n {
            tree.add_child(many, &format!("f{i}"), NodeKind::File, 1).unwrap();
        }
        tree.aggregate();
        let mut s = Session::new();
        s.replace_tree(tree);
        let node = s.tree().children(s.tree().root()).next().unwrap();
        let report = build_report(&s, node);
        assert_eq!(report.files, u64::try_from(n).unwrap());
        assert_eq!(report.paths.len(), MAX_LISTED);
        assert!(report.truncated);
        assert_eq!(report.built_paths, MAX_LISTED, "path strings are built only for the listed nodes");
    }

    #[test]
    fn partial_scans_need_an_extra_flag_for_permanent_folder_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), b"aaaa").unwrap();
        std::fs::write(sub.join("skip.log"), b"unseen").unwrap();
        let mut s = Session::new();
        let opts = dirustat_scan::ScanOptions { exclude: vec!["*.log".to_owned()], ..Default::default() };
        let handle = dirustat_scan::start(&root, opts).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        s.set_platform(Arc::new(FakePlatform::new()));
        let sub_s = sub.to_string_lossy().into_owned();
        let preview = s.execute("file.delete_preview", &json!({"path": sub_s, "mode": "permanent"})).unwrap();
        assert_eq!(preview["partial_tree"], true);
        assert_eq!(s.execute("session.state", &json!({})).unwrap()["partial_scan"], true);
        let p = json!({"path": sub_s, "confirm": true, "i_understand_permanent": true});
        assert_eq!(s.execute("file.delete_permanent", &p).unwrap_err().code(), "confirmation_required");
        assert!(sub.join("skip.log").exists());
        // Dry run needs no extra flag; recycling is reversible and needs none either.
        let dry = json!({"path": sub_s, "confirm": true, "i_understand_permanent": true, "dry_run": true});
        assert_eq!(s.execute("file.delete_permanent", &dry).unwrap()["partial_tree"], true);
        let with =
            json!({"path": sub_s, "confirm": true, "i_understand_permanent": true, "i_understand_partial": true});
        assert_eq!(s.execute("file.delete_permanent", &with).unwrap()["done"], true);
        assert!(!sub.exists());
        // A complete scan reports no partial tree.
        let mut full = fixture();
        let out = full.s.execute("file.delete_preview", &json!({"path": full.path_of("sub")})).unwrap();
        assert_eq!(out["partial_tree"], false);
    }

    #[test]
    fn subtrees_with_read_errors_are_partial() {
        let mut t = dirustat_model::Tree::new("root");
        let bad = t.add_child(t.root(), "bad", NodeKind::Dir, 0).unwrap();
        let good = t.add_child(t.root(), "good", NodeKind::Dir, 0).unwrap();
        t.add_child(good, "f", NodeKind::File, 1).unwrap();
        t.mark_error(bad).unwrap();
        t.aggregate();
        let mut s = Session::new();
        s.replace_tree(t);
        let ids: Vec<_> = s.tree().children(s.tree().root()).collect();
        let flags: Vec<bool> = ids.iter().map(|&id| subtree_partial(&s, id)).collect();
        let names: Vec<_> = ids.iter().filter_map(|&id| s.tree().get(id)).map(|n| n.name().to_owned()).collect();
        assert_eq!(names.iter().zip(&flags).find(|(n, _)| *n == "bad").map(|(_, f)| *f), Some(true));
        assert_eq!(names.iter().zip(&flags).find(|(n, _)| *n == "good").map(|(_, f)| *f), Some(false));
    }

    #[test]
    fn recycle_without_a_bin_or_with_a_lying_shell_is_an_error() {
        let mut f = fixture();
        let keep = f.path_of("keep.txt");
        f.fake.set_no_recycle_bin(true);
        let err = f.s.execute("file.delete", &json!({"path": keep, "confirm": true})).unwrap_err();
        assert_eq!(err.code(), "unsupported");
        assert!(err.to_string().contains("file.delete_permanent"), "{err}");
        assert!(Path::new(&keep).exists());
        assert_eq!(f.s.tree().len(), 7);
        f.fake.set_no_recycle_bin(false);
        f.fake.set_leave_in_place(true);
        assert_eq!(f.code("file.delete", json!({"path": keep, "confirm": true})), "internal");
        assert!(Path::new(&keep).exists());
        assert_eq!(f.s.tree().len(), 7, "a recycle that left the item must not prune the tree");
        let journal = f.s.execute("file.journal", &json!({})).unwrap();
        assert_eq!(journal["entries"][0]["ok"], false);
    }

    #[test]
    fn move_uses_the_no_replace_primitive_and_checks_names() {
        let mut f = fixture();
        let src = f.path_of("keep.txt");
        let dest = f.dir.path().join("moved.txt").to_string_lossy().into_owned();
        f.s.execute("file.move", &json!({"path": src, "destination": dest, "confirm": true})).unwrap();
        assert_eq!(f.fake.calls(), vec![PlatformCall::Rename(PathBuf::from(&src), PathBuf::from(&dest))]);
        // The platform refuses an existing destination even if it appeared after our own check.
        let big = f.path_of("big.bin");
        f.fake.set_failure(Some(dirustat_platform::PlatformError::Unsupported("rename_no_replace")));
        assert_eq!(
            f.code("file.move", json!({"path": big, "destination": f.path_of("x.bin"), "confirm": true})),
            "unsupported"
        );
        assert!(Path::new(&big).exists());
        let names =
            |c: &[&str]| destination_component_problem(&c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(), true);
        assert!(names(&["a", "b:stream"]).is_some());
        assert!(names(&["a", "b."]).is_some());
        assert!(names(&["a ", "b"]).is_some());
        assert!(names(&["a", "b?"]).is_some());
        assert!(names(&["a", "b.txt"]).is_none());
        assert!(destination_component_problem(&["a:b".to_owned()], false).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn move_rejects_stream_and_trailing_dot_destinations() {
        let mut f = fixture();
        let src = f.path_of("keep.txt");
        for name in ["moved.txt:evil", "moved.", "moved "] {
            let dest = format!("{}\\{name}", f.dir.path().display());
            assert_eq!(
                f.code("file.move", json!({"path": src, "destination": dest, "confirm": true})),
                "invalid_params"
            );
        }
        assert!(Path::new(&src).exists() && f.fake.calls().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn network_scan_roots_refuse_every_destructive_command() {
        let mut tree = dirustat_model::Tree::new(r"\\localhost\share\data");
        tree.add_child(tree.root(), "f.txt", NodeKind::File, 5).unwrap();
        tree.aggregate();
        let mut s = Session::new();
        s.replace_tree(tree);
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        let path = r"\\localhost\share\data\f.txt";
        for (id, p) in [
            ("file.delete", json!({"path": path, "confirm": true})),
            ("file.delete_permanent", json!({"path": path, "confirm": true, "i_understand_permanent": true})),
            ("file.move", json!({"path": path, "destination": r"C:\x\y.txt", "confirm": true})),
            ("file.delete_preview", json!({"path": path})),
        ] {
            let err = s.execute(id, &p).unwrap_err();
            assert_eq!(err.code(), "protected_path", "{id}: {err}");
            assert!(err.to_string().contains("UNC"), "{err}");
        }
        assert!(fake.calls().is_empty());
    }

    /// QA-145: a lettered root is trusted only when the drive really is a local disk. A letter with
    /// no volume (or a mapped network drive, whose type is "remote") is refused like a UNC root.
    #[cfg(windows)]
    #[test]
    fn lettered_roots_on_non_local_drives_refuse_every_destructive_command() {
        let free = ('D'..='Z').find(|c| !Path::new(&format!("{c}:\\")).exists()).unwrap();
        let root = format!("{free}:\\data");
        let mut tree = dirustat_model::Tree::new(&root);
        tree.add_child(tree.root(), "f.txt", NodeKind::File, 5).unwrap();
        tree.aggregate();
        let mut s = Session::new();
        s.replace_tree(tree);
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        let path = format!("{root}\\f.txt");
        for (id, p) in [
            ("file.delete", json!({"path": path, "confirm": true})),
            ("file.delete_permanent", json!({"path": path, "confirm": true, "i_understand_permanent": true})),
            ("file.move", json!({"path": path, "destination": r"C:\x\y.txt", "confirm": true})),
            ("file.delete_preview", json!({"path": path})),
        ] {
            let err = s.execute(id, &p).unwrap_err();
            assert_eq!(err.code(), "protected_path", "{id}: {err}");
        }
        assert!(fake.calls().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn failed_permanent_removal_flags_a_rescan() {
        use std::os::windows::fs::OpenOptionsExt;
        let mut f = fixture();
        let locked = f.root().join("sub").join("deep").join("b.txt");
        // An exclusive handle (no FILE_SHARE_DELETE) makes the removal fail half-way.
        let _hold = std::fs::OpenOptions::new().read(true).share_mode(0).open(&locked).unwrap();
        let sub = f.path_of("sub");
        let p = json!({"path": sub, "confirm": true, "i_understand_permanent": true});
        assert_eq!(f.s.execute("file.delete_permanent", &p).unwrap_err().code(), "internal");
        assert_eq!(f.s.execute("session.state", &json!({})).unwrap()["rescan_recommended"], true);
        let journal = f.s.execute("file.journal", &json!({})).unwrap();
        assert_eq!(journal["entries"][0]["ok"], false);
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_not_followed_and_only_the_link_goes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"keep me").unwrap();
        let link = root.join("junc");
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .is_ok_and(|o| o.status.success());
        if !made {
            return; // junction creation unavailable here
        }
        let mut s = Session::new();
        let handle = dirustat_scan::start(&root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        let link_s = link.to_string_lossy().into_owned();
        // Recycling a link is refused; the platform is never asked.
        let err = s.execute("file.delete", &json!({"path": link_s, "confirm": true})).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        assert!(fake.calls().is_empty());
        // A path that passes through the junction does not resolve in the tree.
        let through = link.join("precious.txt").to_string_lossy().into_owned();
        assert!(s.execute("file.delete", &json!({"path": through, "confirm": true})).is_err());
        assert!(target.join("precious.txt").exists());
        // Permanent deletion removes the link itself and leaves the target alone.
        let p = json!({"path": link_s, "confirm": true, "i_understand_permanent": true});
        s.execute("file.delete_permanent", &p).unwrap();
        assert!(!link.exists());
        assert!(target.join("precious.txt").exists());
    }

    /// Scans `root` with a fake platform.
    fn scanned(root: &Path) -> (Session, Arc<FakePlatform>) {
        let mut s = Session::new();
        let handle = dirustat_scan::start(root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        (s, fake)
    }

    fn id_named(s: &Session, name: &str) -> u64 {
        let t = s.tree();
        (0..t.len())
            .find(|&i| t.node_at(i).and_then(|n| t.get(n)).is_some_and(|n| n.name() == name))
            .map(|i| u64::try_from(i).unwrap())
            .unwrap()
    }

    /// QA-116: `dup` and `dup.` are two files on NTFS but Win32 normalisation maps both paths to `dup`.
    #[cfg(windows)]
    #[test]
    fn trailing_dot_names_never_reach_the_wrong_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("dup"), b"same-size-data").unwrap();
        let verbatim = format!(r"\\?\{}\dup.", root.display());
        std::fs::write(&verbatim, b"same-size-data").unwrap();
        let (mut s, fake) = scanned(&root);
        let (rev, dot) = (s.revision(), id_named(&s, "dup."));
        for id in ["file.delete", "file.delete_permanent"] {
            let mut p = json!({"id": dot, "revision": rev, "confirm": true});
            if id == "file.delete_permanent" {
                p["i_understand_permanent"] = json!(true);
            }
            assert_eq!(s.execute(id, &p).unwrap_err().code(), "unsupported", "{id}");
        }
        assert!(root.join("dup").exists() && std::fs::metadata(&verbatim).is_ok());
        assert!(fake.calls().is_empty());
        // The plainly named file is still deletable: the check refuses ambiguity, not the whole folder.
        let plain = id_named(&s, "dup");
        s.execute("file.delete", &json!({"id": plain, "revision": rev, "confirm": true})).unwrap();
        assert!(!root.join("dup").exists() && std::fs::metadata(&verbatim).is_ok());
    }

    /// QA-116: a lone surrogate and a literal U+FFFD collapse to the same stored name.
    #[cfg(windows)]
    #[test]
    fn lossy_names_are_flagged_and_refused() {
        use std::os::windows::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let wide: Vec<u16> = "x".encode_utf16().chain([0xD800]).chain(".tmp".encode_utf16()).collect();
        let lone = root.join(std::ffi::OsString::from_wide(&wide));
        std::fs::write(&lone, b"0123456789").unwrap();
        std::fs::write(root.join("x\u{FFFD}.tmp"), b"0123456789").unwrap();
        let (mut s, fake) = scanned(&root);
        let t = s.tree();
        let flags: Vec<bool> = t.children(t.root()).filter_map(|c| t.get(c)).map(|n| n.data().name_lossy).collect();
        assert_eq!(flags.iter().filter(|f| **f).count(), 1, "exactly the lone-surrogate node is lossy");
        let rev = s.revision();
        let lossy_id = t
            .children(t.root())
            .find(|&c| t.get(c).is_some_and(|n| n.data().name_lossy))
            .map(|c| u64::try_from(c.index()).unwrap())
            .unwrap();
        let err = s.execute("file.delete", &json!({"id": lossy_id, "revision": rev, "confirm": true})).unwrap_err();
        assert_eq!(err.code(), "unsupported");
        assert!(lone.exists() && root.join("x\u{FFFD}.tmp").exists());
        assert!(fake.calls().is_empty());
    }

    /// QA-118: content added or rewritten deep inside a folder after the preview is not deleted unseen.
    #[test]
    fn permanent_folder_delete_detects_deep_changes() {
        let mut f = fixture();
        let sub = f.path_of("sub");
        let dry = f.s.execute("file.delete_permanent", &json!({"path": sub, "dry_run": true, "confirm": true}));
        assert_eq!(dry.unwrap()["count"], 4);
        let new_file = f.root().join("sub").join("deep").join("NEW_UNSEEN.docx");
        std::fs::write(&new_file, b"precious").unwrap();
        let p = json!({"path": sub, "confirm": true, "i_understand_permanent": true});
        let err = f.s.execute("file.delete_permanent", &p).unwrap_err();
        assert_eq!(err.code(), "stale_tree");
        assert!(err.to_string().contains("NEW_UNSEEN.docx"), "{err}");
        assert!(new_file.exists() && f.root().join("sub").exists());
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["total"], 0);
        // A move of the folder is refused for the same reason.
        let mv = json!({"path": sub, "destination": f.path_of("moved"), "confirm": true});
        assert_eq!(f.s.execute("file.move", &mv).unwrap_err().code(), "stale_tree");
        // Rewriting an existing file at depth is caught too.
        std::fs::remove_file(&new_file).unwrap();
        std::fs::write(f.root().join("sub").join("deep").join("b.txt"), b"rewritten, longer").unwrap();
        assert_eq!(f.s.execute("file.delete_permanent", &p).unwrap_err().code(), "stale_tree");
        assert!(f.root().join("sub").join("deep").join("b.txt").exists());
    }

    #[test]
    fn recycle_reports_subtree_drift_as_a_warning() {
        let mut f = fixture();
        let sub = f.path_of("sub");
        let clean = f.s.execute("file.delete_preview", &json!({"path": sub})).unwrap();
        assert!(clean["subtree_changed"].is_null());
        std::fs::write(f.root().join("sub").join("deep").join("NEW.docx"), b"new").unwrap();
        let drift = f.s.execute("file.delete_preview", &json!({"path": sub})).unwrap();
        assert!(drift["subtree_changed"].as_str().is_some_and(|m| m.contains("NEW.docx")));
        // Recycling stays possible: the data is recoverable.
        let done = f.s.execute("file.delete", &json!({"path": sub, "confirm": true})).unwrap();
        assert_eq!(done["done"], true);
    }

    /// QA-119: every name of a hardlinked file is deletable (the tree stores size 0 for later names).
    #[test]
    fn second_hardlink_name_is_deletable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hl_a.bin"), vec![5u8; 500]).unwrap();
        if std::fs::hard_link(root.join("hl_a.bin"), root.join("hl_b.bin")).is_err() {
            return; // hardlinks unavailable on this file system
        }
        let (mut s, _fake) = scanned(&root);
        let t = s.tree();
        let zero = t.children(t.root()).filter_map(|c| t.get(c)).filter(|n| n.data().logical_size == 0).count();
        assert!(zero > 0 || !cfg!(windows), "the scan must charge a hardlinked file once");
        if zero == 0 {
            return; // no identity query on this platform; nothing to test
        }
        let rev = s.revision();
        for name in ["hl_a.bin", "hl_b.bin"] {
            let out = s.execute("file.delete_preview", &json!({"path": root.join(name).to_string_lossy()}));
            assert!(out.is_ok(), "{name}: {out:?}");
        }
        let second = id_named(&s, "hl_b.bin");
        let second_dup =
            s.tree().get(s.tree().node_at(usize::try_from(second).unwrap()).unwrap()).unwrap().data().attributes
                & dirustat_model::ATTR_HARDLINK_DUPLICATE
                != 0;
        let target = if second_dup { "hl_b.bin" } else { "hl_a.bin" };
        let id = id_named(&s, target);
        s.execute("file.delete", &json!({"id": id, "revision": rev, "confirm": true})).unwrap();
        assert!(!root.join(target).exists());
    }

    /// Temp dir `root` holding `names`, each with the same content and the same modification time
    /// (so only the name can tell the files apart), scanned, with a fake platform.
    fn lookalikes(names: &[&str]) -> (tempfile::TempDir, Session, Arc<FakePlatform>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        for name in names {
            let file = std::fs::File::create(root.join(name)).unwrap();
            std::io::Write::write_all(&mut &file, b"same-size").unwrap();
            file.set_modified(when).unwrap();
        }
        let (s, fake) = scanned(&root);
        (dir, s, fake)
    }

    fn delete_perm(s: &mut Session, path: &Path) -> Result<Value, CommandError> {
        let p = json!({"path": path.to_string_lossy(), "confirm": true, "i_understand_permanent": true});
        s.execute("file.delete_permanent", &p)
    }

    /// QA-141: DEL and C1 control characters are legal in names, so such files and folders are deletable.
    #[test]
    fn c1_control_characters_in_names_are_deletable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let folder = root.join("c1\u{85}dir");
        std::fs::create_dir_all(&folder).unwrap();
        let files = [root.join("del\u{7f}.txt"), root.join("c1\u{85}.txt"), root.join("c1b\u{9f}.txt")];
        for f in &files {
            std::fs::write(f, b"x").unwrap();
        }
        std::fs::write(folder.join("inner.txt"), b"inner").unwrap();
        let (mut s, _fake) = scanned(&root);
        for f in &files {
            let out = delete_perm(&mut s, f).unwrap();
            assert_eq!(out["done"], true, "{f:?}");
            assert!(!f.exists());
        }
        let out = delete_perm(&mut s, &folder).unwrap();
        assert_eq!(out["done"], true);
        assert!(!folder.exists());
    }

    /// QA-142: creating a junction in a folder just before the scan once made the next permanent
    /// delete fail intermittently with "the modification time changed" (a lazily updated folder
    /// time in the directory listing). The original repro, 50 times.
    #[cfg(windows)]
    #[test]
    fn folder_with_a_fresh_junction_is_never_falsely_stale() {
        let mut made_any = false;
        for round in 0..50 {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("root");
            let sub = root.join("sub");
            let elsewhere = dir.path().join("elsewhere");
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::create_dir_all(&elsewhere).unwrap();
            std::fs::write(sub.join("a.txt"), b"a").unwrap();
            let made = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(sub.join("lnk"))
                .arg(&elsewhere)
                .output()
                .is_ok_and(|o| o.status.success());
            if !made {
                return; // junction creation unavailable here
            }
            made_any = true;
            let (mut s, _fake) = scanned(&root);
            let p = json!({"path": sub.to_string_lossy(), "dry_run": true, "confirm": true, "i_understand_permanent": true});
            let out = s.execute("file.delete_permanent", &p);
            assert!(out.is_ok(), "round {round}: {out:?}");
        }
        assert!(made_any);
    }

    /// QA-142: the relaxed folder time still leaves the content protected (QA-118): a file added to
    /// the folder after the scan is refused.
    #[test]
    fn folder_time_relaxation_keeps_the_content_check() {
        let mut f = fixture();
        let sub = f.path_of("sub");
        std::fs::write(f.root().join("sub").join("NEW.docx"), b"new").unwrap();
        let p = json!({"path": sub, "confirm": true, "i_understand_permanent": true});
        let err = f.s.execute("file.delete_permanent", &p).unwrap_err();
        assert_eq!(err.code(), "stale_tree");
        assert!(err.to_string().contains("NEW.docx"), "{err}");
    }

    /// QA-140: a folder holding two names that collide after lossy conversion is not deleted
    /// permanently on the strength of a comparison that cannot tell them apart.
    #[cfg(windows)]
    #[test]
    fn permanent_delete_refuses_a_folder_with_colliding_names() {
        use std::os::windows::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("root").join("sub");
        let wide: Vec<u16> = "d".encode_utf16().chain([0xD800]).collect();
        let (lone, literal) = (std::ffi::OsString::from_wide(&wide), std::ffi::OsString::from("d\u{FFFD}"));
        for name in [&lone, &literal] {
            std::fs::create_dir_all(sub.join(name)).unwrap();
            std::fs::write(sub.join(name).join("a.txt"), b"aaaa").unwrap();
        }
        let (mut s, fake) = scanned(&dir.path().join("root"));
        std::fs::write(sub.join(&lone).join("NEW_UNSEEN.docx"), b"precious").unwrap();
        let err = delete_perm(&mut s, &sub).unwrap_err();
        assert_eq!(err.code(), "stale_tree", "{err}");
        assert!(sub.join(&lone).join("NEW_UNSEEN.docx").exists());
        assert!(fake.calls().is_empty());
    }

    /// QA-139: a request names exactly one node. A trailing no-break space is part of the name.
    #[test]
    fn nbsp_suffix_addresses_the_nbsp_file_and_never_its_sibling() {
        let (dir, mut s, _fake) = lookalikes(&["dupe", "dupe\u{a0}"]);
        let root = dir.path().join("root");
        let out = delete_perm(&mut s, &root.join("dupe\u{a0}")).unwrap();
        assert_eq!(out["done"], true);
        assert!(root.join("dupe").exists(), "the sibling named without NBSP must survive");
        assert!(!root.join("dupe\u{a0}").exists());
    }

    /// QA-139: whitespace around a component is never stripped to reach another file.
    #[test]
    fn surrounding_whitespace_never_reaches_the_plain_name() {
        let (dir, mut s, _fake) = lookalikes(&["dupe"]);
        let root = dir.path().join("root");
        let before = s.revision();
        for name in ["dupe\u{a0}", "dupe\u{3000}", "dupe\t", "\u{a0}dupe", "dupe\u{2003}"] {
            let err = delete_perm(&mut s, &root.join(name)).unwrap_err();
            assert_eq!(err.code(), "invalid_params", "{name:?}: {err}");
        }
        // Whole-request padding is not stripped either: a trailing space, newline or NBSP stays part of the name.
        let base = root.join("dupe").to_string_lossy().into_owned();
        for padded in [format!("{base} "), format!("{base}\n"), format!("{base}\u{a0}"), format!(" {base}")] {
            let p = json!({"path": padded, "confirm": true, "i_understand_permanent": true});
            assert_eq!(s.execute("file.delete_permanent", &p).unwrap_err().code(), "invalid_params", "{padded:?}");
        }
        assert!(root.join("dupe").exists() && s.revision() == before);
    }

    /// QA-139: the Kelvin sign is its own name on NTFS; Rust's `to_lowercase` would fold it to `k`.
    #[test]
    fn kelvin_sign_is_not_k() {
        let (dir, mut s, _fake) = lookalikes(&["k", "\u{212A}"]);
        let root = dir.path().join("root");
        delete_perm(&mut s, &root.join("\u{212A}")).unwrap();
        assert!(root.join("k").exists() && !root.join("\u{212A}").exists());
        // With only `k` left, the Kelvin spelling resolves to nothing.
        let err = delete_perm(&mut s, &root.join("\u{212A}")).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        let (dir, mut s, _fake) = lookalikes(&["k"]);
        let err = delete_perm(&mut s, &dir.path().join("root").join("\u{212A}")).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        assert!(dir.path().join("root").join("k").exists());
    }

    /// QA-139: fullwidth letters are not ASCII; only ASCII case differences are folded.
    #[test]
    fn fullwidth_letters_are_not_folded() {
        let (dir, mut s, _fake) = lookalikes(&["\u{ff41}.txt"]);
        let root = dir.path().join("root");
        let err = delete_perm(&mut s, &root.join("\u{ff21}.txt")).unwrap_err();
        assert_eq!(err.code(), "invalid_params", "{err}");
        assert!(root.join("\u{ff41}.txt").exists());
        delete_perm(&mut s, &root.join("\u{ff41}.txt")).unwrap();
        assert!(!root.join("\u{ff41}.txt").exists());
    }

    /// QA-139: ASCII case differences resolve on Windows (NTFS semantics) and not elsewhere.
    #[test]
    fn ascii_case_differences_follow_the_platform() {
        let (dir, mut s, _fake) = lookalikes(&["Readme.txt"]);
        let root = dir.path().join("root");
        let result = delete_perm(&mut s, &root.join("README.TXT"));
        if cfg!(windows) {
            assert_eq!(result.unwrap()["done"], true);
            assert!(!root.join("Readme.txt").exists());
        } else {
            assert_eq!(result.unwrap_err().code(), "invalid_params");
            assert!(root.join("Readme.txt").exists());
        }
    }

    fn synthetic_session(names: &[&str]) -> Session {
        let root = if cfg!(windows) { r"C:\scan" } else { "/scan" };
        let mut t = dirustat_model::Tree::new(root);
        for name in names {
            t.add_child(t.root(), name, NodeKind::File, 1).unwrap();
        }
        t.aggregate();
        let mut s = Session::new();
        s.replace_tree(t);
        s
    }

    /// QA-139: two siblings that match under the fold are refused as ambiguous, never first-match.
    #[test]
    fn siblings_matching_under_the_fold_are_ambiguous() {
        let s = synthetic_session(&["A", "a", "b"]);
        let at =
            |name: &str| node_for_path(&s, &format!("{}{name}", if cfg!(windows) { r"C:\scan\" } else { "/scan/" }));
        if cfg!(windows) {
            assert_eq!(at("a").unwrap_err().code(), "stale_tree");
            assert_eq!(at("A").unwrap_err().code(), "stale_tree");
        } else {
            assert_ne!(at("a").unwrap(), at("A").unwrap());
        }
        assert!(at("b").is_ok());
        assert_eq!(at("B").is_ok(), cfg!(windows));
    }

    /// QA-139: trailing dots and spaces are part of the requested name, not stripped.
    #[test]
    fn trailing_dots_and_spaces_do_not_resolve_to_the_plain_name() {
        let s = synthetic_session(&["dupe"]);
        let base = if cfg!(windows) { r"C:\scan\" } else { "/scan/" };
        assert!(node_for_path(&s, &format!("{base}dupe")).is_ok());
        for suffix in [".", " ", "..", ". ", "\u{a0}", "\u{200b}"] {
            let err = node_for_path(&s, &format!("{base}dupe{suffix}"));
            // `dupe..` ends in the parent marker only for the exact component "..".
            assert!(err.is_err(), "{suffix:?} must not resolve to `dupe`");
        }
    }

    /// QA-123: names Win32 treats as devices are not accepted as move destinations.
    #[test]
    fn device_names_are_refused_as_move_destinations() {
        let comps = |n: &str| vec![n.to_owned()];
        for bad in ["CON", "con", "COM1", "lpt9", "aux.txt", "LPT1.log", "nul.tar.gz", "CLOCK$", "CONIN$", "Com\u{b9}"]
        {
            assert!(destination_component_problem(&comps(bad), true).is_some(), "{bad}");
        }
        for ok in ["COM0", "COM10", "CONSOLE", "console.txt", "auxiliary", "nulled", "com", "lpt"] {
            assert_eq!(destination_component_problem(&comps(ok), true), None, "{ok}");
        }
        assert_eq!(destination_component_problem(&comps("CON"), false), None);
    }

    #[cfg(windows)]
    #[test]
    fn move_to_a_device_name_is_invalid_params() {
        let mut f = fixture();
        let src = f.path_of("keep.txt");
        for dest in ["CON", "COM1", "aux.txt"] {
            let p = json!({"path": src, "destination": f.path_of(dest), "confirm": true});
            assert_eq!(f.s.execute("file.move", &p).unwrap_err().code(), "invalid_params", "{dest}");
        }
        assert!(Path::new(&src).exists());
        assert!(f.fake.calls().is_empty());
    }
}
