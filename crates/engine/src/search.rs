//! Search and filtering over the session tree (G-016). Read-only and iterative.
//!
//! [`Filter`] is the reusable predicate (kind, size range, extensions, name matcher); [`search`]
//! walks the tree with it and keeps the largest matches. Name matching is one of three modes:
//! plain substring, hand-written wildcard (`dirustat_scan::Glob`, no backtracking blow-up) or a
//! regex from the linear-time `regex` engine with a compiled-size limit. Every limit here exists
//! because the query text is hostile input.

use crate::{CommandError, extension_of};
use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use dirustat_scan::Glob;
use regex::{Regex, RegexBuilder};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

/// Hard cap on the hits one search returns.
pub const MAX_RESULTS: usize = 10_000;
/// Longest accepted name pattern, in characters (regex included).
pub const MAX_QUERY_CHARS: usize = 512;
/// Most extensions one filter lists.
pub const MAX_EXTENSIONS: usize = 64;
/// Compiled-size limit handed to the regex engine, in bytes.
const REGEX_SIZE_LIMIT: usize = 1 << 20;
/// Wall-clock budget of one search.
const TIME_BUDGET: Duration = Duration::from_secs(10);
/// The clock is read once per this many visited nodes.
const CLOCK_STRIDE: usize = 4096;

/// How the query text is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    Substring,
    Wildcard,
    Regex,
}

/// Which node kinds pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KindFilter {
    File,
    Dir,
    #[default]
    Any,
}

impl KindFilter {
    fn accepts(self, kind: NodeKind) -> bool {
        match self {
            Self::Any => true,
            Self::Dir => kind == NodeKind::Dir,
            Self::File => kind != NodeKind::Dir,
        }
    }
}

fn invalid(what: impl Into<String>) -> CommandError {
    CommandError::InvalidParams(what.into())
}

/// A compiled name matcher.
#[derive(Clone, Debug)]
pub enum NameMatcher {
    /// `needle` is already lowercased when `case_sensitive` is false.
    Substring {
        needle: String,
        case_sensitive: bool,
    },
    Wildcard(Glob),
    Regex(Regex),
}

impl NameMatcher {
    /// Compiles `query`; hostile or invalid input is `InvalidParams`.
    pub fn new(query: &str, mode: MatchMode, case_sensitive: bool) -> Result<Self, CommandError> {
        if query.chars().take(MAX_QUERY_CHARS.saturating_add(1)).count() > MAX_QUERY_CHARS {
            return Err(invalid(format!("query is longer than {MAX_QUERY_CHARS} characters")));
        }
        if query.contains('\0') {
            return Err(invalid("query contains a NUL character"));
        }
        match mode {
            MatchMode::Substring => {
                let needle = if case_sensitive { query.to_owned() } else { query.to_lowercase() };
                Ok(Self::Substring { needle, case_sensitive })
            }
            MatchMode::Wildcard => {
                Glob::new(query, !case_sensitive).map(Self::Wildcard).map_err(|e| invalid(format!("bad wildcard: {e}")))
            }
            MatchMode::Regex => RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_SIZE_LIMIT.saturating_mul(2))
                .nest_limit(64)
                .build()
                .map(Self::Regex)
                .map_err(|e| invalid(format!("bad regex: {}", e.to_string().chars().take(300).collect::<String>()))),
        }
    }

    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Self::Substring { needle, case_sensitive: true } => text.contains(needle.as_str()),
            Self::Substring { needle, case_sensitive: false } => text.to_lowercase().contains(needle.as_str()),
            Self::Wildcard(glob) => glob.is_match(text),
            Self::Regex(re) => re.is_match(text),
        }
    }
}

/// Reusable node predicate. The default filter accepts everything.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// Lowercase extensions without the dot; empty means any. Directories never match a
    /// non-empty list.
    extensions: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub kind: KindFilter,
    name: Option<NameMatcher>,
    /// Match the name pattern against the full path instead of the name.
    pub match_path: bool,
}

impl Filter {
    /// A filter with only a kind and a size range.
    pub fn sized(kind: KindFilter, min_size: Option<u64>, max_size: Option<u64>) -> Self {
        Self { kind, min_size, max_size, ..Self::default() }
    }

