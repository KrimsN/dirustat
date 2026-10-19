//! `search.*` commands: name/size/extension search over the current tree. The last result is held
//! by the session (keyed to the tree revision) so a shell can highlight it.

use super::tree::{kind_name, page_limit};
use super::{CommandSpec, always, no_params, parse_params};
use crate::search::{self, Filter, KindFilter, MatchMode, SearchOutcome};
use crate::{CommandError, Session, SizeModeName};
use dirustat_model::NodeId;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "search.find",
            label: "Find",
            menu: "",
            shortcut: None,
            params_doc: "{\"query\"?: string (<= 512 chars, empty = no name test), \"mode\"?: \"substring\" (default) | \
                \"wildcard\" (* and ?) | \"regex\" (linear-time engine, size-limited), \"case_sensitive\"?: bool \
                (default false), \"match_path\"?: bool (test the full path instead of the name; entries deeper than 2048 levels cannot be tested and never match; they are counted in skipped_deep), \"min_size\"?: u64, \
                \"max_size\"?: u64, \"extensions\"?: [string] (<= 64, no dot, directories never match), \"kind\"?: \
                \"file\" | \"dir\" | \"any\" (default), \"limit\"?: 1..=10000 (default 100), \"size_mode\"?: \
                \"logical\" | \"allocated\" (default: session mode)}. Read-only. Returns {total_matched, truncated, \
                budget_exhausted, skipped_deep (entries below level 2048 that match_path could not test; non-zero means the result may be incomplete), revision, items: [{id, name, kind, path, size}]} largest first; the ids are also \
                kept as the session's highlight set until search.clear or the next scan.",
            destructive: false,
            enabled: always,
            run: run_find,
        },
        CommandSpec {
            id: "search.clear",
            label: "Clear search",
            menu: "",
            shortcut: None,
            params_doc: "none. Drops the session's held search result; returns {cleared}: how many ids were held.",
            destructive: false,
            enabled: always,
            run: run_clear,
        },
    ]
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModeName {
    Substring,
    Wildcard,
    Regex,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum KindName {
    File,
    Dir,
    Any,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindParams {
    query: Option<String>,
    mode: Option<ModeName>,
    case_sensitive: Option<bool>,
    match_path: Option<bool>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    extensions: Option<Vec<String>>,
    kind: Option<KindName>,
    limit: Option<u64>,
    size_mode: Option<SizeModeName>,
}

fn build_filter(p: &FindParams) -> Result<Filter, CommandError> {
    let mode = match p.mode.as_ref().unwrap_or(&ModeName::Substring) {
        ModeName::Substring => MatchMode::Substring,
        ModeName::Wildcard => MatchMode::Wildcard,
        ModeName::Regex => MatchMode::Regex,
    };
    let kind = match p.kind.as_ref().unwrap_or(&KindName::Any) {
        KindName::File => KindFilter::File,
        KindName::Dir => KindFilter::Dir,
        KindName::Any => KindFilter::Any,
    };
    let mut filter = Filter::sized(kind, p.min_size, p.max_size)
        .with_extensions(p.extensions.as_deref().unwrap_or_default())?
        .with_name(p.query.as_deref().unwrap_or_default(), mode, p.case_sensitive.unwrap_or(false))?;
    filter.match_path = p.match_path.unwrap_or(false);
    filter.check_range()?;
    Ok(filter)
}

/// Filter for listing commands (`tree.children`, `list.largest_files`): extensions, size range and
/// a case-insensitive wildcard name pattern. Any part may be absent.
pub(super) fn listing_filter(
    extensions: Option<&[String]>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    name: Option<&str>,
) -> Result<Filter, CommandError> {
    let filter = Filter::sized(KindFilter::Any, min_size, max_size)
        .with_extensions(extensions.unwrap_or_default())?
        .with_name(name.unwrap_or_default(), MatchMode::Wildcard, false)?;
    filter.check_range()?;
    Ok(filter)
}

fn run_find(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: FindParams = parse_params(params)?;
    let limit = page_limit(p.limit)?;
    let filter = build_filter(&p)?;
    let mode = p.size_mode.map_or_else(|| session.map_size_mode(), SizeModeName::mode);
    let outcome = session.find(&filter, mode, limit);
    let tree = session.tree();
    let items: Vec<Value> = outcome
        .ids
        .iter()
        .filter_map(|&id| {
            let node = tree.get(id)?;
            Some(json!({
                "id": id.index(),
                "name": node.name(),
                "kind": kind_name(node.kind()),
                "path": session.node_path(id)?,
                "size": search::node_size(tree, id, mode),
            }))
        })
        .collect();
    Ok(json!({
        "total_matched": outcome.total_matched,
        "truncated": outcome.truncated,
        "budget_exhausted": outcome.budget_exhausted,
        "skipped_deep": outcome.skipped_deep,
        "revision": session.revision(),
        "items": items,
    }))
}

fn run_clear(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    let held = session.search_hits().len();
    session.clear_search();
    Ok(json!({ "cleared": held }))
}

impl Session {
    /// Runs `filter` over the tree, holds the hits for highlighting and returns the outcome.
    /// `limit` is clamped to [`search::MAX_RESULTS`].
    pub fn find(&mut self, filter: &Filter, mode: dirustat_model::SizeMode, limit: usize) -> SearchOutcome {
        let outcome = search::search(self.tree(), filter, mode, limit, &|id| {
            if filter.needs_path() { self.node_path(id) } else { None }
        });
        self.search.revision = self.revision();
        self.search.ids.clone_from(&outcome.ids);
        outcome
    }

    /// The held search hits; empty when none were made or the tree was replaced since.
    pub fn search_hits(&self) -> &[NodeId] {
        if self.search.revision == self.revision() { &self.search.ids } else { &[] }
    }

    /// Drops the held search hits.
    pub fn clear_search(&mut self) {
        self.search.ids.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::{NodeKind, Tree};

    fn session() -> Session {
        let mut t = Tree::new("root");
        let docs = t.add_child(t.root(), "Docs", NodeKind::Dir, 0).unwrap();
        t.add_child(docs, "Report.TXT", NodeKind::File, 500).unwrap();
        t.add_child(docs, "notes.md", NodeKind::File, 50).unwrap();
        t.add_child(t.root(), "big.bin", NodeKind::File, 9000).unwrap();
        t.add_child(t.root(), "small.bin", NodeKind::File, 10).unwrap();
        t.aggregate();
        let mut s = Session::new();
        s.replace_tree(t);
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        s
    }

    fn names(v: &Value) -> Vec<&str> {
        v["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect()
    }

    /// QA-146: `search.find match_path` reports how many entries were too deep to be tested.
    #[test]
    fn find_reports_entries_skipped_for_depth() {
        let mut t = Tree::new("r");
        let mut cur = t.root();
        let limit = usize::try_from(search::MAX_PATH_MATCH_DEPTH).unwrap();
        for _ in 0..limit + 3 {
            cur = t.add_child(cur, "d", NodeKind::Dir, 0).unwrap();
        }
        t.aggregate();
        let mut s = Session::new();
        s.replace_tree(t);
        let out = s.execute("search.find", &json!({"query": "d", "match_path": true, "limit": 1})).unwrap();
        assert_eq!(out["skipped_deep"], 3);
        assert_eq!(out["total_matched"], limit);
        let out = s.execute("search.find", &json!({"query": "d"})).unwrap();
        assert_eq!(out["skipped_deep"], 0);
        assert_eq!(out["total_matched"], limit + 3);
        let info = s.commands().into_iter().find(|c| c.id == "search.find").unwrap();
        assert!(info.params_doc.contains("skipped_deep"));
    }
    #[test]
    fn find_by_each_mode_sorted_by_size() {
        let mut s = session();
        let out = s.execute("search.find", &json!({"query": "bin"})).unwrap();
        assert_eq!(names(&out), ["big.bin", "small.bin"]);
        assert_eq!(out["items"][0]["size"], 9000);
        assert_eq!(out["items"][0]["kind"], "file");
        assert_eq!(out["items"][0]["path"], "root\\big.bin".replace('\\', std::path::MAIN_SEPARATOR_STR));
        let out = s.execute("search.find", &json!({"query": "*.TXT", "mode": "wildcard"})).unwrap();
        assert_eq!(names(&out), ["Report.TXT"]);
        let out = s.execute("search.find", &json!({"query": "^(report|notes)", "mode": "regex"})).unwrap();
        assert_eq!(names(&out), ["Report.TXT", "notes.md"]);
        let out = s.execute("search.find", &json!({"query": "report", "case_sensitive": true})).unwrap();
        assert_eq!(out["total_matched"], 0);
    }

    #[test]
    fn find_by_path_size_kind_and_extension() {
        let mut s = session();
        let sep = std::path::MAIN_SEPARATOR;
        let q = format!("Docs{sep}");
        let out = s.execute("search.find", &json!({"query": q, "match_path": true})).unwrap();
        assert_eq!(names(&out), ["Report.TXT", "notes.md"]);
        let out = s.execute("search.find", &json!({"min_size": 100, "max_size": 600})).unwrap();
        assert_eq!(names(&out), ["Docs", "Report.TXT"]);
        let out = s.execute("search.find", &json!({"kind": "dir"})).unwrap();
        assert_eq!(names(&out), ["Docs"]);
        let out = s.execute("search.find", &json!({"extensions": ["BIN"], "kind": "file", "limit": 1})).unwrap();
        assert_eq!(names(&out), ["big.bin"]);
        assert_eq!((out["total_matched"].as_u64(), out["truncated"].as_bool()), (Some(2), Some(true)));
    }

    #[test]
    fn hits_are_held_and_invalidated_by_a_new_tree() {
        let mut s = session();
        assert!(s.search_hits().is_empty());
        s.execute("search.find", &json!({"query": "bin"})).unwrap();
        assert_eq!(s.search_hits().len(), 2);
        assert_eq!(s.execute("search.clear", &json!({})).unwrap()["cleared"], 2);
        assert!(s.search_hits().is_empty());
        s.execute("search.find", &json!({"query": "bin"})).unwrap();
        s.replace_tree(Tree::new("other"));
        assert!(s.search_hits().is_empty(), "a stale revision hides the hits");
        assert_eq!(s.execute("search.clear", &json!({})).unwrap()["cleared"], 0);
    }

    #[test]
    fn invalid_params_are_rejected() {
        let mut s = session();
        let long = "x".repeat(600);
        let bad = [
            json!({"query": "("  , "mode": "regex"}),
            json!({"query": "(a{1000}){1000}", "mode": "regex"}),
            json!({"query": "a\u{0}"}),
            json!({"query": long}),
            json!({"mode": "fuzzy"}),
            json!({"kind": "link"}),
            json!({"limit": 0}),
            json!({"limit": 10_001}),
            json!({"limit": -1}),
            json!({"min_size": 9, "max_size": 1}),
            json!({"min_size": -1}),
            json!({"extensions": [""]}),
            json!({"extensions": "txt"}),
            json!({"size_mode": "huge"}),
            json!({"bogus": 1}),
        ];
        for p in bad {
            assert_eq!(s.execute("search.find", &p).unwrap_err().code(), "invalid_params", "{p}");
        }
        assert_eq!(s.execute("search.clear", &json!({"x": 1})).unwrap_err().code(), "invalid_params");
        assert!(s.execute("search.find", &json!({"limit": 10_000})).is_ok());
        assert!(s.execute("search.find", &json!(null)).is_ok());
    }

    #[test]
    fn works_on_a_scanned_tree_with_unicode_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("файл_日本語.txt"), b"abc").unwrap();
        std::fs::write(dir.path().join("plain.rs"), b"abcdef").unwrap();
        let mut s = Session::new();
        let h = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(h), Ok(true));
        let out = s.execute("search.find", &json!({"query": "ФАЙЛ", "size_mode": "logical"})).unwrap();
        assert_eq!(names(&out), ["файл_日本語.txt"]);
        assert_eq!(out["items"][0]["size"], 3);
    }
}
