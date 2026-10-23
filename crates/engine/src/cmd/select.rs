//! `select.*` commands: selection by node id (arena index).

use super::{CommandSpec, always, no_params, parse_params};
use crate::{CommandError, Session};
use dirustat_model::{NodeId, Tree};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "select.node",
            label: "Select node",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": u64} arena index of the node to select (as reported by session.state).",
            destructive: false,
            enabled: always,
            run: run_node,
        },
        CommandSpec {
            id: "select.clear",
            label: "Clear selection",
            menu: "Edit/Clear selection",
            shortcut: Some("Escape"),
            params_doc: "none",
            destructive: false,
            enabled: has_selection,
            run: run_clear,
        },
    ]
}

fn has_selection(session: &Session) -> bool {
    session.selection().is_some()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeParams {
    id: u64,
}

/// Resolves an arena index reported by the engine back to a node id in O(1).
pub(super) fn node_by_index(tree: &Tree, index: u64) -> Option<NodeId> {
    tree.node_at(usize::try_from(index).ok()?)
}

fn run_node(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: NodeParams = parse_params(params)?;
    let no_such = || CommandError::InvalidParams(format!("no node with id {}", p.id));
    let node = node_by_index(session.tree(), p.id).ok_or_else(no_such)?;
    if !session.select_node(node) {
        return Err(no_such());
    }
    if session.is_scanning() {
        // The id belongs to the tree that is current now; when the scan finishes the selection
        // follows its node by path and is dropped (see `selection_lost`) if the node is gone.
        return Ok(json!({
            "selected": p.id,
            "scan_pending": true,
            "note": "a scan is running: the selection is carried to the new tree by path when it finishes, \
                     or dropped if the node does not exist there (see session.state selection_lost)",
        }));
    }
    Ok(json!({ "selected": p.id }))
}

fn run_clear(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    session.clear_selection();
    Ok(json!({ "selected": null }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("a.txt"), b"hello").unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        (dir, s)
    }

    #[test]
    fn select_and_clear() {
        let (_dir, mut s) = scanned();
        assert!(s.tree().len() >= 3);
        let last = u64::try_from(s.tree().len() - 1).unwrap();
        assert_eq!(s.execute("select.node", &json!({"id": last})).unwrap(), json!({"selected": last}));
        assert_eq!(s.selection().map(NodeId::index), usize::try_from(last).ok());
        assert_eq!(s.state_json()["selection"], last);
        s.execute("select.clear", &json!({})).unwrap();
        assert!(s.selection().is_none());
    }

    #[test]
    fn invalid_ids_are_rejected_and_selection_kept() {
        let (_dir, mut s) = scanned();
        s.execute("select.node", &json!({"id": 1})).unwrap();
        let beyond = u64::try_from(s.tree().len()).unwrap();
        for bad in [json!({"id": beyond}), json!({"id": u64::MAX}), json!({"id": -1}), json!({"id": "1"}), json!({})] {
            let err = s.execute("select.node", &bad).unwrap_err();
            assert_eq!(err.code(), "invalid_params", "{bad}");
        }
        assert_eq!(s.selection().map(NodeId::index), Some(1));
    }

    fn id_named(s: &Session, name: &str) -> u64 {
        let tree = s.tree();
        let node = (0..tree.len())
            .filter_map(|i| tree.node_at(i))
            .find(|id| tree.get(*id).is_some_and(|n| n.name() == name))
            .unwrap();
        u64::try_from(node.index()).unwrap()
    }

    fn rescan(s: &mut Session, dir: &std::path::Path) {
        s.execute("scan.start", &json!({"path": dir.to_string_lossy()})).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while s.execute("session.poll", &json!({})).unwrap()["scanning"] == true {
            assert!(std::time::Instant::now() < deadline, "scan did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Regression for QA-113 (b): a selection made before a scan finishes was cleared with the tree.
    #[test]
    fn selection_survives_a_rescan_when_the_node_still_exists() {
        let (dir, mut s) = scanned();
        // Different file layout so the arena index of the node changes in the new tree.
        std::fs::write(dir.path().join("aaa.txt"), b"x").unwrap();
        let before = id_named(&s, "a.txt");
        s.execute("select.node", &json!({"id": before})).unwrap();
        s.execute("scan.start", &json!({"path": dir.path().to_string_lossy()})).unwrap();
        // Issued while the scan is pending: reported as such.
        let out = s.execute("select.node", &json!({"id": before})).unwrap();
        assert_eq!(out["scan_pending"], true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while s.execute("session.poll", &json!({})).unwrap()["scanning"] == true {
            assert!(std::time::Instant::now() < deadline, "scan did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let state = s.state_json();
        assert_eq!(state["selection_name"], "a.txt", "{state}");
        assert_eq!(state["selection"], id_named(&s, "a.txt"));
        assert!(state["selection_lost"].is_null());
        // The restored node is the one under `sub`, not another file of the same name elsewhere.
        let sel = s.selection().unwrap();
        let parent = s.tree().get(sel).and_then(|n| n.parent()).and_then(|p| s.tree().get(p)).map(|n| n.name());
        assert_eq!(parent, Some("sub"));
    }

    #[test]
    fn selection_that_is_gone_after_a_rescan_is_reported() {
        let (dir, mut s) = scanned();
        let id = id_named(&s, "a.txt");
        s.execute("select.node", &json!({"id": id})).unwrap();
        std::fs::remove_file(dir.path().join("sub").join("a.txt")).unwrap();
        rescan(&mut s, dir.path());
        assert!(s.selection().is_none());
        let state = s.state_json();
        assert!(state["selection_lost"].as_str().unwrap().ends_with("a.txt"), "{state}");
        // The next selection change clears the report.
        s.execute("select.node", &json!({"id": 0})).unwrap();
        assert!(s.state_json()["selection_lost"].is_null());
    }

    #[test]
    fn selecting_by_click_clears_the_lost_selection_report() {
        let (dir, mut s) = scanned();
        s.execute("select.node", &json!({"id": id_named(&s, "a.txt")})).unwrap();
        std::fs::remove_file(dir.path().join("sub").join("a.txt")).unwrap();
        rescan(&mut s, dir.path());
        assert!(s.state_json()["selection_lost"].is_string());
        s.map_layout(dirustat_treemap::Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 });
        assert!(s.select_at(1.0, 1.0).is_some());
        assert!(s.state_json()["selection_lost"].is_null());
    }

    #[test]
    fn selection_does_not_follow_into_a_different_root() {
        let (_dir, mut s) = scanned();
        s.execute("select.node", &json!({"id": id_named(&s, "a.txt")})).unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("sub")).unwrap();
        std::fs::write(other.path().join("sub").join("a.txt"), b"x").unwrap();
        rescan(&mut s, other.path());
        assert!(s.selection().is_none());
        assert!(s.state_json()["selection_lost"].is_string());
    }

    #[test]
    fn clear_is_not_enabled_without_selection() {
        let mut s = Session::new();
        let err = s.execute("select.clear", &json!({})).unwrap_err();
        assert_eq!(err.code(), "not_enabled");
    }
}
