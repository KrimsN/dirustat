//! `list.*` commands: flat top-N file list and mounted drives.

use super::tree::{MAX_PAGE, page_limit};
use super::{CommandSpec, always, no_params, parse_params};
use crate::{CommandError, DriveInfo, MAX_DRIVES, Session, extension_of};
use dirustat_model::{NodeId, NodeKind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "list.largest_files",
            label: "Largest files",
            menu: "View/Largest files",
            shortcut: None,
            params_doc: "{\"limit\"?: 1..=10000 (default 100), \"offset\"?: 0..=10000 (default 0), \"min_size\"?: \
                u64 (default 0)} the largest files by the session size mode, descending. Returns {total_returned, \
                offset, items: [{id, path, size, ext}]}.",
            destructive: false,
            enabled: always,
            run: run_largest,
        },
        CommandSpec {
            id: "list.drives",
            label: "Drives",
            menu: "",
            shortcut: None,
            params_doc: "none. Returns {drives: [{path, label, total, free}]} from the custom provider if the \
                shell set one, else from the platform services (empty when they fail or are unsupported).",
            destructive: false,
            enabled: always,
            run: run_drives,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LargestParams {
    limit: Option<u64>,
    offset: Option<u64>,
    min_size: Option<u64>,
}

/// Heap entry: ordered so that a larger size wins and, on ties, the smaller arena index wins.
type Entry = (u64, Reverse<NodeId>);

fn run_largest(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: LargestParams = parse_params(params)?;
    let limit = page_limit(p.limit)?;
    let offset = match p.offset {
        None => 0,
        Some(o) => usize::try_from(o)
            .ok()
            .filter(|n| *n <= MAX_PAGE)
            .ok_or_else(|| CommandError::InvalidParams(format!("offset must be 0..={MAX_PAGE}")))?,
    };
    let min_size = p.min_size.unwrap_or(0);
    let found = session.largest_files(limit.saturating_add(offset), min_size);
    let mode = session.map_size_mode();
    let items: Vec<Value> = found
        .into_iter()
        .skip(offset)
        .filter_map(|id| {
            let node = session.tree().get(id)?;
            Some(json!({
                "id": id.index(),
                "path": session.node_path(id)?,
                "size": node.own_size(mode),
                "ext": extension_of(node.name()),
            }))
        })
        .collect();
    Ok(json!({ "total_returned": items.len(), "offset": offset, "items": items }))
}

fn run_drives(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    Ok(json!({ "drives": session.drives() }))
}

impl Session {
    /// The `count` largest files (links included, directories not) with size >= `min_size` by the
    /// session size mode, largest first, ties by arena index. One iterative pass over the tree
    /// with a bounded min-heap: O(n log count) time, O(count) memory.
    ///
    /// Known cost: this runs while the caller holds the session lock. A result cache keyed by
    /// (`Session::revision`, size mode, count, min_size) would make repeated calls O(count); it is
    /// deferred (not needed yet at the current call rates).
    pub fn largest_files(&self, count: usize, min_size: u64) -> Vec<NodeId> {
        let count = count.min(MAX_PAGE.saturating_mul(2));
        if count == 0 {
            return Vec::new();
        }
        let tree = self.tree();
        let mode = self.map_size_mode();
        let mut heap: BinaryHeap<Reverse<Entry>> = BinaryHeap::new();
        let mut stack = vec![tree.root()];
        while let Some(id) = stack.pop() {
            let Some(node) = tree.get(id) else { continue };
            if node.kind() == NodeKind::Dir {
                stack.extend(tree.children(id));
                continue;
            }
            let size = node.own_size(mode);
            if size < min_size {
                continue;
            }
            let entry: Entry = (size, Reverse(id));
            if heap.len() < count {
                heap.push(Reverse(entry));
            } else if heap.peek().is_some_and(|Reverse(worst)| entry > *worst) {
                heap.pop();
                heap.push(Reverse(entry));
            }
        }
        let mut best: Vec<Entry> = heap.into_iter().map(|Reverse(e)| e).collect();
        best.sort_by(|a, b| b.cmp(a));
        best.into_iter().map(|(_, Reverse(id))| id).collect()
    }

    /// Overrides the source of `list.drives` with a custom function. Without one the session's
    /// platform services ([`Session::set_platform`]) answer.
    pub fn set_drives_provider(&mut self, provider: Box<dyn Fn() -> Vec<DriveInfo> + Send>) {
        self.drives.0 = Some(provider);
    }

    /// Volumes from the custom provider if set, else from the platform services (an error there
    /// means "no drives"), bounded to [`MAX_DRIVES`] with `free <= total`.
    pub fn drives(&self) -> Vec<DriveInfo> {
        let raw: Vec<DriveInfo> = match &self.drives.0 {
            Some(provider) => provider(),
            None => self.platform.drives().unwrap_or_default().into_iter().map(DriveInfo::from).collect(),
        };
        raw.into_iter().take(MAX_DRIVES).map(DriveInfo::sanitized).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_files(n: usize) -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("d")).unwrap();
        for i in 0..n {
            let sub = if i % 2 == 0 { dir.path().join("d") } else { dir.path().to_path_buf() };
            std::fs::write(sub.join(format!("f{i}.dat")), vec![0u8; (i + 1) * 100]).unwrap();
        }
        let mut s = Session::new();
        let h = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(h), Ok(true));
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        (dir, s)
    }

    fn sizes(v: &Value) -> Vec<u64> {
        v["items"].as_array().unwrap().iter().map(|i| i["size"].as_u64().unwrap()).collect()
    }

    #[test]
    fn largest_files_top_n_paging_and_min_size() {
        let (_d, mut s) = session_with_files(10);
        let top = s.execute("list.largest_files", &json!({"limit": 3})).unwrap();
        assert_eq!(sizes(&top), [1000, 900, 800]);
        assert_eq!(top["items"][0]["ext"], "dat");
        assert!(top["items"][0]["path"].as_str().unwrap().ends_with("f9.dat"));
        let page = s.execute("list.largest_files", &json!({"limit": 2, "offset": 2})).unwrap();
        assert_eq!(sizes(&page), [800, 700]);
        let min = s.execute("list.largest_files", &json!({"min_size": 900})).unwrap();
        assert_eq!(sizes(&min), [1000, 900]);
        let all = s.execute("list.largest_files", &json!({})).unwrap();
        assert_eq!(sizes(&all).len(), 10);
        let none = s.execute("list.largest_files", &json!({"min_size": u64::MAX})).unwrap();
        assert!(sizes(&none).is_empty());
    }

    #[test]
    fn ties_are_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..6 {
            std::fs::write(dir.path().join(format!("t{i}")), b"same").unwrap();
        }
        let mut s = Session::new();
        let h = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        s.finish_scan(h).unwrap();
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        let a = s.execute("list.largest_files", &json!({"limit": 3})).unwrap();
        let b = s.execute("list.largest_files", &json!({"limit": 3})).unwrap();
        assert_eq!(a, b);
        let ids: Vec<u64> = a["items"].as_array().unwrap().iter().map(|i| i["id"].as_u64().unwrap()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn largest_files_validation_and_empty_tree() {
        let mut s = Session::new();
        let out = s.execute("list.largest_files", &json!({})).unwrap();
        assert!(out["items"].as_array().unwrap().is_empty());
        assert!(s.largest_files(usize::MAX, 0).is_empty());
        for bad in [
            json!({"limit": 0}),
            json!({"limit": 10_001}),
            json!({"limit": -1}),
            json!({"offset": 10_001}),
            json!({"offset": u64::MAX}),
            json!({"min_size": -1}),
            json!({"min_size": "1"}),
            json!({"x": 1}),
        ] {
            assert_eq!(s.execute("list.largest_files", &bad).unwrap_err().code(), "invalid_params", "{bad}");
        }
        assert!(s.execute("list.largest_files", &json!({"limit": 10_000, "offset": 10_000})).is_ok());
    }

    #[test]
    fn drives_come_from_platform_and_provider_overrides_and_is_sanitized() {
        use dirustat_platform::{FakePlatform, PlatformError};
        let mut s = Session::new();
        let fake = std::sync::Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        let out = s.execute("list.drives", &json!({})).unwrap();
        assert_eq!(out["drives"][0]["path"], "Z:\\");
        assert_eq!(out["drives"][0]["total"], 100);
        fake.set_failure(Some(PlatformError::Unsupported("drives")));
        assert_eq!(s.execute("list.drives", &json!({})).unwrap(), json!({"drives": []}));
        s.set_drives_provider(Box::new(|| {
            (0..1000)
                .map(|i| DriveInfo { path: format!("{i}:\\"), label: String::new(), total: 10, free: 99 })
                .collect()
        }));
        let drives = s.drives();
        assert_eq!(drives.len(), MAX_DRIVES);
        assert!(drives.iter().all(|d| d.free <= d.total));
        let out = s.execute("list.drives", &json!({})).unwrap();
        assert_eq!(out["drives"][0]["free"], 10);
        assert_eq!(s.execute("list.drives", &json!({"x": 1})).unwrap_err().code(), "invalid_params");
    }

    #[test]
    fn panicking_provider_is_contained() {
        let mut s = Session::new();
        s.set_drives_provider(Box::new(|| std::panic::panic_any("provider failed")));
        let err = s.execute("list.drives", &json!({})).unwrap_err();
        assert_eq!(err.code(), "internal");
        assert!(s.execute("session.state", &json!({})).is_ok());
    }
}
