//! `cleanup.*` commands (G-017): user-defined cleanups on top of the `file.*` machinery.
//!
//! * `cleanup.list`, `cleanup.save`, `cleanup.remove`: definitions in the settings (no disk access).
//! * `cleanup.preview` (read-only): resolves the selector against the current tree and reports
//!   every node with its protected/stale status.
//! * `cleanup.run` (destructive): re-resolves the selector and applies the action node by node
//!   through the same checks and the same `perform` as `file.delete` / `file.delete_permanent`:
//!   protected-path refusal, stale-tree re-check and a journal entry per node.
//!
//! Nothing here runs by itself: not even the built-in definitions. All ids are resolved to paths
//! before the first action; the tree is pruned once at the end (one revision bump), so arena ids
//! stay valid while the run is in progress.

use super::file::{Mode, node_for_path, perform, preflight};
use super::tree::kind_name;
use super::{CommandSpec, always, no_params, parse_params};
use crate::cleanup::{
    CleanupAction, CleanupDef, Compiled, DEF_KEYS, MAX_CLEANUPS, MAX_NODES_PER_RUN, SELECTOR_KEYS, builtins,
    dir_is_empty_on_disk, is_builtin, select,
};
use crate::journal::now_secs;
use crate::protected::ProtectedPaths;
use crate::{CommandError, Session, prune};
use dirustat_model::{NodeId, NodeKind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// Consecutive platform failures after which a run stops (a service that is down fails every node).
const MAX_CONSECUTIVE_PLATFORM_FAILURES: u32 = 5;
/// Journal op name of cleanup actions.
const OP: &str = "cleanup.run";

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "cleanup.list",
            label: "List cleanups",
            menu: "",
            shortcut: None,
            params_doc: "none. Read-only. Returns {max, cleanups: [{id, name, action, selector, builtin}]}: the 3 \
                built-ins first (never run automatically, cannot be edited), then the user's. action: recycle | \
                permanent | empty_dir_contents | reveal. selector: {names: [wildcard], extensions: [string], \
                min_size, max_size, kind: file|dir|any, only_under: absolute path, older_than_days, empty: bool}.",
            destructive: false,
            enabled: always,
            run: run_list,
        },
        CommandSpec {
            id: "cleanup.save",
            label: "Save cleanup",
            menu: "",
            shortcut: None,
            params_doc: "{\"definition\": {id: slug [a-z0-9_-] <= 64, name: 1..=128 chars, action, selector?}}. \
                Adds or replaces a user cleanup in the settings; touches no file. Limits: at most 64 cleanups, 64 \
                name patterns (<= 256 chars, no separators), 64 extensions, only_under absolute without . or .. \
                components, older_than_days 1..=36500, at least one of names / extensions / min_size / max_size / \
                older_than_days / empty. `empty` and empty_dir_contents need kind \"dir\". Built-in ids and unknown \
                fields are invalid_params. Returns {saved, replaced, cleanups}.",
            destructive: false,
            enabled: always,
            run: run_save,
        },
        CommandSpec {
            id: "cleanup.remove",
            label: "Remove cleanup",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": string}. Removes a user cleanup from the settings; built-ins cannot be removed. \
                Returns {removed, cleanups}.",
            destructive: false,
            enabled: always,
            run: run_remove,
        },
        CommandSpec {
            id: "cleanup.preview",
            label: "Preview cleanup",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": string}. Read-only, never touches the disk beyond stat calls. Resolves the selector \
                against the current tree (never the scan root, no node inside another selected node) and returns \
                {id, name, action, revision, count, size, truncated, items: [{id, name, kind, path, size, status, \
                error?}]} largest first, at most 10000 items. status: ok | protected | stale | invalid. For \
                empty_dir_contents the items are the entries inside each selected folder.",
            destructive: false,
            enabled: has_tree,
            run: run_preview,
        },
        CommandSpec {
            id: "cleanup.run",
            label: "Run cleanup",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": string, \"dry_run\"?: bool (default false; changes nothing, same report), \
                \"i_understand_permanent\"?: bool (must be true for a permanent action unless dry_run)}. Needs \
                confirm: true. Re-resolves the selector and applies the action node by node with the same checks \
                and journal as file.delete / file.delete_permanent (recycle bin by default, protected paths and \
                stale nodes are reported, not touched). At most 10000 nodes per run (truncated). Best effort: \
                returns {id, action, dry_run, count, size, truncated, deleted, failed, not_attempted, stopped, \
                results: [{id, path, ok, status, error?}], done, tree_updated, rescan_recommended, revision, \
                ids_invalidated}. A run stops (stopped: reason) when the platform has no recycle bin or fails 5 \
                times in a row. After a run node ids are invalid. The reveal action shows only the largest match.",
            destructive: true,
            enabled: has_tree,
            run: run_run,
        },
    ]
}

