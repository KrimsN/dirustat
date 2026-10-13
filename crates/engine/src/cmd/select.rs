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

/// Finds the node whose arena index is `index` with an iterative walk (ids cannot be forged).
pub(super) fn node_by_index(tree: &Tree, index: u64) -> Option<NodeId> {
    let index = usize::try_from(index).ok()?;
    if index >= tree.len() {
        return None;
    }
    let mut stack = vec![tree.root()];
    while let Some(node) = stack.pop() {
        if node.index() == index {
            return Some(node);
        }
        stack.extend(tree.children(node));
    }
    None
}

fn run_node(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: NodeParams = parse_params(params)?;
    let no_such = || CommandError::InvalidParams(format!("no node with id {}", p.id));
    let node = node_by_index(session.tree(), p.id).ok_or_else(no_such)?;
    if !session.select_node(node) {
        return Err(no_such());
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

    #[test]
    fn clear_is_not_enabled_without_selection() {
        let mut s = Session::new();
        let err = s.execute("select.clear", &json!({})).unwrap_err();
        assert_eq!(err.code(), "not_enabled");
    }
}
