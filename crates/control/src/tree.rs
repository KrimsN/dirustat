//! `session.tree`: a bounded, paginated view of a subtree. Never serializes the whole tree.

use crate::error::ControlError;
use dirustat_model::{Node, NodeId, NodeKind, SizeMode, Tree};
use serde::Deserialize;
use serde_json::{Value, json};

/// Deepest expansion a caller can ask for (bounds the recursion below).
pub const MAX_DEPTH: u32 = 8;
/// Most descendant nodes returned by one call.
pub const MAX_LIMIT: usize = 1000;
/// Most nodes one call may examine (children collected for sorting, children counted). The
/// session mutex is held meanwhile, so this bounds how long a huge directory can stall other
/// callers; past it the answer is marked `truncated` and `visit_capped`.
pub const MAX_VISITED: usize = 50_000;
const DEFAULT_DEPTH: u32 = 1;
const DEFAULT_LIMIT: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    node: Option<u64>,
    depth: Option<u64>,
    limit: Option<u64>,
    offset: Option<u64>,
    mode: Option<String>,
}

/// Runs `session.tree` against `tree`. Children are sorted by size, largest first; `offset` and
/// `limit` page through the children of `node`; `depth` and `limit` are capped.
pub fn session_tree(tree: &Tree, params: &Value) -> Result<Value, ControlError> {
    let empty = Value::Object(serde_json::Map::new());
    let params = if params.is_null() { &empty } else { params };
    let p = Params::deserialize(params).map_err(|e| ControlError::invalid_params(e.to_string()))?;
    let mode = match p.mode.as_deref() {
        None | Some("allocated") => SizeMode::Allocated,
        Some("logical") => SizeMode::Logical,
        Some(other) => return Err(ControlError::invalid_params(format!("unknown mode: {other}"))),
    };
    let depth = clamp_u32(p.depth, DEFAULT_DEPTH, MAX_DEPTH);
    let limit = clamp_usize(p.limit, DEFAULT_LIMIT, MAX_LIMIT);
    let offset = usize::try_from(p.offset.unwrap_or(0)).unwrap_or(usize::MAX);
    let id = match p.node {
        None => tree.root(),
        Some(index) => {
            find_node(tree, index).ok_or_else(|| ControlError::invalid_params(format!("no node with id {index}")))?
        }
    };
    let mut ctx = Ctx { tree, mode, budget: limit, visit: MAX_VISITED, truncated: false, visit_capped: false };
    let mut node = ctx.render(id, depth, offset);
    // A lower bound when the visit budget ran out (`visit_capped`).
    let total_children = tree.children(id).take(MAX_VISITED).count();
    if total_children >= MAX_VISITED {
        ctx.visit_capped = true;
        ctx.truncated = true;
    }
    if let Some(obj) = node.as_object_mut() {
        obj.insert("total_children".to_owned(), json!(total_children));
    }
    Ok(json!({
        "revision_nodes": tree.len(),
        "size_mode": if mode == SizeMode::Logical { "logical" } else { "allocated" },
        "offset": offset,
        "limit": limit,
        "depth": depth,
        "truncated": ctx.truncated,
        "visit_capped": ctx.visit_capped,
        "node": node,
    }))
}

fn clamp_u32(v: Option<u64>, default: u32, max: u32) -> u32 {
    v.map_or(default, |v| u32::try_from(v).unwrap_or(max).min(max))
}

fn clamp_usize(v: Option<u64>, default: usize, max: usize) -> usize {
    v.map_or(default, |v| usize::try_from(v).unwrap_or(max).min(max))
}

/// Resolves an arena index to a node id in O(1).
pub fn find_node(tree: &Tree, index: u64) -> Option<NodeId> {
    tree.node_at(usize::try_from(index).ok()?)
}

struct Ctx<'a> {
    tree: &'a Tree,
    mode: SizeMode,
    /// Descendant nodes still allowed in the output.
    budget: usize,
    /// Nodes still allowed to be examined.
    visit: usize,
    truncated: bool,
    visit_capped: bool,
}

impl Ctx<'_> {
    /// Recursion is bounded by `MAX_DEPTH`.
    fn render(&mut self, id: NodeId, depth: u32, skip: usize) -> Value {
        let Some(node) = self.tree.get(id) else {
            return Value::Null;
        };
        let mut out = describe(self.tree, id, node, self.mode);
        if depth == 0 {
            return out;
        }
        let mut kids: Vec<(NodeId, u64)> = Vec::new();
        for c in self.tree.children(id) {
            if self.visit == 0 {
                self.truncated = true;
                self.visit_capped = true;
                break;
            }
            self.visit = self.visit.saturating_sub(1);
            kids.push((c, self.tree.get(c).map_or(0, |n| n.total_size(self.mode))));
        }
        kids.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut rendered = Vec::new();
        let mut shown = 0_usize;
        for (child, _) in kids.iter().skip(skip) {
            if self.budget == 0 {
                self.truncated = true;
                break;
            }
            self.budget = self.budget.saturating_sub(1);
            shown = shown.saturating_add(1);
            rendered.push(self.render(*child, depth.saturating_sub(1), 0));
        }
        if skip.saturating_add(shown) < kids.len() {
            self.truncated = true;
        }
        if let Some(obj) = out.as_object_mut() {
            obj.insert("children".to_owned(), Value::Array(rendered));
        }
        out
    }
}

