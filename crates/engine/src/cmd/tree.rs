//! `tree.*` commands: paged children listing and path reconstruction. Everything is iterative and
//! bounded; node ids are arena indices as reported by `session.state` and `select.node`.

use super::search::listing_filter;
use super::select::node_by_index;
use super::{CommandSpec, always, parse_params};
use crate::search::Budget;
use crate::{CommandError, Session};
use dirustat_model::{NodeId, NodeKind, Tree};
use serde::Deserialize;
use serde_json::{Value, json};

/// Largest page a listing command returns.
pub(super) const MAX_PAGE: usize = 10_000;
const DEFAULT_PAGE: usize = 100;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "tree.children",
            label: "List children",
            menu: "",
            shortcut: None,
            params_doc: "{\"node\"?: u64 (default: root), \"limit\"?: 1..=10000 (default 100), \"offset\"?: u64 \
                (default 0), \"sort\"?: \"size\" (default, descending) | \"name\"} children of a node by the \
                session size mode. Optional filters: \"extensions\"?: [string] (directories then never match), \"min_size\"?: u64, \"max_size\"?: u64, \"name\"?: wildcard pattern (case-insensitive); total counts the filtered children. Returns {node, total, offset, revision, budget_exhausted, items: [{id, name, kind, size, files, error}]}; ids are valid only for the returned revision (file.* commands need it with id); budget_exhausted: the 10 s filter budget ran out and the list is partial.",
            destructive: false,
            enabled: always,
            run: run_children,
        },
        CommandSpec {
            id: "tree.path",
            label: "Node path",
            menu: "",
            shortcut: None,
            params_doc: "{\"node\": u64} arena index. Returns {id, path} with the full path from the scan root.",
            destructive: false,
            enabled: always,
            run: run_path,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildrenParams {
    node: Option<u64>,
    limit: Option<u64>,
    offset: Option<u64>,
    sort: Option<SortName>,
    extensions: Option<Vec<String>>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    name: Option<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum SortName {
    Size,
    Name,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathParams {
    node: u64,
}

/// Validates a page `limit` (1..=[`MAX_PAGE`], default when absent).
pub(super) fn page_limit(limit: Option<u64>) -> Result<usize, CommandError> {
    let Some(limit) = limit else { return Ok(DEFAULT_PAGE) };
    match usize::try_from(limit) {
        Ok(n) if (1..=MAX_PAGE).contains(&n) => Ok(n),
        _ => Err(CommandError::InvalidParams(format!("limit must be 1..={MAX_PAGE}"))),
    }
}

fn resolve(session: &Session, index: u64) -> Result<NodeId, CommandError> {
    node_by_index(session.tree(), index).ok_or_else(|| CommandError::InvalidParams(format!("no node with id {index}")))
}

pub(super) fn kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Link => "link",
    }
}

fn run_children(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: ChildrenParams = parse_params(params)?;
    let limit = page_limit(p.limit)?;
    let offset = usize::try_from(p.offset.unwrap_or(0)).unwrap_or(usize::MAX);
    let parent = match p.node {
        Some(index) => resolve(session, index)?,
        None => session.tree().root(),
    };
    let mode = session.map_size_mode();
    let tree = session.tree();
    let filter = listing_filter(p.extensions.as_deref(), p.min_size, p.max_size, p.name.as_deref())?;
    let mut budget = Budget::new();
    let mut rows: Vec<(NodeId, u64, &str)> = Vec::new();
    for id in tree.children(parent) {
        if budget.tick() {
            break;
        }
        let Some(n) = tree.get(id) else { continue };
        let size = n.total_size(mode);
        if filter.matches(n.name(), n.kind(), size, &mut || None) {
            rows.push((id, size, n.name()));
        }
    }
    match p.sort.unwrap_or(SortName::Size) {
        SortName::Size => rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(b.2)).then_with(|| a.0.cmp(&b.0))),
        SortName::Name => rows.sort_by(|a, b| a.2.cmp(b.2).then_with(|| a.0.cmp(&b.0))),
    }
    let total = rows.len();
    let items: Vec<Value> = rows
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter_map(|(id, size, name)| {
            let n = tree.get(id)?;
            Some(json!({
                "id": id.index(),
                "name": name,
                "kind": kind_name(n.kind()),
                "size": size,
                "files": n.file_count(),
                "error": n.error_count() > 0,
            }))
        })
        .collect();
    Ok(json!({
        "node": parent.index(),
        "total": total,
        "offset": offset,
        "revision": session.revision(),
        "budget_exhausted": budget.exhausted(),
        "items": items,
    }))
}

fn run_path(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: PathParams = parse_params(params)?;
    let node = resolve(session, p.node)?;
    let path = session.node_path(node).ok_or_else(|| CommandError::InvalidParams("node has no path".to_owned()))?;
    Ok(json!({ "id": p.node, "path": path }))
}

impl Session {
    /// Full path of `node`: the root name (the scanned path) followed by the names down to the
    /// node. Walks up the parent chain with a step bound of the tree size; `None` for an unknown
    /// node or a broken parent chain.
    pub fn node_path(&self, node: NodeId) -> Option<String> {
        path_in(self.tree(), node)
    }
}

/// [`Session::node_path`] for any tree (background jobs hold their own `Arc<Tree>`).
pub(crate) fn path_in(tree: &Tree, node: NodeId) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    let mut cur = node;
    for _ in 0..=tree.len() {
        let n = tree.get(cur)?;
        names.push(n.name());
        match n.parent() {
            Some(parent) => cur = parent,
            None => return Some(join_path(names.into_iter().rev())),
        }
    }
    None
}

