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

use super::platform::platform_error;
use super::select::node_by_index;
use super::tree::{kind_name, page_limit};
use super::{CommandSpec, always, parse_params};
use crate::journal::{JournalEntry, now_secs};
use crate::protected::{ProtectedPaths, key_of};
use crate::{CommandError, Session, fsops, prune};
use dirustat_model::{NodeId, NodeKind, SizeMode};
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
            params_doc: "{\"id\"|\"path\": see file.delete, \"mode\"?: \"recycle\" (default) | \"permanent\"}. Read-only: \
                runs every check of the real command (protected path, stale tree, link rules) and returns the report \
                of what would be affected: {dry_run: true, mode, path, kind, count, files, dirs, links, size, \
                size_logical, size_allocated, paths: [string] (at most 200, the target first), truncated}. Needs no \
                confirm.",
            destructive: false,
            enabled: has_tree,
            run: run_preview,
        },
        CommandSpec {
            id: "file.delete",
            label: "Delete (recycle bin)",
            menu: "File/Delete",
            shortcut: Some("Delete"),
            params_doc: "{\"id\": u64} arena index of a node, or {\"path\": string} an absolute path that resolves to a \
                node of the current tree (exactly one), \"dry_run\"?: bool (default false; changes nothing and \
                returns the report). Moves the item to the recycle bin. Needs confirm: true even for dry_run (use \
                file.delete_preview to look first). Refuses protected paths (protected_path), a tree that no longer \
                matches the disk (stale_tree) and links/junctions (use file.delete_permanent, which removes the link \
                itself). Returns {dry_run, mode, path, kind, count, files, dirs, links, size, size_logical, \
                size_allocated, paths, truncated} plus, when done: {done, tree_updated, revision, ids_invalidated}; \
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
            params_doc: "{\"id\": u64} or {\"path\": string} (exactly one, as file.delete), \"dry_run\"?: bool, \
                \"i_understand_permanent\": bool must be true unless dry_run (otherwise confirmation_required). \
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
            params_doc: "{\"id\": u64} or {\"path\": string} (exactly one, as file.delete), \"destination\": string \
                absolute path the item gets (must not exist, its parent must be an existing directory, neither \
                may be protected, not inside the item itself), \"dry_run\"?: bool. Plain rename on one volume \
                (no overwrite, no copy fallback). Returns the delete report with mode \"move\" plus {destination, \
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Recycle,
    Permanent,
    Move,
}