fn describe(tree: &Tree, id: NodeId, node: &Node, mode: SizeMode) -> Value {
    let kind = match node.kind() {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Link => "link",
    };
    json!({
        "id": id.index(),
        "name": node.name(),
        "kind": kind,
        "size": node.total_size(mode),
        "logical": node.total_size(SizeMode::Logical),
        "allocated": node.total_size(SizeMode::Allocated),
        "files": node.file_count(),
        "dirs": node.dir_count(),
        "errors": node.error_count(),
        "parent": node.parent().map(NodeId::index),
        "has_children": tree.children(id).next().is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tree {
        let mut t = Tree::new("root");
        let root = t.root();
        let d = t.add_child(root, "dir", NodeKind::Dir, 0).unwrap();
        for i in 0..5_u64 {
            t.add_child(d, &format!("f{i}"), NodeKind::File, (i + 1) * 10).unwrap();
        }
        t.add_child(root, "big", NodeKind::File, 1000).unwrap();
        t.aggregate();
        t
    }

    #[test]
    fn root_page_sorted_by_size() {
        let v = session_tree(&sample(), &json!({})).unwrap();
        let kids = v["node"]["children"].as_array().unwrap();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0]["name"], "big");
        assert_eq!(v["node"]["total_children"], 2);
        assert_eq!(v["truncated"], false);
        assert!(kids[1]["children"].is_null());
    }

    #[test]
    fn pagination_and_truncation() {
        let t = sample();
        let dir = find_node(&t, 1).unwrap().index();
        let v = session_tree(&t, &json!({"node": dir, "limit": 2})).unwrap();
        assert_eq!(v["node"]["children"].as_array().unwrap().len(), 2);
        assert_eq!(v["truncated"], true);
        let v = session_tree(&t, &json!({"node": dir, "limit": 2, "offset": 4})).unwrap();
        let kids = v["node"]["children"].as_array().unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0]["name"], "f0");
        assert_eq!(v["truncated"], false);
    }

    #[test]
    fn depth_expands_and_is_capped() {
        let t = sample();
        let v = session_tree(&t, &json!({"depth": 2})).unwrap();
        let dir = v["node"]["children"].as_array().unwrap().iter().find(|c| c["name"] == "dir").unwrap().clone();
        assert_eq!(dir["children"].as_array().unwrap().len(), 5);
        let v = session_tree(&t, &json!({"depth": u64::MAX, "limit": u64::MAX})).unwrap();
        assert_eq!(v["depth"], MAX_DEPTH);
        assert_eq!(v["limit"], MAX_LIMIT);
    }

    #[test]
    fn huge_directories_are_visited_with_a_budget() {
        let mut t = Tree::new("root");
        let root = t.root();
        for i in 0..(MAX_VISITED + 10) {
            t.add_child(root, &format!("f{i}"), NodeKind::File, 1).unwrap();
        }
        t.aggregate();
        let v = session_tree(&t, &json!({"depth": 1, "limit": 5})).unwrap();
        assert_eq!(v["visit_capped"], true);
        assert_eq!(v["truncated"], true);
        assert_eq!(v["node"]["total_children"], MAX_VISITED);
        assert_eq!(v["node"]["children"].as_array().unwrap().len(), 5);
        let small = session_tree(&sample(), &json!({})).unwrap();
        assert_eq!(small["visit_capped"], false);
    }

    #[test]
    fn bad_params_are_errors() {
        let t = sample();
        for p in [
            json!({"node": 999}),
            json!({"node": -1}),
            json!({"depth": "x"}),
            json!({"mode": "weird"}),
            json!({"bogus": 1}),
            json!([1]),
            json!("s"),
        ] {
            assert_eq!(session_tree(&t, &p).unwrap_err().code(), "invalid_params", "{p}");
        }
        assert!(session_tree(&t, &Value::Null).is_ok());
    }

    #[test]
    fn logical_mode_and_empty_tree() {
        let v = session_tree(&Tree::new("r"), &json!({"mode": "logical"})).unwrap();
        assert_eq!(v["size_mode"], "logical");
        assert_eq!(v["node"]["children"], json!([]));
    }
}
