//! `duplicates.*` commands (G-020): find files with identical content. Read-only; the search runs
//! on a background thread owned by the session, like a scan: `duplicates.find` returns at once,
//! `duplicates.status` reports progress and finishes the job, `duplicates.results` pages the groups.
//! Nothing here deletes anything: removing a copy goes through `file.delete` like any other file.

use super::tree::{kind_name, path_in};
use super::{CommandSpec, always, no_params, parse_params};
use crate::duplicates::{Group, Job, Outcome, Params};
use crate::{CommandError, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Most groups a search may keep.
pub const MAX_KEEP: usize = 10_000;
const DEFAULT_KEEP: usize = 1_000;
/// Most groups a `duplicates.results` page returns.
const MAX_PAGE: usize = 200;
const DEFAULT_PAGE: usize = 50;
/// Members listed per group in a page (`count` always carries the real number).
const MAX_FILES_LISTED: usize = 200;
const MAX_PREFIX_CHARS: usize = 4096;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "duplicates.find",
            label: "Find duplicate files",
            menu: "",
            shortcut: None,
            params_doc: "{\"min_size\"?: u64 (default 1; zero-size files are never duplicates), \"path_prefix\"?: \
                string (<= 4096 chars; only files below this path, in the spelling of the scan root, case-insensitive \
                on Windows), \"verify\"?: bool (default false; byte-compare the members of every hash group after \
                the BLAKE3 match), \"limit\"?: 1..=10000 (groups kept, default 1000, most wasted space first; totals \
                stay exact)}. Read-only background job: files are grouped by logical size, then by a partial hash \
                (first and last 64 KiB), then by the full content hash. Hardlinks (several names of one file) and \
                reparse points are not duplicates; vanished, unreadable or changed files are skipped and counted. \
                Returns at once with {started, revision}; read progress and finish with duplicates.status, read \
                the groups with duplicates.results. Any tree change (scan, delete) drops the job and its result.",
            destructive: false,
            enabled: |s| !s.is_scanning() && !s.duplicates_running(),
            run: run_find,
        },
        CommandSpec {
            id: "duplicates.status",
            label: "Duplicate search status",
            menu: "",
            shortcut: None,
            params_doc: "none. Finishes a completed search without blocking. Returns {state: \"idle\" | \"running\" | \
                \"done\" | \"cancelled\" | \"failed\", revision, stage (running: collecting | partial_hash | \
                full_hash | verify | finishing), progress (running: {stage_total, stage_done, bytes_read, problems}), \
                summary (done: {total_groups, total_files, wasted_bytes, kept_groups, truncated, candidates, \
                hardlinks_collapsed, bytes_read, problems, problem_samples}), error (failed)}.",
            destructive: false,
            enabled: always,
            run: run_status,
        },
        CommandSpec {
            id: "duplicates.results",
            label: "Duplicate groups",
            menu: "",
            shortcut: None,
            params_doc: "{\"offset\"?: u64 (default 0), \"limit\"?: 1..=200 (default 50), \"revision\"?: u64 (the \
                revision the caller read ids at; any other current revision is stale_tree)}. Pages the kept \
                groups, most wasted space first. Returns {revision, total_groups, kept_groups, offset, truncated, \
                wasted_bytes, groups: [{index, size, wasted, count, files_truncated, files: [{id, name, path}]}]}; \
                at most 200 members are listed per group (count is the real number). Ids are valid for the \
                returned revision (select.node, file.* need it). Not enabled until a search has finished.",
            destructive: false,
            enabled: Session::has_duplicate_results,
            run: run_results,
        },
        CommandSpec {
            id: "duplicates.cancel",
            label: "Cancel duplicate search",
            menu: "",
            shortcut: None,
            params_doc: "none. Asks the running search to stop; its partial findings are dropped.",
            destructive: false,
            enabled: Session::duplicates_running,
            run: run_cancel,
        },
        CommandSpec {
            id: "duplicates.clear",
            label: "Clear duplicate results",
            menu: "",
            shortcut: None,
            params_doc: "none. Cancels a running search and drops the held result; returns {cleared}: the number of \
                groups that were held.",
            destructive: false,
            enabled: always,
            run: run_clear,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindParams {
    min_size: Option<u64>,
    path_prefix: Option<String>,
    verify: Option<bool>,
    limit: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultsParams {
    offset: Option<u64>,
    limit: Option<u64>,
    revision: Option<u64>,
}

/// What the session holds about the duplicate search.
#[derive(Debug, Default)]
pub(crate) struct DupState {
    running: Option<Running>,
    finished: Option<Finished>,
}

#[derive(Debug)]
struct Running {
    job: Job,
    revision: u64,
}

#[derive(Debug)]
struct Finished {
    revision: u64,
    result: Result<Outcome, String>,
}

fn run_find(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: FindParams = parse_params(params)?;
    if let Some(prefix) = &p.path_prefix
        && (prefix.is_empty() || prefix.contains('\0') || prefix.chars().count() > MAX_PREFIX_CHARS)
    {
        return Err(CommandError::InvalidParams(format!(
            "path_prefix must be 1..={MAX_PREFIX_CHARS} characters without NUL"
        )));
    }
    let keep = match p.limit {
        None => DEFAULT_KEEP,
        Some(n) => usize::try_from(n)
            .ok()
            .filter(|n| (1..=MAX_KEEP).contains(n))
            .ok_or_else(|| CommandError::InvalidParams(format!("limit must be 1..={MAX_KEEP}")))?,
    };
    let params = Params {
        min_size: p.min_size.unwrap_or(1),
        path_prefix: p.path_prefix,
        verify: p.verify.unwrap_or(false),
        keep,
    };
    let revision = session.start_duplicates(params)?;
    Ok(json!({ "started": true, "revision": revision }))
}

fn run_status(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    Ok(session.duplicates_status())
}

fn run_cancel(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    session.cancel_duplicates();
    Ok(json!({ "cancelling": true }))
}

fn run_clear(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    Ok(json!({ "cleared": session.clear_duplicates() }))
}

fn run_results(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: ResultsParams = parse_params(params)?;
    session.poll_duplicates();
    if let Some(rev) = p.revision
        && rev != session.revision()
    {
        return Err(CommandError::StaleTree(format!(
            "ids were read at revision {rev} but the tree is at revision {}; read them again",
            session.revision()
        )));
    }
    let limit = match p.limit {
        None => DEFAULT_PAGE,
        Some(n) => usize::try_from(n)
            .ok()
            .filter(|n| (1..=MAX_PAGE).contains(n))
            .ok_or_else(|| CommandError::InvalidParams(format!("limit must be 1..={MAX_PAGE}")))?,
    };
    let offset = usize::try_from(p.offset.unwrap_or(0)).unwrap_or(usize::MAX);
    let Some(outcome) = session.duplicate_outcome() else {
        return Err(CommandError::NotEnabled("duplicates.results: no finished search".to_owned()));
    };
    let tree = session.tree();
    let groups: Vec<Value> = outcome
        .groups
        .iter()
        .enumerate()
        .skip(offset)
        .take(limit)
        .map(|(index, g)| group_json(tree, index, g))
        .collect();
    Ok(json!({
        "revision": session.revision(),
        "total_groups": outcome.total_groups,
        "kept_groups": outcome.groups.len(),
        "offset": offset,
        "truncated": outcome.total_groups > u64::try_from(outcome.groups.len()).unwrap_or(u64::MAX),
        "wasted_bytes": outcome.wasted_bytes,
        "groups": groups,
    }))
}

fn group_json(tree: &dirustat_model::Tree, index: usize, g: &Group) -> Value {
    let files: Vec<Value> = g
        .files
        .iter()
        .take(MAX_FILES_LISTED)
        .filter_map(|&id| {
            let node = tree.get(id)?;
            Some(json!({
                "id": id.index(),
                "name": node.name(),
                "kind": kind_name(node.kind()),
                "path": path_in(tree, id),
            }))
        })
        .collect();
    json!({
        "index": index,
        "size": g.size,
        "wasted": g.wasted(),
        "count": g.files.len(),
        "files_truncated": g.files.len() > MAX_FILES_LISTED,
        "files": files,
    })
}

impl Session {
    /// True while a duplicate search runs.
    pub fn duplicates_running(&self) -> bool {
        self.dups.running.is_some()
    }

    /// True when a finished, uncancelled search result is held.
    pub fn has_duplicate_results(&self) -> bool {
        self.duplicate_outcome().is_some()
    }

    /// The held result, if any. Always belongs to the current tree revision.
    pub fn duplicate_outcome(&self) -> Option<&Outcome> {
        match &self.dups.finished {
            Some(Finished { result: Ok(outcome), revision, .. })
                if !outcome.cancelled && *revision == self.revision() =>
            {
                Some(outcome)
            }
            _ => None,
        }
    }

    /// Starts the background search over the current tree; returns the revision it works on.
    pub(crate) fn start_duplicates(&mut self, params: Params) -> Result<u64, CommandError> {
        if self.is_scanning() || self.dups.running.is_some() {
            return Err(CommandError::NotEnabled("duplicates.find".to_owned()));
        }
        let job = Job::start(Arc::clone(&self.tree), params)
            .map_err(|e| CommandError::Platform(format!("cannot start the duplicate search: {e}")))?;
        let revision = self.revision();
        self.dups.finished = None;
        self.dups.running = Some(Running { job, revision });
        Ok(revision)
    }

    /// Non-blocking: moves a finished job's outcome into the session. Returns whether it did.
    pub fn poll_duplicates(&mut self) -> bool {
        if !self.dups.running.as_ref().is_some_and(|r| r.job.is_finished()) {
            return false;
        }
        let Some(Running { job, revision }) = self.dups.running.take() else { return false };
        let result = job.join();
        self.dups.finished = Some(Finished { revision, result });
        true
    }

    /// Asks the running search to stop; `poll_duplicates` then records it as cancelled.
    pub fn cancel_duplicates(&mut self) {
        if let Some(r) = &self.dups.running {
            r.job.cancel();
        }
    }

    /// Cancels a running search and drops the result; returns the number of groups held.
    pub fn clear_duplicates(&mut self) -> usize {
        let held = self.duplicate_outcome().map_or(0, |o| o.groups.len());
        self.invalidate_duplicates();
        held
    }

    /// The tree changed (or the user cleared the panel): stop the job and forget everything.
    pub(crate) fn invalidate_duplicates(&mut self) {
        if let Some(r) = self.dups.running.take() {
            // The thread notices the flag within one buffer and ends on its own; nobody joins it.
            r.job.cancel();
        }
        self.dups.finished = None;
    }

    /// Search state for `duplicates.status` and the shell.
    pub fn duplicates_status(&mut self) -> Value {
        self.poll_duplicates();
        if let Some(r) = &self.dups.running {
            let p = r.job.progress();
            return json!({
                "state": "running",
                "revision": r.revision,
                "stage": p.stage.as_str(),
                "progress": {
                    "stage_total": p.stage_total,
                    "stage_done": p.stage_done,
                    "bytes_read": p.bytes_read,
                    "problems": p.problems,
                },
            });
        }
        match &self.dups.finished {
            None => json!({ "state": "idle", "revision": self.revision() }),
            Some(Finished { revision, result: Err(why), .. }) => {
                json!({ "state": "failed", "revision": revision, "error": why })
            }
            Some(Finished { revision, result: Ok(o), .. }) if o.cancelled => {
                json!({ "state": "cancelled", "revision": revision })
            }
            Some(Finished { revision, result: Ok(o), .. }) => {
                let kept = u64::try_from(o.groups.len()).unwrap_or(u64::MAX);
                json!({
                    "state": "done",
                    "revision": revision,
                    "summary": {
                        "total_groups": o.total_groups,
                        "total_files": o.total_files,
                        "wasted_bytes": o.wasted_bytes,
                        "kept_groups": kept,
                        "truncated": o.total_groups > kept,
                        "candidates": o.candidates,
                        "hardlinks_collapsed": o.hardlinks_collapsed,
                        "bytes_read": o.bytes_read,
                        "problems": o.problems,
                        "problem_samples": o.problem_samples,
                    },
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::{NodeKind, Tree};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    const CHUNK_BYTES: usize = 64 * 1024;

    /// Deterministic content of `len` bytes; different seeds differ in most bytes.
    fn bytes(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from((i * 7 + seed * 13 + i / 251) % 251).unwrap_or(0)).collect()
    }

    fn write(root: &Path, rel: &str, content: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn scanned(root: &Path) -> Session {
        let mut s = Session::new();
        let handle = dirustat_scan::start(root, dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        s
    }

    /// Polls `duplicates.status` until the job is over.
    fn wait(s: &mut Session) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let v = s.execute("duplicates.status", &json!({})).unwrap();
            if v["state"] != "running" {
                return v;
            }
            assert!(Instant::now() < deadline, "the duplicate search did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn find(s: &mut Session, params: &Value) -> Value {
        s.execute("duplicates.find", params).unwrap();
        let status = wait(s);
        assert_eq!(status["state"], "done", "{status}");
        status
    }

    fn groups(s: &mut Session) -> Vec<Value> {
        s.execute("duplicates.results", &json!({"limit": 200})).unwrap()["groups"].as_array().unwrap().clone()
    }

    fn names(group: &Value) -> Vec<String> {
        let mut v: Vec<String> =
            group["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn identical_files_form_a_group_and_report_wasted_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let content = bytes(300_000, 1);
        write(dir.path(), "a/one.bin", &content);
        write(dir.path(), "b/two.bin", &content);
        write(dir.path(), "c/three.bin", &content);
        write(dir.path(), "c/other.bin", &bytes(300_000, 2));
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({}));
        let sum = &status["summary"];
        assert_eq!((sum["total_groups"].as_u64(), sum["total_files"].as_u64()), (Some(1), Some(3)));
        assert_eq!(sum["wasted_bytes"], 600_000);
        assert_eq!(sum["candidates"], 4);
        assert_eq!(sum["problems"], 0);
        let g = groups(&mut s);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0]["size"], 300_000);
        assert_eq!(g[0]["count"], 3);
        assert_eq!(g[0]["wasted"], 600_000);
        assert_eq!(names(&g[0]), ["one.bin", "three.bin", "two.bin"]);
        // The ids work with the existing commands: a shell selects a member in the treemap.
        let id = g[0]["files"][0]["id"].as_u64().unwrap();
        assert!(s.execute("select.node", &json!({"id": id})).is_ok());
    }

    #[test]
    fn small_files_are_settled_by_the_partial_hash_alone() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "x/a.txt", b"hello world");
        write(dir.path(), "y/b.txt", b"hello world");
        write(dir.path(), "y/c.txt", b"hello_world");
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({}));
        assert_eq!(status["summary"]["total_groups"], 1);
        // 11 bytes per file, read once each.
        assert_eq!(status["summary"]["bytes_read"], 33);
        assert_eq!(names(&groups(&mut s)[0]), ["a.txt", "b.txt"]);
    }

    #[test]
    fn same_size_different_content_is_not_a_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.bin", &bytes(200_000, 1));
        write(dir.path(), "b.bin", &bytes(200_000, 2));
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({}));
        assert_eq!(status["summary"]["total_groups"], 0);
        assert_eq!(status["summary"]["candidates"], 2);
        assert!(groups(&mut s).is_empty());
    }

    #[test]
    fn a_partial_hash_collision_is_resolved_by_the_full_hash() {
        // Same first and last 64 KiB, different middle: the partial hashes are equal.
        let head_tail = bytes(CHUNK_BYTES, 3);
        let make = |mid: u8| {
            let mut v = head_tail.clone();
            v.extend(std::iter::repeat_n(mid, 100_000));
            v.extend_from_slice(&head_tail);
            v
        };
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "one.bin", &make(1));
        write(dir.path(), "two.bin", &make(2));
        write(dir.path(), "three.bin", &make(1));
        for verify in [false, true] {
            let mut s = scanned(dir.path());
            let status = find(&mut s, &json!({"verify": verify}));
            assert_eq!(status["summary"]["total_groups"], 1, "verify={verify}");
            assert_eq!(status["summary"]["total_files"], 2, "verify={verify}");
            assert_eq!(names(&groups(&mut s)[0]), ["one.bin", "three.bin"], "verify={verify}");
        }
    }

    #[test]
    fn hardlinked_names_are_not_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "orig.bin", &bytes(150_000, 5));
        if std::fs::hard_link(dir.path().join("orig.bin"), dir.path().join("link.bin")).is_err() {
            return; // no hardlink support on this file system
        }
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({}));
        assert_eq!(status["summary"]["total_groups"], 0, "{status}");
        // A real copy next to them is a duplicate of the file (once, not twice).
        write(dir.path(), "copy.bin", &bytes(150_000, 5));
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({}));
        let sum = &status["summary"];
        assert_eq!(sum["total_groups"], 1, "{status}");
        assert_eq!(sum["wasted_bytes"], 150_000, "{status}");
        assert_eq!(sum["total_files"], 2);
    }

    #[test]
    fn two_names_of_one_file_collapse_even_when_the_scan_did_not_flag_them() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "orig.bin", &bytes(1000, 6));
        if std::fs::hard_link(dir.path().join("orig.bin"), dir.path().join("link.bin")).is_err() {
            return;
        }
        write(dir.path(), "copy.bin", &bytes(1000, 6));
        // A hand-made tree: every name has the full size and no hardlink flag (like a backend
        // that cannot tell).
        let mut t = Tree::new(&dir.path().to_string_lossy());
        for name in ["orig.bin", "link.bin", "copy.bin"] {
            t.add_child(t.root(), name, NodeKind::File, 1000).unwrap();
        }
        t.aggregate();
        let mut s = Session::new();
        s.replace_tree(t);
        let status = find(&mut s, &json!({}));
        let sum = &status["summary"];
        assert_eq!((sum["total_groups"].as_u64(), sum["total_files"].as_u64()), (Some(1), Some(2)), "{status}");
        assert_eq!(sum["hardlinks_collapsed"], 1);
        assert_eq!(sum["wasted_bytes"], 1000);
    }

    #[test]
    fn empty_files_and_small_files_below_min_size_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["e1", "e2", "e3"] {
            write(dir.path(), n, b"");
        }
        write(dir.path(), "s1", &bytes(500, 1));
        write(dir.path(), "s2", &bytes(500, 1));
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({"min_size": 0}));
        assert_eq!(status["summary"]["total_groups"], 1, "zero-size files are never duplicates");
        assert_eq!(status["summary"]["candidates"], 2);
        let status = find(&mut s, &json!({"min_size": 501}));
        assert_eq!(status["summary"]["total_groups"], 0);
        assert_eq!(status["summary"]["candidates"], 0);
    }

    #[test]
    fn a_file_that_vanishes_or_changes_is_skipped_and_counted() {
        let dir = tempfile::tempdir().unwrap();
        let content = bytes(70_000, 9);
        for n in ["a", "b", "c", "d"] {
            write(dir.path(), n, &content);
        }
        let mut s = scanned(dir.path());
        std::fs::remove_file(dir.path().join("a")).unwrap();
        write(dir.path(), "b", &bytes(70_001, 9)); // grew after the scan
        let status = find(&mut s, &json!({}));
        let sum = &status["summary"];
        assert_eq!((sum["total_groups"].as_u64(), sum["total_files"].as_u64()), (Some(1), Some(2)), "{status}");
        assert_eq!(sum["problems"], 2);
        let samples: Vec<&str> =
            sum["problem_samples"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(samples.iter().any(|m| m.ends_with("vanished")), "{samples:?}");
        assert!(samples.iter().any(|m| m.contains("size changed")), "{samples:?}");
        assert_eq!(names(&groups(&mut s)[0]), ["c", "d"]);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn reparse_points_and_symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let content = bytes(5000, 4);
        write(dir.path(), "real.bin", &content);
        write(dir.path(), "copy.bin", &content);
        let mut s = scanned(dir.path());
        // Swap one copy for a link after the scan: the tree still says "file".
        std::fs::remove_file(dir.path().join("copy.bin")).unwrap();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(dir.path().join("real.bin"), dir.path().join("copy.bin")).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(dir.path().join("real.bin"), dir.path().join("copy.bin")).is_ok();
        if !made {
            return; // no privilege to create symlinks here
        }
        let status = find(&mut s, &json!({}));
        assert_eq!(status["summary"]["total_groups"], 0, "{status}");
        assert_eq!(status["summary"]["problems"], 1);
        assert!(status["summary"]["problem_samples"][0].as_str().unwrap().contains("reparse"));
    }

    #[test]
    fn path_prefix_limits_the_search() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "x/a1", &bytes(2000, 1));
        write(dir.path(), "x/a2", &bytes(2000, 1));
        write(dir.path(), "y/b1", &bytes(3000, 2));
        write(dir.path(), "y/b2", &bytes(3000, 2));
        write(dir.path(), "xx/a3", &bytes(2000, 1));
        let mut s = scanned(dir.path());
        let root = s.node_path(s.tree().root()).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let status = find(&mut s, &json!({"path_prefix": format!("{root}{sep}x")}));
        assert_eq!(status["summary"]["total_groups"], 1, "`x` must not match `xx`: {status}");
        assert_eq!(names(&groups(&mut s)[0]), ["a1", "a2"]);
        // A trailing separator, and another case where the file system ignores case.
        if cfg!(windows) {
            let status = find(&mut s, &json!({"path_prefix": format!("{}{sep}Y{sep}", root.to_uppercase())}));
            assert_eq!(status["summary"]["total_groups"], 1, "{status}");
            assert_eq!(names(&groups(&mut s)[0]), ["b1", "b2"]);
        }
        // One of the pair inside the prefix, the other outside: no group.
        let status = find(&mut s, &json!({"path_prefix": format!("{root}{sep}xx")}));
        assert_eq!(status["summary"]["total_groups"], 0);
        // A prefix outside the tree finds nothing.
        let status = find(&mut s, &json!({"path_prefix": format!("{root}{sep}no{sep}such")}));
        assert_eq!(status["summary"]["candidates"], 0);
    }

    #[test]
    fn groups_are_ordered_by_wasted_space_and_limited() {
        let dir = tempfile::tempdir().unwrap();
        for (i, (size, copies)) in [(1000usize, 2usize), (5000, 2), (800, 4)].into_iter().enumerate() {
            for c in 0..copies {
                write(dir.path(), &format!("g{i}/f{c}"), &bytes(size, i + 1));
            }
        }
        let mut s = scanned(dir.path());
        let status = find(&mut s, &json!({"limit": 2}));
        let sum = &status["summary"];
        assert_eq!(sum["total_groups"], 3);
        assert_eq!(sum["kept_groups"], 2);
        assert_eq!(sum["truncated"], true);
        assert_eq!(sum["wasted_bytes"], 5000 + 1000 + 2400);
        let page = s.execute("duplicates.results", &json!({})).unwrap();
        let wasted: Vec<u64> =
            page["groups"].as_array().unwrap().iter().map(|g| g["wasted"].as_u64().unwrap()).collect();
        assert_eq!(wasted, [5000, 2400]);
        assert_eq!(page["truncated"], true);
        let second = s.execute("duplicates.results", &json!({"offset": 1, "limit": 1})).unwrap();
        assert_eq!(second["groups"][0]["wasted"], 2400);
        assert_eq!(second["groups"][0]["index"], 1);
    }

    #[test]
    fn cancel_stops_the_job_and_leaves_no_result() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", &bytes(1000, 1));
        write(dir.path(), "b", &bytes(1000, 1));
        let mut s = scanned(dir.path());
        // Core level, deterministic: a job whose cancel flag is already set.
        let cancel = AtomicBool::new(true);
        let progress = crate::duplicates::Progress::default();
        let params = Params { min_size: 1, path_prefix: None, verify: true, keep: 10 };
        let out = crate::duplicates::find(s.tree(), &params, &cancel, &progress);
        assert!(out.cancelled && out.groups.is_empty());
        // Command level: cancelled or already done, never running, never half a result.
        s.execute("duplicates.find", &json!({})).unwrap();
        assert!(s.execute("duplicates.find", &json!({})).is_err(), "one search at a time");
        s.execute("duplicates.cancel", &json!({})).unwrap();
        let status = wait(&mut s);
        assert!(["cancelled", "done"].contains(&status["state"].as_str().unwrap()), "{status}");
        s.execute("duplicates.clear", &json!({})).unwrap();
        assert_eq!(s.execute("duplicates.status", &json!({})).unwrap()["state"], "idle");
        assert_eq!(s.execute("duplicates.results", &json!({})).unwrap_err().code(), "not_enabled");
    }

    #[test]
    fn a_new_tree_drops_the_result_and_stale_revisions_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", &bytes(1000, 1));
        write(dir.path(), "b", &bytes(1000, 1));
        let mut s = scanned(dir.path());
        find(&mut s, &json!({}));
        let rev = s.revision();
        assert!(s.execute("duplicates.results", &json!({"revision": rev})).is_ok());
        assert_eq!(s.execute("duplicates.results", &json!({"revision": rev + 1})).unwrap_err().code(), "stale_tree");
        // A running search is dropped as well.
        s.execute("duplicates.find", &json!({})).unwrap();
        s.replace_tree(Tree::new("other"));
        assert!(!s.duplicates_running());
        assert_eq!(s.execute("duplicates.status", &json!({})).unwrap()["state"], "idle");
        assert_eq!(s.execute("duplicates.results", &json!({})).unwrap_err().code(), "not_enabled");
    }

    #[test]
    fn the_search_never_changes_the_disk_or_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", &bytes(1000, 1));
        write(dir.path(), "b", &bytes(1000, 1));
        let mut s = scanned(dir.path());
        let (rev, nodes) = (s.revision(), s.tree().len());
        find(&mut s, &json!({"verify": true}));
        assert_eq!((s.revision(), s.tree().len()), (rev, nodes));
        assert!(dir.path().join("a").exists() && dir.path().join("b").exists());
        assert!(s.commands().iter().filter(|c| c.id.starts_with("duplicates.")).all(|c| !c.destructive));
    }

    #[test]
    fn invalid_params_are_rejected() {
        let mut s = Session::new();
        let long = "x".repeat(MAX_PREFIX_CHARS + 1);
        for p in [
            json!({"limit": 0}),
            json!({"limit": 10_001}),
            json!({"limit": -1}),
            json!({"min_size": -1}),
            json!({"min_size": "big"}),
            json!({"verify": "yes"}),
            json!({"path_prefix": ""}),
            json!({"path_prefix": "a\u{0}b"}),
            json!({"path_prefix": long}),
            json!({"bogus": 1}),
        ] {
            assert_eq!(s.execute("duplicates.find", &p).unwrap_err().code(), "invalid_params", "{p}");
        }
        assert_eq!(s.execute("duplicates.status", &json!({"x": 1})).unwrap_err().code(), "invalid_params");
        assert_eq!(s.execute("duplicates.clear", &json!({"x": 1})).unwrap_err().code(), "invalid_params");
        // An empty session has nothing: the search finishes at once.
        s.execute("duplicates.find", &json!(null)).unwrap();
        assert_eq!(wait(&mut s)["summary"]["total_groups"], 0);
        assert_eq!(s.execute("duplicates.results", &json!({"limit": 0})).unwrap_err().code(), "invalid_params");
    }

    #[test]
    fn find_is_not_enabled_while_a_scan_runs() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", b"x");
        let mut s = Session::new();
        s.execute("scan.start", &json!({"path": dir.path().to_string_lossy()})).unwrap();
        if s.is_scanning() {
            assert_eq!(s.execute("duplicates.find", &json!({})).unwrap_err().code(), "not_enabled");
        }
    }
}