impl Mode {
    fn name(self) -> &'static str {
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
    path: Option<String>,
    dry_run: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermanentParams {
    id: Option<u64>,
    path: Option<String>,
    dry_run: Option<bool>,
    i_understand_permanent: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewParams {
    id: Option<u64>,
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

/// Resolves `{id | path}` to a node of the current tree.
fn resolve_node(session: &Session, id: Option<u64>, path: Option<&str>) -> Result<NodeId, CommandError> {
    match (id, path) {
        (Some(index), None) => node_by_index(session.tree(), index)
            .ok_or_else(|| CommandError::InvalidParams(format!("no node with id {index}"))),
        (None, Some(path)) => node_for_path(session, path),
        _ => Err(CommandError::InvalidParams("give exactly one of `id` or `path`".to_owned())),
    }
}

/// Walks the tree from the root along the normalized `path`; case-insensitive on Windows.
fn node_for_path(session: &Session, path: &str) -> Result<NodeId, CommandError> {
    let windows = cfg!(windows);
    let tree = session.tree();
    let outside = || CommandError::InvalidParams("path does not resolve to a node of the scanned tree".to_owned());
    let key = key_of(path, windows).ok_or_else(|| CommandError::InvalidParams("path must be absolute".to_owned()))?;
    let root_path = session.node_path(tree.root()).ok_or_else(outside)?;
    let root_key = key_of(&root_path, windows).ok_or_else(outside)?;
    if !key.starts_with(&root_key) {
        return Err(outside());
    }
    let fold = |s: &str| if windows { s.to_lowercase() } else { s.to_owned() };
    let mut cur = tree.root();
    for comp in key.comps.get(root_key.comps.len()..).unwrap_or_default() {
        cur = tree.children(cur).find(|&c| tree.get(c).is_some_and(|n| fold(n.name()) == *comp)).ok_or_else(outside)?;
    }
    Ok(cur)
}

/// Iterative walk of the subtree: counts, saturating sums and a capped path listing.
fn build_report(session: &Session, node: NodeId, path: &str) -> Report {
    let tree = session.tree();
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let mut report = Report::default();
    let mut stack: Vec<(NodeId, Option<String>)> = vec![(node, Some(path.to_owned()))];
    while let Some((id, node_path)) = stack.pop() {
        let Some(n) = tree.get(id) else { continue };
        match n.kind() {
            NodeKind::Dir => report.dirs = report.dirs.saturating_add(1),
            NodeKind::File => report.files = report.files.saturating_add(1),
            NodeKind::Link => report.links = report.links.saturating_add(1),
        }
        report.logical = report.logical.saturating_add(n.own_size(SizeMode::Logical));
        report.allocated = report.allocated.saturating_add(n.own_size(SizeMode::Allocated));
        let listed = match node_path {
            Some(p) if report.paths.len() < MAX_LISTED => {
                report.paths.push(p.clone());
                Some(p)
            }
            Some(_) => {
                report.truncated = true;
                None
            }
            None => {
                report.truncated = true;
                None
            }
        };
        if n.kind() == NodeKind::Dir {
            for child in tree.children(id) {
                let child_path = listed.as_ref().and_then(|p| {
                    let name = tree.get(child)?.name();
                    let mut s = p.clone();
                    if !s.ends_with(['/', '\\']) {
                        s.push(sep);
                    }
                    s.push_str(name);
                    Some(s)
                });
                stack.push((child, child_path));
            }
        }
    }
    report
}

/// The shared pre-flight of every destructive command; touches nothing.
fn plan(session: &Session, id: Option<u64>, path: Option<&str>, mode: Mode) -> Result<Plan, CommandError> {
    let node = resolve_node(session, id, path)?;
    let tree = session.tree();
    let target = tree.get(node).ok_or_else(|| CommandError::InvalidParams("unknown node".to_owned()))?;
    let node_path =
        session.node_path(node).ok_or_else(|| CommandError::InvalidParams("node has no path".to_owned()))?;
    let scan_root = session
        .node_path(tree.root())
        .ok_or_else(|| CommandError::InvalidParams("the tree has no root path".to_owned()))?;
    let protected = ProtectedPaths::from_env();
    protected.check(&node_path, &scan_root).map_err(CommandError::ProtectedPath)?;
    fsops::verify_location(&node_path, &scan_root, &protected, cfg!(windows))?;
    fsops::verify_matches(Path::new(&node_path), target.kind(), target.data())?;
    if mode == Mode::Recycle && target.kind() == NodeKind::Link {
        return Err(CommandError::InvalidParams(
            "links and junctions are not sent to the recycle bin; use file.delete_permanent, which removes the link \
             itself and never its target"
                .to_owned(),
        ));
    }
    let report = build_report(session, node, &node_path);
    Ok(Plan { node, path: node_path, scan_root, kind: target.kind(), report })
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
    let plan = plan(session, p.id, p.path.as_deref(), mode)?;
    Ok(report_json(session, &plan, mode, true))
}

fn run_delete(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: DeleteParams = parse_params(params)?;
    execute(session, "file.delete", Mode::Recycle, p.id, p.path.as_deref(), p.dry_run.unwrap_or(false), None)
}

fn run_delete_permanent(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: PermanentParams = parse_params(params)?;
    let dry_run = p.dry_run.unwrap_or(false);
    if !dry_run && p.i_understand_permanent != Some(true) {
        return Err(CommandError::ConfirmationRequired("file.delete_permanent: i_understand_permanent".to_owned()));
    }
    execute(session, "file.delete_permanent", Mode::Permanent, p.id, p.path.as_deref(), dry_run, None)
}

fn run_move(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: MoveParams = parse_params(params)?;
    execute(session, "file.move", Mode::Move, p.id, p.path.as_deref(), p.dry_run.unwrap_or(false), Some(&p.destination))
}

/// Validates the destination of a move; returns it as given. Nothing is touched.
fn check_destination(plan: &Plan, destination: &str) -> Result<(), CommandError> {
    let windows = cfg!(windows);
    let bad = |why: &str| CommandError::InvalidParams(format!("destination: {why}"));
    let dest_key = key_of(destination, windows).ok_or_else(|| bad("must be an absolute path"))?;
    let src_key = key_of(&plan.path, windows).ok_or_else(|| bad("the source has no absolute path"))?;
    if dest_key.starts_with(&src_key) {
        return Err(bad("cannot move an item into itself"));
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

fn execute(
    session: &mut Session,
    op: &str,
    mode: Mode,
    id: Option<u64>,
    path: Option<&str>,
    dry_run: bool,
    destination: Option<&str>,
) -> Result<Value, CommandError> {
    let plan = plan(session, id, path, mode)?;
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
        return Ok(out);
    }
    let target = Path::new(&plan.path);
    let result = match mode {
        Mode::Recycle => session.platform().move_to_recycle_bin(target).map_err(platform_error),
        Mode::Permanent => fsops::remove_permanently(target)
            .map_err(|e| CommandError::Platform(format!("permanent deletion failed: {e}"))),
        Mode::Move => destination.map_or(Ok(()), |dest| {
            std::fs::rename(target, dest).map_err(|e| CommandError::Platform(format!("move failed: {e}")))
        }),
    };
    let size = plan.report.size(session.map_size_mode());
    session.journal.record(JournalEntry {
        time: now_secs(),
        op: op.to_owned(),
        path: plan.path.clone(),
        destination: destination.map(str::to_owned),
        size,
        mode: mode.name().to_owned(),
        ok: result.is_ok(),
        error: result.as_ref().err().map(ToString::to_string),
    });
    result?;
    let updated = session.forget_node(plan.node);
    if let Some(map) = out.as_object_mut() {
        map.insert("done".to_owned(), json!(true));
        map.insert("tree_updated".to_owned(), json!(updated));
        map.insert("revision".to_owned(), json!(session.revision()));
        map.insert("ids_invalidated".to_owned(), json!(updated));
    }
    Ok(out)
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
        let preview = f.s.execute("file.delete_preview", &json!({"id": f.id_of("sub")})).unwrap();
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
            json!({"id": 0}),
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
        assert_eq!(f.code("file.move", json!({"id": 0, "destination": dest, "confirm": true})), "protected_path");
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
            json!({"id": 0}),
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
}