fn has_tree(session: &Session) -> bool {
    session.tree().len() > 1 && !session.is_scanning()
}

fn invalid(what: impl Into<String>) -> CommandError {
    CommandError::InvalidParams(what.into())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveParams {
    definition: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunParams {
    id: String,
    dry_run: Option<bool>,
    i_understand_permanent: Option<bool>,
}

fn cleanups_json(session: &Session) -> Vec<Value> {
    let entry = |d: &CleanupDef, builtin: bool| json!({ "id": d.id, "name": d.name, "action": d.action, "selector": d.selector, "builtin": builtin });
    builtins()
        .iter()
        .map(|d| entry(d, true))
        .chain(session.settings().cleanups.iter().map(|d| entry(d, false)))
        .collect()
}

fn run_list(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    Ok(json!({ "max": MAX_CLEANUPS, "cleanups": cleanups_json(session) }))
}

/// Refuses keys outside `allowed` in a JSON object.
fn check_keys(what: &str, value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    let Value::Object(map) = value else { return Err(invalid(format!("{what} must be an object"))) };
    match map.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(invalid(format!("unknown field in {what}: {}", k.chars().take(100).collect::<String>()))),
        None => Ok(()),
    }
}

fn run_save(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: SaveParams = parse_params(params)?;
    check_keys("definition", &p.definition, DEF_KEYS)?;
    if let Some(sel) = p.definition.get("selector") {
        check_keys("selector", sel, SELECTOR_KEYS)?;
    }
    let mut def: CleanupDef =
        serde_json::from_value(p.definition).map_err(|e| invalid(format!("bad definition: {e}")))?;
    def.name = def.name.trim().to_owned();
    def.validate()?;
    if is_builtin(&def.id) {
        return Err(invalid("built-in cleanups cannot be replaced; pick another id"));
    }
    let mut next = session.settings().clone();
    let id = def.id.clone();
    let replaced = match next.cleanups.iter_mut().find(|d| d.id == id) {
        Some(slot) => {
            *slot = def;
            true
        }
        None => {
            if next.cleanups.len() >= MAX_CLEANUPS {
                return Err(invalid(format!("at most {MAX_CLEANUPS} cleanups")));
            }
            next.cleanups.push(def);
            false
        }
    };
    session.apply_settings(next);
    Ok(json!({ "saved": id, "replaced": replaced, "cleanups": cleanups_json(session) }))
}

fn run_remove(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: IdParams = parse_params(params)?;
    if is_builtin(&p.id) {
        return Err(invalid("built-in cleanups cannot be removed"));
    }
    let mut next = session.settings().clone();
    let before = next.cleanups.len();
    next.cleanups.retain(|d| d.id != p.id);
    if next.cleanups.len() == before {
        return Err(invalid("no such cleanup"));
    }
    session.apply_settings(next);
    Ok(json!({ "removed": p.id, "cleanups": cleanups_json(session) }))
}

fn find_def(session: &Session, id: &str) -> Result<CleanupDef, CommandError> {
    builtins()
        .into_iter()
        .chain(session.settings().cleanups.iter().cloned())
        .find(|d| d.id == id)
        .ok_or_else(|| invalid("no such cleanup"))
}

/// One node a cleanup would act on.
struct Target {
    id: NodeId,
    path: String,
    name: String,
    kind: NodeKind,
    size: u64,
    /// For empty_dir_contents: why the containing folder itself is refused, decided up front
    /// (acting on siblings changes the folder's modification time).
    container_error: Option<CommandError>,
}

struct Resolved {
    targets: Vec<Target>,
    truncated: bool,
}

fn mode_of(action: CleanupAction) -> Option<Mode> {
    match action {
        CleanupAction::Recycle | CleanupAction::EmptyDirContents => Some(Mode::Recycle),
        CleanupAction::Permanent => Some(Mode::Permanent),
        CleanupAction::Reveal => None,
    }
}

/// Resolves the selector of `def` against the current tree. Reads the tree and, for
/// empty_dir_contents, stats the containing folders; changes nothing.
fn resolve(session: &Session, def: &CleanupDef, protected: &ProtectedPaths) -> Result<Resolved, CommandError> {
    let tree = session.tree();
    let now = i64::try_from(now_secs()).unwrap_or(i64::MAX);
    let compiled = Compiled::new(def, now)?;
    let start = match &def.selector.only_under {
        Some(under) => {
            let node = node_for_path(session, under)?;
            if tree.get(node).is_none_or(|n| n.kind() != NodeKind::Dir) {
                return Err(invalid("only_under must name a folder of the scanned tree"));
            }
            node
        }
        None => tree.root(),
    };
    let mode = session.map_size_mode();
    let selection = select(tree, start, &compiled, mode, MAX_NODES_PER_RUN);
    let mut truncated = selection.truncated;
    let mut targets = Vec::new();
    let target_of = |id: NodeId, container_error: Option<CommandError>| {
        let node = tree.get(id)?;
        Some(Target {
            id,
            path: session.node_path(id)?,
            name: node.name().to_owned(),
            kind: node.kind(),
            size: crate::search::node_size(tree, id, mode),
            container_error,
        })
    };
    for id in selection.nodes {
        if def.action != CleanupAction::EmptyDirContents {
            targets.extend(target_of(id, None));
            continue;
        }
        let refusal = preflight(session, id, protected, Mode::Recycle).err();
        for child in tree.children(id) {
            if targets.len() >= MAX_NODES_PER_RUN {
                truncated = true;
                break;
            }
            targets.extend(target_of(child, refusal.clone()));
        }
    }
    Ok(Resolved { targets, truncated })
}

/// Status word and message of a refusal.
fn status_of(err: &CommandError) -> (&'static str, String) {
    let word = match err {
        CommandError::ProtectedPath(_) => "protected",
        CommandError::StaleTree(_) => "stale",
        _ => "invalid",
    };
    (word, err.to_string())
}

/// The checks of `file.delete` for one target (plus the emptiness guard for `empty` selectors).
/// Returns the path to act on. Reads the disk, changes nothing.
fn check(session: &Session, protected: &ProtectedPaths, def: &CleanupDef, t: &Target) -> Result<String, CommandError> {
    if let Some(e) = &t.container_error {
        return Err(e.clone());
    }
    let Some(mode) = mode_of(def.action) else { return Ok(t.path.clone()) };
    let path = preflight(session, t.id, protected, mode)?;
    if def.selector.empty && !dir_is_empty_on_disk(Path::new(&path)) {
        return Err(CommandError::StaleTree("the folder is no longer empty".to_owned()));
    }
    Ok(path)
}

fn total_size(targets: &[Target]) -> u64 {
    targets.iter().fold(0u64, |acc, t| acc.saturating_add(t.size))
}

fn run_preview(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: IdParams = parse_params(params)?;
    let def = find_def(session, &p.id)?;
    let protected = ProtectedPaths::from_env();
    let resolved = resolve(session, &def, &protected)?;
    let items: Vec<Value> = resolved
        .targets
        .iter()
        .map(|t| {
            let mut item = json!({
                "id": t.id.index(), "name": t.name, "kind": kind_name(t.kind), "path": t.path, "size": t.size,
            });
            let (status, error) = match check(session, &protected, &def, t) {
                Ok(_) => ("ok", None),
                Err(e) => {
                    let (word, msg) = status_of(&e);
                    (word, Some(msg))
                }
            };
            if let Some(map) = item.as_object_mut() {
                map.insert("status".to_owned(), json!(status));
                if let Some(msg) = error {
                    map.insert("error".to_owned(), json!(msg));
                }
            }
            item
        })
        .collect();
    Ok(json!({
        "id": def.id,
        "name": def.name,
        "action": def.action,
        "revision": session.revision(),
        "count": resolved.targets.len(),
        "size": total_size(&resolved.targets),
        "truncated": resolved.truncated,
        "items": items,
    }))
}

fn result_entry(t: &Target, ok: bool, status: &str, error: Option<String>) -> Value {
    let mut v = json!({ "id": t.id.index(), "path": t.path, "ok": ok, "status": status });
    if let (Some(msg), Some(map)) = (error, v.as_object_mut()) {
        map.insert("error".to_owned(), json!(msg));
    }
    v
}

fn run_run(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: RunParams = parse_params(params)?;
    let def = find_def(session, &p.id)?;
    let dry_run = p.dry_run.unwrap_or(false);
    if def.action == CleanupAction::Permanent && !dry_run && p.i_understand_permanent != Some(true) {
        return Err(CommandError::ConfirmationRequired("cleanup.run: i_understand_permanent".to_owned()));
    }
    let protected = ProtectedPaths::from_env();
    let mut resolved = resolve(session, &def, &protected)?;
    if def.action == CleanupAction::Reveal {
        resolved.targets.truncate(1);
    }
    let mode = mode_of(def.action);
    let mut results: Vec<Value> = Vec::new();
    let mut removed: Vec<NodeId> = Vec::new();
    let (mut deleted, mut failed, mut deleted_size) = (0u64, 0u64, 0u64);
    let mut stopped: Option<String> = None;
    let mut consecutive = 0u32;
    for t in &resolved.targets {
        if stopped.is_some() {
            break;
        }
        let path = match check(session, &protected, &def, t) {
            Ok(path) => path,
            Err(e) => {
                let (word, msg) = status_of(&e);
                failed = failed.saturating_add(1);
                results.push(result_entry(t, false, word, Some(msg)));
                continue;
            }
        };
        if dry_run {
            results.push(result_entry(t, true, "ok", None));
            continue;
        }
        let outcome = match mode {
            Some(mode) => perform(session, OP, mode, &path, None, t.size),
            None => {
                session.platform().reveal_in_file_manager(Path::new(&path)).map_err(super::platform::platform_error)
            }
        };
        match outcome {
            Ok(()) => {
                consecutive = 0;
                deleted = deleted.saturating_add(1);
                deleted_size = deleted_size.saturating_add(t.size);
                if mode.is_some() {
                    removed.push(t.id);
                }
                results.push(result_entry(t, true, "done", None));
            }
            Err(e) => {
                failed = failed.saturating_add(1);
                results.push(result_entry(t, false, "failed", Some(e.to_string())));
                match e {
                    CommandError::Unsupported(_) => stopped = Some(e.to_string()),
                    CommandError::Platform(_) => {
                        consecutive = consecutive.saturating_add(1);
                        if consecutive >= MAX_CONSECUTIVE_PLATFORM_FAILURES {
                            stopped = Some(format!("{consecutive} platform failures in a row; last: {e}"));
                        }
                    }
                    _ => consecutive = 0,
                }
            }
        }
    }
    let not_attempted = resolved.targets.len().saturating_sub(results.len());
    let updated = !removed.is_empty() && session.forget_nodes(&removed);
    Ok(json!({
        "id": def.id,
        "action": def.action,
        "dry_run": dry_run,
        "count": resolved.targets.len(),
        "size": total_size(&resolved.targets),
        "truncated": resolved.truncated,
        "deleted": if dry_run { 0 } else { deleted },
        "deleted_size": deleted_size,
        "failed": failed,
        "not_attempted": not_attempted,
        "stopped": stopped,
        "results": results,
        "done": !dry_run && stopped.is_none(),
        "tree_updated": updated,
        "rescan_recommended": !removed.is_empty() && !updated,
        "revision": session.revision(),
        "ids_invalidated": updated,
    }))
}

impl Session {
    /// Cuts every node of `nodes` (and their subtrees) out of the session tree in one pass and
    /// bumps the revision once. `false` leaves the tree as it was.
    fn forget_nodes(&mut self, nodes: &[NodeId]) -> bool {
        let Some(pruned) = prune::without_subtrees(self.tree(), nodes) else { return false };
        self.tree = Arc::new(pruned);
        self.tree_replaced();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_platform::{FakePlatform, PlatformCall, PlatformError};
    use std::path::PathBuf;

    struct Fixture {
        dir: tempfile::TempDir,
        s: Session,
        fake: Arc<FakePlatform>,
    }

    /// `root/{a.tmp, keep.txt, sub/{b.temp, keep2.txt, deep/c.tmp}, target/{out.bin}, hollow/inner/}`.
    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("sub").join("deep")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join("hollow").join("inner")).unwrap();
        std::fs::write(root.join("a.tmp"), b"aaaa").unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();
        std::fs::write(root.join("sub").join("b.temp"), b"bb").unwrap();
        std::fs::write(root.join("sub").join("keep2.txt"), b"k2").unwrap();
        std::fs::write(root.join("sub").join("deep").join("c.tmp"), b"c").unwrap();
        std::fs::write(root.join("target").join("out.bin"), vec![1u8; 100]).unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(&root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        Fixture { dir, s, fake }
    }

    impl Fixture {
        fn root(&self) -> PathBuf {
            self.dir.path().join("root")
        }
        fn exists(&self, rel: &str) -> bool {
            rel.split('/').fold(self.root(), |p, c| p.join(c)).exists()
        }
        fn save(&mut self, def: Value) -> Result<Value, CommandError> {
            self.s.execute("cleanup.save", &json!({ "definition": def }))
        }
        fn run(&mut self, id: &str, extra: Value) -> Result<Value, CommandError> {
            let mut p = json!({"id": id, "confirm": true});
            if let (Some(m), Some(e)) = (p.as_object_mut(), extra.as_object()) {
                m.extend(e.clone());
            }
            self.s.execute("cleanup.run", &p)
        }
        fn paths(items: &Value) -> Vec<String> {
            items
                .as_array()
                .unwrap()
                .iter()
                .map(|i| {
                    let p = i["path"].as_str().unwrap();
                    p.rsplit(['/', '\\']).next().unwrap().to_owned()
                })
                .collect()
        }
    }

    fn tmp_def(id: &str) -> Value {
        json!({"id": id, "name": "Temp", "action": "recycle", "selector": {"names": ["*.tmp", "*.temp"], "kind": "file"}})
    }

    #[test]
    fn list_has_builtins_and_save_remove_round_trip() {
        let mut f = fixture();
        let list = f.s.execute("cleanup.list", &json!({})).unwrap();
        let ids: Vec<_> = list["cleanups"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["temporary-files", "empty-folders", "build-output-folders"]);
        assert_eq!(list["cleanups"][0]["builtin"], true);
        let out = f.save(tmp_def("mine")).unwrap();
        assert_eq!((out["saved"].as_str(), out["replaced"].as_bool()), (Some("mine"), Some(false)));
        assert_eq!(f.save(tmp_def("mine")).unwrap()["replaced"], true);
        assert_eq!(f.s.settings().cleanups.len(), 1);
        // Survives a settings file round trip.
        let file = f.dir.path().join("settings.json");
        f.s.settings().save_to(&file).unwrap();
        assert_eq!(crate::Settings::load_from(&file).settings.cleanups, f.s.settings().cleanups);
        let out = f.s.execute("cleanup.remove", &json!({"id": "mine"})).unwrap();
        assert_eq!(out["cleanups"].as_array().unwrap().len(), 3);
        for bad in [json!({"id": "mine"}), json!({"id": "temporary-files"})] {
            assert_eq!(f.s.execute("cleanup.remove", &bad).unwrap_err().code(), "invalid_params");
        }
    }

    #[test]
    fn hostile_saves_are_refused_and_change_nothing() {
        let mut f = fixture();
        let long = "x".repeat(10_000);
        let many: Vec<String> = (0..1000).map(|i| format!("p{i}*")).collect();
        let bad = [
            json!({"id": "", "name": "n", "action": "recycle", "selector": {"names": ["a"]}}),
            json!({"id": "ok", "name": "", "action": "recycle", "selector": {"names": ["a"]}}),
            json!({"id": "ok", "name": "a\u{0}b", "action": "recycle", "selector": {"names": ["a"]}}),
            json!({"id": "ok", "name": long, "action": "recycle", "selector": {"names": ["a"]}}),
            json!({"id": "ok", "name": "n", "action": "explode", "selector": {"names": ["a"]}}),
            json!({"id": "ok", "name": "n", "action": "recycle"}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"names": many}}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"names": ["a\u{0}"]}}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"names": ["a"], "bogus": 1}}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"names": ["a"], "only_under": "/x/../../etc"}}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"names": ["a"], "only_under": "rel"}}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"min_size": -1}}),
            json!({"id": "empty-folders", "name": "n", "action": "recycle", "selector": {"names": ["a"]}}),
            json!({"id": "ok", "name": "n", "action": "recycle", "selector": {"names": ["a"]}, "extra": 1}),
            json!("string"),
            json!(null),
        ];
        for def in bad {
            assert_eq!(f.save(def.clone()).unwrap_err().code(), "invalid_params", "{def}");
        }
        assert!(f.s.execute("cleanup.save", &json!({})).is_err());
        assert!(f.s.settings().cleanups.is_empty());
        // Duplicate ids replace, never duplicate; the cap holds.
        for i in 0..crate::cleanup::MAX_CLEANUPS {
            f.save(tmp_def(&format!("c{i}"))).unwrap();
        }
        assert_eq!(f.save(tmp_def("overflow")).unwrap_err().code(), "invalid_params");
        assert_eq!(f.save(tmp_def("c0")).unwrap()["replaced"], true);
        assert_eq!(f.s.settings().cleanups.len(), crate::cleanup::MAX_CLEANUPS);
    }

    #[test]
    fn preview_changes_nothing_and_dedupes() {
        let mut f = fixture();
        let out = f.s.execute("cleanup.preview", &json!({"id": "temporary-files"})).unwrap();
        let mut names = Fixture::paths(&out["items"]);
        names.sort();
        assert_eq!(names, ["a.tmp", "b.temp", "c.tmp"]);
        assert_eq!(out["count"], 3);
        assert_eq!(out["size"], 7);
        assert_eq!(out["truncated"], false);
        assert!(out["items"].as_array().unwrap().iter().all(|i| i["status"] == "ok"));
        // Largest first.
        assert_eq!(out["items"][0]["size"], 4);
        let out = f.s.execute("cleanup.preview", &json!({"id": "empty-folders"})).unwrap();
        assert_eq!(Fixture::paths(&out["items"]), ["hollow"]);
        let out = f.s.execute("cleanup.preview", &json!({"id": "build-output-folders"})).unwrap();
        assert_eq!(Fixture::paths(&out["items"]), ["target"]);
        assert!(f.exists("a.tmp") && f.exists("hollow/inner") && f.exists("target/out.bin"));
        assert!(f.fake.calls().is_empty());
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["total"], 0);
        assert_eq!(f.s.execute("cleanup.preview", &json!({"id": "nope"})).unwrap_err().code(), "invalid_params");
    }

    #[test]
    fn run_refuses_without_confirm_and_dry_run_changes_nothing() {
        let mut f = fixture();
        for p in [json!({"id": "temporary-files"}), json!({"id": "temporary-files", "confirm": false})] {
            assert_eq!(f.s.execute("cleanup.run", &p).unwrap_err().code(), "confirmation_required");
        }
        let rev = f.s.revision();
        let out = f.run("temporary-files", json!({"dry_run": true})).unwrap();
        assert_eq!(
            (out["dry_run"].as_bool(), out["count"].as_u64(), out["deleted"].as_u64()),
            (Some(true), Some(3), Some(0))
        );
        assert_eq!(out["results"].as_array().unwrap().len(), 3);
        assert_eq!(out["tree_updated"], false);
        assert_eq!(f.s.revision(), rev);
        assert!(f.exists("a.tmp") && f.exists("sub/b.temp"));
        assert!(f.fake.calls().is_empty());
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["total"], 0);
    }

    #[test]
    fn run_recycles_each_node_journals_and_prunes_once() {
        let mut f = fixture();
        let rev = f.s.revision();
        let out = f.run("temporary-files", json!({})).unwrap();
        assert_eq!(
            (out["deleted"].as_u64(), out["failed"].as_u64(), out["done"].as_bool()),
            (Some(3), Some(0), Some(true))
        );
        assert_eq!(out["tree_updated"], true);
        assert_eq!(out["ids_invalidated"], true);
        assert_eq!(f.s.revision(), rev + 1);
        assert!(!f.exists("a.tmp") && !f.exists("sub/b.temp") && !f.exists("sub/deep/c.tmp"));
        assert!(f.exists("keep.txt") && f.exists("sub/keep2.txt"));
        assert_eq!(f.fake.binned().len(), 3);
        assert!(f.fake.calls().iter().all(|c| matches!(c, PlatformCall::MoveToRecycleBin(_))));
        let journal = f.s.execute("file.journal", &json!({})).unwrap();
        assert_eq!(journal["total"], 3);
        assert_eq!(journal["entries"][0]["op"], "cleanup.run");
        assert_eq!(journal["entries"][0]["mode"], "recycle");
        // 11 nodes before (root + 10), 3 files gone.
        let out = f.s.execute("cleanup.preview", &json!({"id": "temporary-files"})).unwrap();
        assert_eq!(out["count"], 0);
    }

    #[test]
    fn ancestors_are_deleted_and_descendants_are_not_selected_separately() {
        let mut f = fixture();
        f.save(json!({"id": "both", "name": "Both", "action": "recycle",
            "selector": {"names": ["sub", "*.tmp", "*.temp", "deep"]}}))
            .unwrap();
        let out = f.s.execute("cleanup.preview", &json!({"id": "both"})).unwrap();
        let mut names = Fixture::paths(&out["items"]);
        names.sort();
        assert_eq!(names, ["a.tmp", "sub"]);
        let out = f.run("both", json!({})).unwrap();
        assert_eq!(out["deleted"], 2);
        assert_eq!(f.fake.binned().len(), 2);
        assert!(!f.exists("sub") && f.exists("keep.txt"));
    }

    #[test]
    fn stale_nodes_are_reported_and_not_deleted() {
        let mut f = fixture();
        std::fs::write(f.root().join("a.tmp"), b"changed content").unwrap();
        std::fs::remove_file(f.root().join("sub").join("b.temp")).unwrap();
        let prev = f.s.execute("cleanup.preview", &json!({"id": "temporary-files"})).unwrap();
        let statuses: Vec<_> =
            prev["items"].as_array().unwrap().iter().map(|i| i["status"].as_str().unwrap()).collect();
        assert_eq!(statuses.iter().filter(|s| **s == "stale").count(), 2);
        let out = f.run("temporary-files", json!({})).unwrap();
        assert_eq!((out["deleted"].as_u64(), out["failed"].as_u64()), (Some(1), Some(2)));
        assert!(out["results"].as_array().unwrap().iter().any(|r| r["status"] == "stale" && r["ok"] == false));
        assert!(f.exists("a.tmp"));
        assert_eq!(f.fake.binned().len(), 1);
    }

    #[test]
    fn protected_nodes_are_reported_not_deleted() {
        let mut f = fixture();
        // The check refuses the scan root; the resolver never selects it in the first place.
        let protected = ProtectedPaths::from_env();
        let def = find_def(&f.s, "temporary-files").unwrap();
        let resolved = resolve(&f.s, &def, &protected).unwrap();
        let root_target = Target {
            id: f.s.tree().root(),
            path: f.root().to_string_lossy().into_owned(),
            name: "root".into(),
            kind: NodeKind::Dir,
            size: 0,
            container_error: None,
        };
        assert_eq!(check(&f.s, &protected, &def, &root_target).unwrap_err().code(), "protected_path");
        assert!(resolved.targets.iter().all(|t| t.id != f.s.tree().root()));
        // A selector matching everything never selects the scan root.
        f.save(json!({"id": "all", "name": "All", "action": "permanent", "selector": {"names": ["*"], "kind": "dir"}}))
            .unwrap();
        let out = f.s.execute("cleanup.preview", &json!({"id": "all"})).unwrap();
        assert!(out["items"].as_array().unwrap().iter().all(|i| i["id"] != 0));
        let dry = f.run("all", json!({"dry_run": true})).unwrap();
        assert_eq!(dry["count"], 3);
        assert!(f.exists("sub") && f.exists("hollow"));
    }

    #[test]
    fn container_refusal_blocks_its_children() {
        let f = fixture();
        let protected = ProtectedPaths::from_env();
        let def = CleanupDef {
            id: "x".into(),
            name: "x".into(),
            action: CleanupAction::EmptyDirContents,
            selector: crate::cleanup::Selector {
                names: vec!["x".into()],
                kind: crate::cleanup::SelectorKind::Dir,
                ..Default::default()
            },
        };
        let t = Target {
            id: f.s.tree().root(),
            path: String::new(),
            name: String::new(),
            kind: NodeKind::File,
            size: 0,
            container_error: Some(CommandError::ProtectedPath("x".into())),
        };
        assert_eq!(check(&f.s, &protected, &def, &t).unwrap_err().code(), "protected_path");
    }

    #[test]
    fn permanent_needs_the_extra_flag() {
        let mut f = fixture();
        f.save(json!({"id": "perm", "name": "Perm", "action": "permanent", "selector": {"names": ["*.tmp"], "kind": "file"}}))
            .unwrap();
        for extra in [json!({}), json!({"i_understand_permanent": false})] {
            assert_eq!(f.run("perm", extra).unwrap_err().code(), "confirmation_required");
        }
        assert_eq!(f.run("perm", json!({"i_understand_permanent": "yes"})).unwrap_err().code(), "invalid_params");
        assert!(f.exists("a.tmp"));
        assert!(f.run("perm", json!({"dry_run": true})).is_ok());
        let out = f.run("perm", json!({"i_understand_permanent": true})).unwrap();
        assert_eq!(out["deleted"], 2);
        assert!(!f.exists("a.tmp") && !f.exists("sub/deep/c.tmp"));
        assert!(f.fake.calls().is_empty(), "permanent deletion bypasses the recycle bin");
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["entries"][0]["mode"], "permanent");
    }

    #[test]
    fn empty_folders_and_empty_dir_contents() {
        let mut f = fixture();
        let out = f.run("empty-folders", json!({})).unwrap();
        assert_eq!(out["deleted"], 1);
        assert!(!f.exists("hollow"));
        // A folder that gained a file after the scan is no longer empty.
        std::fs::create_dir_all(f.root().join("late")).unwrap();
        let mut s2 = Session::new();
        let handle = dirustat_scan::start(&f.root(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s2.finish_scan(handle), Ok(true));
        s2.set_platform(f.fake.clone());
        std::fs::write(f.root().join("late").join("x.txt"), b"x").unwrap();
        let out = s2.execute("cleanup.run", &json!({"id": "empty-folders", "confirm": true})).unwrap();
        assert_eq!(out["deleted"], 0);
        assert!(f.exists("late/x.txt"));
        // empty_dir_contents keeps the folder and recycles what is inside.
        let mut f = fixture();
        f.save(json!({"id": "clear", "name": "Clear", "action": "empty_dir_contents",
            "selector": {"names": ["target"], "kind": "dir"}}))
            .unwrap();
        let prev = f.s.execute("cleanup.preview", &json!({"id": "clear"})).unwrap();
        assert_eq!(Fixture::paths(&prev["items"]), ["out.bin"]);
        let out = f.run("clear", json!({})).unwrap();
        assert_eq!(out["deleted"], 1);
        assert!(f.exists("target") && !f.exists("target/out.bin"));
        assert_eq!(f.fake.binned().len(), 1);
    }

    #[test]
    fn reveal_touches_nothing_and_shows_the_largest() {
        let mut f = fixture();
        f.save(json!({"id": "show", "name": "Show", "action": "reveal", "selector": {"min_size": 50, "kind": "file"}}))
            .unwrap();
        let out = f.run("show", json!({})).unwrap();
        assert_eq!(
            (out["count"].as_u64(), out["deleted"].as_u64(), out["tree_updated"].as_bool()),
            (Some(1), Some(1), Some(false))
        );
        assert!(matches!(f.fake.calls().as_slice(), [PlatformCall::Reveal(p)] if p.ends_with("out.bin")));
        assert!(f.exists("target/out.bin"));
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["total"], 0);
    }

    #[test]
    fn only_under_restricts_and_rejects_outside_paths() {
        let mut f = fixture();
        let sub = f.root().join("sub").to_string_lossy().into_owned();
        f.save(json!({"id": "scoped", "name": "Scoped", "action": "recycle",
            "selector": {"names": ["*.tmp", "*.temp"], "only_under": sub}}))
            .unwrap();
        let out = f.s.execute("cleanup.preview", &json!({"id": "scoped"})).unwrap();
        let mut names = Fixture::paths(&out["items"]);
        names.sort();
        assert_eq!(names, ["b.temp", "c.tmp"]);
        let outside = f.dir.path().to_string_lossy().into_owned();
        f.save(json!({"id": "out", "name": "Out", "action": "recycle",
            "selector": {"names": ["*"], "only_under": outside}}))
            .unwrap();
        assert_eq!(f.s.execute("cleanup.preview", &json!({"id": "out"})).unwrap_err().code(), "invalid_params");
        assert_eq!(f.run("out", json!({})).unwrap_err().code(), "invalid_params");
        let file_path = f.root().join("keep.txt").to_string_lossy().into_owned();
        f.save(json!({"id": "file", "name": "F", "action": "recycle",
            "selector": {"names": ["*"], "only_under": file_path}}))
            .unwrap();
        assert_eq!(f.s.execute("cleanup.preview", &json!({"id": "file"})).unwrap_err().code(), "invalid_params");
        assert!(f.fake.calls().is_empty());
    }

    #[test]
    fn platform_failure_stops_the_run_cleanly() {
        let mut f = fixture();
        f.fake.set_failure(Some(PlatformError::Unsupported("recycle bin")));
        let out = f.run("temporary-files", json!({})).unwrap();
        assert!(out["stopped"].is_string());
        assert_eq!(
            (out["deleted"].as_u64(), out["failed"].as_u64(), out["not_attempted"].as_u64()),
            (Some(0), Some(1), Some(2))
        );
        assert_eq!(out["done"], false);
        assert_eq!(out["tree_updated"], false);
        assert!(f.exists("a.tmp"));
        // Repeated per-node failures stop after the limit, not after the whole list.
        f.fake.set_failure(Some(PlatformError::PermissionDenied("x".into())));
        f.save(json!({"id": "bin", "name": "Bin", "action": "recycle", "selector": {"names": ["*"], "kind": "file"}}))
            .unwrap();
        let out = f.run("bin", json!({})).unwrap();
        assert_eq!(out["failed"], i64::from(MAX_CONSECUTIVE_PLATFORM_FAILURES));
        assert!(out["stopped"].is_string() && out["not_attempted"].as_u64() > Some(0));
        assert_eq!(f.s.execute("file.journal", &json!({})).unwrap()["entries"][0]["ok"], false);
    }

    #[test]
    fn empty_session_disables_preview_and_run_but_not_definitions() {
        let mut s = Session::new();
        for id in ["cleanup.preview", "cleanup.run"] {
            let err = s.execute(id, &json!({"id": "temporary-files", "confirm": true})).unwrap_err();
            assert_eq!(err.code(), "not_enabled", "{id}");
        }
        assert!(s.execute("cleanup.list", &json!({})).is_ok());
        let infos = s.commands();
        assert!(infos.iter().any(|c| c.id == "cleanup.run" && c.destructive));
        for id in ["cleanup.list", "cleanup.save", "cleanup.remove", "cleanup.preview"] {
            assert!(infos.iter().any(|c| c.id == id && !c.destructive), "{id}");
        }
    }

    #[test]
    fn hostile_run_params_do_not_panic_or_delete() {
        let mut f = fixture();
        let long = "x".repeat(100_000);
        for p in [
            json!({}),
            json!({"id": 5}),
            json!({"id": long}),
            json!({"id": "a\u{0}"}),
            json!({"id": "temporary-files", "dry_run": "yes"}),
            json!({"id": "temporary-files", "bogus": 1}),
            json!({"id": "../../x"}),
        ] {
            let mut p = p;
            if let Some(m) = p.as_object_mut() {
                m.insert("confirm".to_owned(), json!(true));
            }
            match f.s.execute("cleanup.run", &p) {
                Err(CommandError::Panicked { .. }) => panic!("panicked on {p}"),
                Ok(v) => panic!("succeeded on {p}: {v}"),
                Err(_) => {}
            }
        }
        assert!(f.exists("a.tmp") && f.fake.calls().is_empty());
    }
}