fn join_path<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let sep = if cfg!(windows) { '\\' } else { '/' };
    let mut out = String::new();
    for part in parts {
        if out.is_empty() {
            out.push_str(part);
        } else {
            if !out.ends_with(['/', '\\']) {
                out.push(sep);
            }
            out.push_str(part);
        }
    }
    // Windows names never contain `/`, so any left over came from the root spelling.
    if cfg!(windows) { out.replace('/', "\\") } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::Tree;

    /// Session over a synthetic tree built through the public scan API on a temp dir.
    pub(super) fn scanned() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("a.txt"), vec![1u8; 10]).unwrap();
        std::fs::write(dir.path().join("big.bin"), vec![1u8; 5000]).unwrap();
        std::fs::write(dir.path().join("small.md"), b"x").unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        (dir, s)
    }

    fn id_of(s: &Session, name: &str) -> u64 {
        (0..s.tree().len())
            .find_map(|i| {
                let id = u64::try_from(i).ok()?;
                let node = node_by_index(s.tree(), id)?;
                (s.tree().get(node)?.name() == name).then_some(id)
            })
            .unwrap()
    }

    #[test]
    fn children_accept_filters() {
        let (_d, mut s) = scanned();
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        let names = |v: &Value| -> Vec<String> {
            v["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_owned()).collect()
        };
        let out = s.execute("tree.children", &json!({"extensions": ["bin", "MD"]})).unwrap();
        assert_eq!(names(&out), ["big.bin", "small.md"]);
        assert_eq!(out["total"], 2);
        let out = s.execute("tree.children", &json!({"min_size": 100})).unwrap();
        assert_eq!(names(&out), ["big.bin"]);
        let out = s.execute("tree.children", &json!({"name": "S*"})).unwrap();
        assert_eq!(names(&out), ["sub", "small.md"]);
        let bad = json!({"min_size": 5, "max_size": 1});
        assert_eq!(s.execute("tree.children", &bad).unwrap_err().code(), "invalid_params");
    }

    #[test]
    fn children_are_sorted_and_paged() {
        let (_d, mut s) = scanned();
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        let out = s.execute("tree.children", &json!({})).unwrap();
        assert_eq!(out["total"], 3);
        let names: Vec<_> = out["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["big.bin", "sub", "small.md"]);
        assert_eq!(out["items"][0]["size"], 5000);
        assert_eq!(out["items"][0]["kind"], "file");
        let page = s.execute("tree.children", &json!({"limit": 1, "offset": 1})).unwrap();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["name"], "sub");
        let by_name = s.execute("tree.children", &json!({"sort": "name"})).unwrap();
        assert_eq!(by_name["items"][0]["name"], "big.bin");
        let past = s.execute("tree.children", &json!({"offset": u64::MAX})).unwrap();
        assert!(past["items"].as_array().unwrap().is_empty());
        let sub = id_of(&s, "sub");
        let in_sub = s.execute("tree.children", &json!({"node": sub})).unwrap();
        assert_eq!(in_sub["items"][0]["name"], "a.txt");
    }

    #[test]
    fn path_reconstruction() {
        let (dir, mut s) = scanned();
        let a = id_of(&s, "a.txt");
        let out = s.execute("tree.path", &json!({"node": a})).unwrap();
        let path = out["path"].as_str().unwrap();
        assert!(path.ends_with("a.txt"), "{path}");
        assert!(std::path::Path::new(path).starts_with(dir.path()), "{path}");
        assert!(std::fs::metadata(path).is_ok(), "{path}");
        let root = s.execute("tree.path", &json!({"node": 0})).unwrap();
        assert!(!root["path"].as_str().unwrap().is_empty());
    }

    #[test]
    fn join_handles_trailing_separators_and_empty_root() {
        assert_eq!(join_path(["", "a"].into_iter()), "a");
        if cfg!(windows) {
            assert_eq!(join_path(["E:/a/b", "f"].into_iter()), "E:\\a\\b\\f");
        }
        assert_eq!(join_path(["C:\\", "a"].into_iter()), "C:\\a");
        assert_eq!(join_path(["/", "a", "b"].into_iter()).replace('\\', "/"), "/a/b");
    }

    #[test]
    fn invalid_params_and_empty_tree() {
        let mut s = Session::new();
        let out = s.execute("tree.children", &json!({})).unwrap();
        assert_eq!(out["total"], 0);
        assert!(out["items"].as_array().unwrap().is_empty());
        assert_eq!(s.execute("tree.path", &json!({"node": 0})).unwrap()["path"], "");
        for (id, bad) in [
            ("tree.children", json!({"limit": 0})),
            ("tree.children", json!({"limit": 10_001})),
            ("tree.children", json!({"limit": u64::MAX})),
            ("tree.children", json!({"limit": -1})),
            ("tree.children", json!({"node": 99})),
            ("tree.children", json!({"offset": -1})),
            ("tree.children", json!({"sort": "bogus"})),
            ("tree.children", json!({"x": 1})),
            ("tree.path", json!({})),
            ("tree.path", json!({"node": 99})),
            ("tree.path", json!({"node": "0"})),
        ] {
            assert_eq!(s.execute(id, &bad).unwrap_err().code(), "invalid_params", "{id} {bad}");
        }
    }

    #[test]
    fn node_path_of_foreign_id_is_none() {
        let (_d, s) = scanned();
        let mut other = Tree::new("x");
        let mut last = other.root();
        for i in 0..50 {
            last = other.add_child(other.root(), &format!("n{i}"), NodeKind::File, 1).unwrap();
        }
        assert!(s.node_path(last).is_none());
    }
}