    /// Sets the extension list (leading dots and case are ignored; empty strings are an error).
    pub fn with_extensions(mut self, extensions: &[String]) -> Result<Self, CommandError> {
        if extensions.len() > MAX_EXTENSIONS {
            return Err(invalid(format!("at most {MAX_EXTENSIONS} extensions")));
        }
        let mut list = Vec::with_capacity(extensions.len());
        for e in extensions {
            let e = e.trim_start_matches('.').to_lowercase();
            if e.is_empty() || e.len() > 255 || e.contains(['\0', '/', '\\']) {
                return Err(invalid("extensions must be 1..=255 characters without separators"));
            }
            list.push(e);
        }
        self.extensions = list;
        Ok(self)
    }

    /// Sets the name matcher; an empty query clears it.
    pub fn with_name(mut self, query: &str, mode: MatchMode, case_sensitive: bool) -> Result<Self, CommandError> {
        self.name = if query.is_empty() { None } else { Some(NameMatcher::new(query, mode, case_sensitive)?) };
        Ok(self)
    }

    /// Validates the size range.
    pub fn check_range(&self) -> Result<(), CommandError> {
        match (self.min_size, self.max_size) {
            (Some(lo), Some(hi)) if lo > hi => Err(invalid("min_size is greater than max_size")),
            _ => Ok(()),
        }
    }

    /// Whether the filter has no name matcher, so `path_of` is never called.
    pub fn needs_path(&self) -> bool {
        self.match_path && self.name.is_some()
    }

    /// Tests one node. Cheap checks run first; `path_of` is called only when the name matcher is
    /// applied to paths.
    pub fn matches(&self, name: &str, kind: NodeKind, size: u64, path_of: &mut dyn FnMut() -> Option<String>) -> bool {
        if !self.kind.accepts(kind)
            || self.min_size.is_some_and(|m| size < m)
            || self.max_size.is_some_and(|m| size > m)
        {
            return false;
        }
        if !self.extensions.is_empty() {
            if kind == NodeKind::Dir {
                return false;
            }
            let ext = extension_of(name);
            if !self.extensions.contains(&ext) {
                return false;
            }
        }
        let Some(matcher) = &self.name else { return true };
        if self.match_path { path_of().is_some_and(|p| matcher.is_match(&p)) } else { matcher.is_match(name) }
    }
}

/// Size used for ranking and range checks: the subtree total for directories, own size otherwise.
pub fn node_size(tree: &Tree, id: NodeId, mode: SizeMode) -> u64 {
    tree.get(id).map_or(0, |n| if n.kind() == NodeKind::Dir { n.total_size(mode) } else { n.own_size(mode) })
}

/// Result of [`search`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchOutcome {
    /// The largest matches (size descending, ties by arena index), at most the requested limit.
    pub ids: Vec<NodeId>,
    /// Every match seen, including those cut by the limit.
    pub total_matched: usize,
    /// `total_matched` exceeds `ids.len()`.
    pub truncated: bool,
    /// The time budget ran out before the whole tree was visited; results are partial.
    pub budget_exhausted: bool,
}

/// Heap key: larger size first, then smaller arena index.
type Key = (u64, Reverse<NodeId>);

/// Walks the tree (iteratively, root excluded) and keeps the `limit` largest nodes passing `filter`.
pub fn search(
    tree: &Tree,
    filter: &Filter,
    mode: SizeMode,
    limit: usize,
    path_of: &dyn Fn(NodeId) -> Option<String>,
) -> SearchOutcome {
    search_with_deadline(tree, filter, mode, limit, path_of, Instant::now().checked_add(TIME_BUDGET))
}

fn search_with_deadline(
    tree: &Tree,
    filter: &Filter,
    mode: SizeMode,
    limit: usize,
    path_of: &dyn Fn(NodeId) -> Option<String>,
    deadline: Option<Instant>,
) -> SearchOutcome {
    let limit = limit.clamp(1, MAX_RESULTS);
    let mut heap: BinaryHeap<Reverse<Key>> = BinaryHeap::new();
    let (mut total, mut visited, mut exhausted) = (0usize, 0usize, false);
    let mut stack: Vec<NodeId> = tree.children(tree.root()).collect();
    while let Some(id) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited % CLOCK_STRIDE == 0 && deadline.is_some_and(|d| Instant::now() >= d) {
            exhausted = true;
            break;
        }
        let Some(node) = tree.get(id) else { continue };
        if node.kind() == NodeKind::Dir {
            stack.extend(tree.children(id));
        }
        let size = node_size(tree, id, mode);
        if !filter.matches(node.name(), node.kind(), size, &mut || path_of(id)) {
            continue;
        }
        total = total.saturating_add(1);
        let key: Key = (size, Reverse(id));
        if heap.len() < limit {
            heap.push(Reverse(key));
        } else if heap.peek().is_some_and(|Reverse(worst)| key > *worst) {
            heap.pop();
            heap.push(Reverse(key));
        }
    }
    let mut best: Vec<Key> = heap.into_iter().map(|Reverse(k)| k).collect();
    best.sort_by(|a, b| b.cmp(a));
    let ids: Vec<NodeId> = best.into_iter().map(|(_, Reverse(id))| id).collect();
    SearchOutcome { truncated: total > ids.len(), ids, total_matched: total, budget_exhausted: exhausted }
}

/// The last search result held by the session, valid only for the tree revision it was made on.
#[derive(Debug, Default)]
pub(crate) struct SearchState {
    pub(crate) revision: u64,
    pub(crate) ids: Vec<NodeId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        let mut t = Tree::new("root");
        let docs = t.add_child(t.root(), "Docs", NodeKind::Dir, 0).unwrap();
        t.add_child(docs, "Report.TXT", NodeKind::File, 500).unwrap();
        t.add_child(docs, "notes.md", NodeKind::File, 50).unwrap();
        t.add_child(t.root(), "big.bin", NodeKind::File, 9000).unwrap();
        t.add_child(t.root(), "small.bin", NodeKind::File, 10).unwrap();
        t.add_child(t.root(), "файл_日本語.txt", NodeKind::File, 7).unwrap();
        t.aggregate();
        t
    }

    fn names(t: &Tree, out: &SearchOutcome) -> Vec<String> {
        out.ids.iter().filter_map(|id| t.get(*id).map(|n| n.name().to_owned())).collect()
    }

    fn run(t: &Tree, f: &Filter, limit: usize) -> SearchOutcome {
        search(t, f, SizeMode::Logical, limit, &|_| None)
    }

    #[test]
    fn substring_modes_and_case() {
        let t = tree();
        let f = Filter::default().with_name("report", MatchMode::Substring, false).unwrap();
        assert_eq!(names(&t, &run(&t, &f, 10)), ["Report.TXT"]);
        let f = Filter::default().with_name("report", MatchMode::Substring, true).unwrap();
        assert!(run(&t, &f, 10).ids.is_empty());
        let f = Filter::default().with_name("ФАЙЛ", MatchMode::Substring, false).unwrap();
        assert_eq!(run(&t, &f, 10).total_matched, 1);
    }

    #[test]
    fn wildcard_and_regex_modes() {
        let t = tree();
        let f = Filter::default().with_name("*.bin", MatchMode::Wildcard, false).unwrap();
        assert_eq!(names(&t, &run(&t, &f, 10)), ["big.bin", "small.bin"]);
        let f = Filter::default().with_name(r"^(big|notes)\.", MatchMode::Regex, true).unwrap();
        assert_eq!(names(&t, &run(&t, &f, 10)), ["big.bin", "notes.md"]);
        let f = Filter::default().with_name(r"REPORT\.txt$", MatchMode::Regex, false).unwrap();
        assert_eq!(run(&t, &f, 10).total_matched, 1);
    }

    #[test]
    fn size_extension_and_kind_filters() {
        let t = tree();
        let f = Filter { min_size: Some(50), max_size: Some(600), ..Filter::default() };
        assert_eq!(names(&t, &run(&t, &f, 10)), ["Docs", "Report.TXT", "notes.md"]);
        let f = Filter { kind: KindFilter::Dir, ..Filter::default() };
        assert_eq!(names(&t, &run(&t, &f, 10)), ["Docs"]);
        let f = Filter { kind: KindFilter::File, ..Filter::default() }.with_extensions(&[".BIN".into()]).unwrap();
        assert_eq!(names(&t, &run(&t, &f, 10)), ["big.bin", "small.bin"]);
        let f = Filter::default().with_extensions(&["txt".into(), "md".into()]).unwrap();
        assert_eq!(run(&t, &f, 10).total_matched, 3);
        let f = Filter { min_size: Some(5), max_size: Some(4), ..Filter::default() };
        assert!(f.check_range().is_err());
    }

    #[test]
    fn match_path_uses_the_path_callback_only_when_needed() {
        let t = tree();
        let paths = |id: NodeId| t.get(id).map(|n| format!("root/{}", n.name()));
        let mut f = Filter::default().with_name("root/Docs/*", MatchMode::Wildcard, true).unwrap();
        f.match_path = true;
        assert!(f.needs_path());
        let out = search(&t, &f, SizeMode::Logical, 10, &|id| {
            let n = t.get(id)?;
            let parent = n.parent().and_then(|p| t.get(p)).map(|p| p.name().to_owned());
            Some(format!("root/{}/{}", parent.filter(|p| p != "root").unwrap_or_default(), n.name()))
        });
        assert_eq!(out.total_matched, 2);
        let f2 = Filter::default().with_name("bin", MatchMode::Substring, true).unwrap();
        assert!(!f2.needs_path());
        let calls = std::cell::Cell::new(0u32);
        let out = search(&t, &f2, SizeMode::Logical, 10, &|_| {
            calls.set(calls.get() + 1);
            None
        });
        assert_eq!(calls.get(), 0, "path must not be requested");
        assert_eq!(out.total_matched, 2);
        let mut f3 = Filter::default().with_name("root/big", MatchMode::Substring, true).unwrap();
        f3.match_path = true;
        assert_eq!(search(&t, &f3, SizeMode::Logical, 10, &paths).total_matched, 1);
    }

    #[test]
    fn limit_truncates_and_keeps_the_largest() {
        let t = tree();
        let out = run(&t, &Filter { kind: KindFilter::File, ..Filter::default() }, 2);
        assert_eq!(names(&t, &out), ["big.bin", "Report.TXT"]);
        assert_eq!((out.total_matched, out.truncated), (5, true));
        let out = run(&t, &Filter::default(), usize::MAX);
        assert!(!out.truncated && out.ids.len() == 6);
        assert_eq!(run(&t, &Filter::default(), 0).ids.len(), 1);
    }

    #[test]
    fn limit_is_capped_on_big_trees() {
        let mut t = Tree::new("r");
        for i in 0..(MAX_RESULTS + 500) {
            t.add_child(t.root(), &format!("f{i}"), NodeKind::File, 1).unwrap();
        }
        t.aggregate();
        let out = run(&t, &Filter::default(), usize::MAX);
        assert_eq!(out.ids.len(), MAX_RESULTS);
        assert_eq!(out.total_matched, MAX_RESULTS + 500);
        assert!(out.truncated);
    }

    #[test]
    fn hostile_queries_are_invalid_params_or_fast() {
        let bad = [
            ("(", MatchMode::Regex),
            ("[a-", MatchMode::Regex),
            ("a{1000}{1000}{1000}", MatchMode::Regex),
            ("(a{1000}){1000}", MatchMode::Regex),
            (r"\p{Foo}", MatchMode::Regex),
            ("a\0b", MatchMode::Regex),
            ("a\0b", MatchMode::Wildcard),
            ("a\0b", MatchMode::Substring),
        ];
        for (q, mode) in bad {
            let err = Filter::default().with_name(q, mode, false).unwrap_err();
            assert_eq!(err.code(), "invalid_params", "{q:?}");
        }
        let long = "a".repeat(MAX_QUERY_CHARS + 1);
        for mode in [MatchMode::Substring, MatchMode::Wildcard, MatchMode::Regex] {
            assert!(Filter::default().with_name(&long, mode, false).is_err());
        }
        let t = tree();
        let start = Instant::now();
        // Nested quantifiers are linear in the regex engine; star runs collapse in the glob.
        for (q, mode) in [
            ("(a*)*Q", MatchMode::Regex),
            ("(a+)+Q$", MatchMode::Regex),
            ("(x+x+)+Q", MatchMode::Regex),
            (&format!("{}b", "*".repeat(400)).replace("b", "Q"), MatchMode::Wildcard),
        ] {
            let f = Filter::default().with_name(q, mode, false).unwrap();
            assert_eq!(run(&t, &f, 10).total_matched, 0);
        }
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn extension_list_validation() {
        let too_many: Vec<String> = (0..=MAX_EXTENSIONS).map(|i| format!("e{i}")).collect();
        assert!(Filter::default().with_extensions(&too_many).is_err());
        for bad in ["", ".", "a/b", r"a\b", "a\0"] {
            assert!(Filter::default().with_extensions(&[bad.to_owned()]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn expired_deadline_reports_partial_results() {
        let mut t = Tree::new("r");
        for i in 0..(CLOCK_STRIDE * 2) {
            t.add_child(t.root(), &format!("f{i}"), NodeKind::File, 1).unwrap();
        }
        t.aggregate();
        let past = Instant::now().checked_sub(Duration::from_secs(1));
        let out = search_with_deadline(&t, &Filter::default(), SizeMode::Logical, 10, &|_| None, past);
        assert!(out.budget_exhausted);
        assert!(out.total_matched < CLOCK_STRIDE * 2);
    }

    #[test]
    fn empty_tree_has_no_hits() {
        let t = Tree::new("r");
        assert!(run(&t, &Filter::default(), 10).ids.is_empty());
    }
}
