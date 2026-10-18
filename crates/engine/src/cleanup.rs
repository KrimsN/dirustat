//! User-defined cleanups (G-017): definitions, validation and selector matching.
//!
//! A [`CleanupDef`] is only data: an id, a name, an [`CleanupAction`] and a [`Selector`] that picks
//! nodes of the current tree. Nothing here touches the disk (except the read-only
//! [`dir_is_empty_on_disk`] guard) and nothing runs by itself: the commands in `cmd::cleanup`
//! resolve a selector and hand every node to the same checked machinery as `file.delete`.
//!
//! Definitions are untrusted input (settings file, control channel): every list and string is
//! bounded, ids are slugs, patterns are compiled with the non-backtracking glob.

use crate::CommandError;
use crate::protected::key_of;
use crate::search::{Filter, KindFilter};
use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use dirustat_scan::Glob;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::path::Path;

/// Most cleanups one settings file holds (user-defined ones; built-ins are extra).
pub const MAX_CLEANUPS: usize = 64;
/// Longest id, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// Longest display name, in characters.
pub const MAX_NAME_CHARS: usize = 128;
/// Most name patterns in one selector.
pub const MAX_NAME_PATTERNS: usize = 64;
/// Longest single name pattern, in characters.
pub const MAX_PATTERN_CHARS: usize = 256;
/// Longest `only_under` path, in bytes.
pub const MAX_ONLY_UNDER_BYTES: usize = 4096;
/// Largest `older_than_days` (100 years).
pub const MAX_OLDER_THAN_DAYS: u32 = 36_500;
/// Most nodes one preview or run handles; more is reported as `truncated`.
pub const MAX_NODES_PER_RUN: usize = 10_000;
/// Disk entries inspected by the "is this folder still empty" guard.
const MAX_EMPTY_CHECK_ENTRIES: usize = 100_000;

/// What a cleanup does to every selected node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAction {
    /// Move the node to the recycle bin (reversible).
    Recycle,
    /// Delete the node without the recycle bin; needs `i_understand_permanent`.
    Permanent,
    /// Recycle everything inside each selected folder, keeping the folder itself.
    EmptyDirContents,
    /// Show the largest selected node in the file manager; changes nothing.
    Reveal,
}

/// Which kind of node a selector accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectorKind {
    File,
    Dir,
    #[default]
    Any,
}

/// The rule that picks nodes. All given criteria must hold (logical AND); `names` and
/// `extensions` are each an OR over their entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selector {
    /// Wildcard patterns (`*`, `?`) tested against the node name, case-insensitively.
    pub names: Vec<String>,
    /// Extensions without the dot; directories never match a non-empty list.
    pub extensions: Vec<String>,
    /// Minimum size in the session size mode (subtree total for folders).
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub kind: SelectorKind,
    /// Absolute path of a folder inside the scan root; only its descendants are considered.
    pub only_under: Option<String>,
    /// Only nodes whose modification time is known and older than this many days.
    pub older_than_days: Option<u32>,
    /// Only folders that contain no files and no unreadable parts (nested empty folders count).
    pub empty: bool,
}

/// One user-defined (or built-in) cleanup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupDef {
    /// Slug: `[a-z0-9_-]`, starts with a letter or digit.
    pub id: String,
    pub name: String,
    pub action: CleanupAction,
    #[serde(default)]
    pub selector: Selector,
}

/// Keys accepted in a definition object sent through `cleanup.save` (unknown keys are refused
/// there; the settings file is lenient so a newer version's data never breaks loading).
pub const DEF_KEYS: &[&str] = &["id", "name", "action", "selector"];
/// Keys accepted in a selector object sent through `cleanup.save`.
pub const SELECTOR_KEYS: &[&str] =
    &["names", "extensions", "min_size", "max_size", "kind", "only_under", "older_than_days", "empty"];

fn invalid(what: impl Into<String>) -> CommandError {
    CommandError::InvalidParams(what.into())
}

fn bounded_text(text: &str, max_chars: usize) -> bool {
    text.chars().take(max_chars.saturating_add(1)).count() <= max_chars
}

impl CleanupDef {
    /// Checks every field; a definition that passes can be compiled and stored.
    pub fn validate(&self) -> Result<(), CommandError> {
        let id_ok = !self.id.is_empty()
            && bounded_text(&self.id, MAX_ID_CHARS)
            && self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            && self.id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit());
        if !id_ok {
            return Err(invalid(format!("id must be a slug of 1..={MAX_ID_CHARS} characters [a-z0-9_-]")));
        }
        let name = self.name.trim();
        if name.is_empty() || !bounded_text(name, MAX_NAME_CHARS) || name.chars().any(char::is_control) {
            return Err(invalid(format!("name must be 1..={MAX_NAME_CHARS} characters without control characters")));
        }
        let s = &self.selector;
        if s.names.len() > MAX_NAME_PATTERNS {
            return Err(invalid(format!("at most {MAX_NAME_PATTERNS} name patterns")));
        }
        for p in &s.names {
            if !bounded_text(p, MAX_PATTERN_CHARS) || p.contains(['/', '\\']) {
                return Err(invalid(format!(
                    "name patterns are 1..={MAX_PATTERN_CHARS} characters without path separators"
                )));
            }
            Glob::new(p, true).map_err(|e| invalid(format!("bad name pattern: {e}")))?;
        }
        Filter::default().with_extensions(&s.extensions)?;
        Filter::sized(KindFilter::Any, s.min_size, s.max_size).check_range()?;
        if let Some(days) = s.older_than_days
            && !(1..=MAX_OLDER_THAN_DAYS).contains(&days)
        {
            return Err(invalid(format!("older_than_days must be 1..={MAX_OLDER_THAN_DAYS}")));
        }
        if let Some(under) = &s.only_under {
            check_only_under(under)?;
        }
        if s.names.is_empty()
            && s.extensions.is_empty()
            && s.min_size.is_none()
            && s.max_size.is_none()
            && s.older_than_days.is_none()
            && !s.empty
        {
            return Err(invalid(
                "the selector needs at least one criterion (names, extensions, min_size, max_size, older_than_days \
                 or empty); `kind` and `only_under` alone would select everything",
            ));
        }
        if !s.extensions.is_empty() && s.kind == SelectorKind::Dir {
            return Err(invalid("extensions never match folders"));
        }
        let folders_only = s.empty || self.action == CleanupAction::EmptyDirContents;
        if folders_only && s.kind != SelectorKind::Dir {
            return Err(invalid("`empty` and the empty_dir_contents action need kind \"dir\""));
        }
        Ok(())
    }

    /// Whether the action modifies the disk.
    pub fn is_destructive(&self) -> bool {
        self.action != CleanupAction::Reveal
    }
}

/// `only_under`: absolute, bounded, no NUL, no `.` or `..` component.
fn check_only_under(path: &str) -> Result<(), CommandError> {
    if path.len() > MAX_ONLY_UNDER_BYTES || path.contains('\0') {
        return Err(invalid(format!("only_under must be at most {MAX_ONLY_UNDER_BYTES} bytes without NUL")));
    }
    if path.split(['/', '\\']).any(|c| c == ".." || c == ".") {
        return Err(invalid("only_under must not contain `.` or `..` components"));
    }
    if key_of(path, cfg!(windows)).is_none() {
        return Err(invalid("only_under must be an absolute path"));
    }
    Ok(())
}

/// The built-in examples. They are definitions only: nothing runs until the user invokes
/// `cleanup.run`, and they cannot be edited or removed.
pub fn builtins() -> Vec<CleanupDef> {
    let def = |id: &str, name: &str, action, selector| CleanupDef {
        id: id.to_owned(),
        name: name.to_owned(),
        action,
        selector,
    };
    vec![
        def(
            "temporary-files",
            "Temporary files (*.tmp, *.temp)",
            CleanupAction::Recycle,
            Selector {
                names: vec!["*.tmp".to_owned(), "*.temp".to_owned()],
                kind: SelectorKind::File,
                ..Selector::default()
            },
        ),
        def(
            "empty-folders",
            "Empty folders",
            CleanupAction::Recycle,
            Selector { kind: SelectorKind::Dir, empty: true, ..Selector::default() },
        ),
        def(
            "build-output-folders",
            "Build output folders (target, node_modules)",
            CleanupAction::Recycle,
            Selector {
                names: vec!["target".to_owned(), "node_modules".to_owned()],
                kind: SelectorKind::Dir,
                ..Selector::default()
            },
        ),
    ]
}

/// Whether `id` names a built-in cleanup.
pub fn is_builtin(id: &str) -> bool {
    builtins().iter().any(|b| b.id == id)
}

/// Keeps the valid definitions with distinct, non-built-in ids, at most [`MAX_CLEANUPS`].
pub(crate) fn sanitized(list: Vec<CleanupDef>) -> Vec<CleanupDef> {
    let mut kept: Vec<CleanupDef> = Vec::new();
    for def in list {
        if kept.len() >= MAX_CLEANUPS {
            break;
        }
        if def.validate().is_ok() && !is_builtin(&def.id) && !kept.iter().any(|k| k.id == def.id) {
            kept.push(def);
        }
    }
    kept
}

/// Serde helper for `Settings::cleanups`: an entry that does not parse is dropped instead of
/// failing the whole settings file.
pub(crate) fn lenient_list<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<CleanupDef>, D::Error> {
    let raw = Vec::<Value>::deserialize(deserializer)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value::<CleanupDef>(v).ok()).collect())
}

/// A selector compiled for matching.
#[derive(Debug)]
pub struct Compiled {
    names: Vec<Glob>,
    filter: Filter,
    empty: bool,
    /// Nodes with an mtime below this match `older_than_days`.
    older_than: Option<i64>,
}

impl Compiled {
    /// Compiles a validated definition. `now` is the current time in epoch seconds.
    pub fn new(def: &CleanupDef, now: i64) -> Result<Self, CommandError> {
        def.validate()?;
        let s = &def.selector;
        let names = s
            .names
            .iter()
            .map(|p| Glob::new(p, true).map_err(|e| invalid(format!("bad name pattern: {e}"))))
            .collect::<Result<Vec<_>, _>>()?;
        let kind = match s.kind {
            SelectorKind::File => KindFilter::File,
            SelectorKind::Dir => KindFilter::Dir,
            SelectorKind::Any => KindFilter::Any,
        };
        let filter = Filter::sized(kind, s.min_size, s.max_size).with_extensions(&s.extensions)?;
        let older_than = s.older_than_days.map(|d| now.saturating_sub(i64::from(d).saturating_mul(86_400)));
        Ok(Self { names, filter, empty: s.empty, older_than })
    }

    fn matches(&self, tree: &Tree, id: NodeId, mode: SizeMode) -> bool {
        let Some(node) = tree.get(id) else { return false };
        let size = crate::search::node_size(tree, id, mode);
        if !self.filter.matches(node.name(), node.kind(), size, &mut || None) {
            return false;
        }
        if !self.names.is_empty() && !self.names.iter().any(|g| g.is_match(node.name())) {
            return false;
        }
        if self.empty && (node.kind() != NodeKind::Dir || node.file_count() != 0 || node.error_count() != 0) {
            return false;
        }
        match self.older_than {
            Some(limit) => node.data().mtime.is_some_and(|m| m < limit),
            None => true,
        }
    }
}

/// Result of [`select`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Matched nodes, largest first (ties by arena index). No node lies inside another one.
    pub nodes: Vec<NodeId>,
    /// More than the cap matched; the rest was not collected.
    pub truncated: bool,
}

/// Walks the descendants of `start` (never `start` itself, so never the scan root when started
/// there) and collects the nodes that match. A matched node is not descended into, which is the
/// ancestor de-duplication: a match inside a match would be deleted with it. Iterative.
pub fn select(tree: &Tree, start: NodeId, compiled: &Compiled, mode: SizeMode, cap: usize) -> Selection {
    let cap = cap.clamp(1, MAX_NODES_PER_RUN);
    let mut nodes: Vec<NodeId> = Vec::new();
    let mut truncated = false;
    let mut stack: Vec<NodeId> = tree.children(start).collect();
    while let Some(id) = stack.pop() {
        if compiled.matches(tree, id, mode) {
            if nodes.len() >= cap {
                truncated = true;
                break;
            }
            nodes.push(id);
        } else if tree.get(id).is_some_and(|n| n.kind() == NodeKind::Dir) {
            stack.extend(tree.children(id));
        }
    }
    nodes.sort_by(|a, b| {
        let (sa, sb) = (crate::search::node_size(tree, *a, mode), crate::search::node_size(tree, *b, mode));
        sb.cmp(&sa).then(a.cmp(b))
    });
    Selection { nodes, truncated }
}

/// Read-only guard: the folder at `path` contains, at any depth, only folders. Any file, link,
/// unreadable entry or an oversized listing counts as "not empty".
pub fn dir_is_empty_on_disk(path: &Path) -> bool {
    let mut stack = vec![path.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { return false };
        for entry in entries {
            seen = seen.saturating_add(1);
            let Ok(entry) = entry else { return false };
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { return false };
            if seen > MAX_EMPTY_CHECK_ENTRIES || !meta.is_dir() || crate::fsops::is_link(&meta) {
                return false;
            }
            stack.push(entry.path());
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(selector: Selector) -> CleanupDef {
        CleanupDef { id: "x".into(), name: "X".into(), action: CleanupAction::Recycle, selector }
    }

    fn names(patterns: &[&str]) -> Selector {
        Selector { names: patterns.iter().map(|s| (*s).to_owned()).collect(), ..Selector::default() }
    }

    #[test]
    fn builtins_are_valid_and_never_destructive_by_themselves() {
        for b in builtins() {
            assert!(b.validate().is_ok(), "{}", b.id);
        }
        assert_eq!(builtins().len(), 3);
        assert!(is_builtin("empty-folders") && !is_builtin("mine"));
    }

    #[test]
    fn hostile_definitions_are_refused() {
        let ok = def(names(&["*.tmp"]));
        assert!(ok.validate().is_ok());
        let mut bad = Vec::new();
        for id in ["", "A", "-x", "a b", "a/b", "a\0", "ü", &"a".repeat(MAX_ID_CHARS + 1)] {
            bad.push(CleanupDef { id: id.to_owned(), ..ok.clone() });
        }
        for name in ["", "   ", "a\0b", "a\nb", &"n".repeat(MAX_NAME_CHARS + 1)] {
            bad.push(CleanupDef { name: name.to_owned(), ..ok.clone() });
        }
        let many: Vec<&str> = vec!["a"; MAX_NAME_PATTERNS + 1];
        let long = "a".repeat(MAX_PATTERN_CHARS + 1);
        for sel in [
            names(&many),
            names(&[""]),
            names(&["a\0"]),
            names(&["a/b"]),
            names(&["a\\b"]),
            names(&[&long]),
            Selector::default(),
            Selector { kind: SelectorKind::Dir, ..Selector::default() },
            Selector { only_under: Some("/tmp".into()), ..Selector::default() },
            Selector { extensions: vec![String::new()], ..Selector::default() },
            Selector { extensions: vec!["a/b".into()], ..Selector::default() },
            Selector { min_size: Some(5), max_size: Some(4), ..names(&["a"]) },
            Selector { older_than_days: Some(0), ..names(&["a"]) },
            Selector { older_than_days: Some(MAX_OLDER_THAN_DAYS + 1), ..names(&["a"]) },
            Selector { extensions: vec!["tmp".into()], kind: SelectorKind::Dir, ..Selector::default() },
            Selector { empty: true, kind: SelectorKind::Any, ..Selector::default() },
            Selector { only_under: Some("relative/dir".into()), ..names(&["a"]) },
            Selector { only_under: Some(String::new()), ..names(&["a"]) },
            Selector { only_under: Some("/a/../b".into()), ..names(&["a"]) },
            Selector { only_under: Some("C:\\a\\..\\b".into()), ..names(&["a"]) },
            Selector { only_under: Some("/a\0".into()), ..names(&["a"]) },
            Selector { only_under: Some(format!("/{}", "a".repeat(MAX_ONLY_UNDER_BYTES))), ..names(&["a"]) },
        ] {
            bad.push(def(sel));
        }
        bad.push(CleanupDef { action: CleanupAction::EmptyDirContents, ..ok });
        for b in bad {
            assert_eq!(b.validate().unwrap_err().code(), "invalid_params", "{b:?}");
        }
    }

    #[test]
    fn sanitize_drops_invalid_duplicates_builtins_and_excess() {
        let good = |id: &str| CleanupDef { id: id.into(), ..def(names(&["*.tmp"])) };
        let mut list =
            vec![good("a"), good("a"), good("empty-folders"), CleanupDef { name: String::new(), ..good("b") }];
        list.extend((0..200).map(|i| good(&format!("c{i}"))));
        let kept = sanitized(list);
        assert_eq!(kept.len(), MAX_CLEANUPS);
        assert_eq!(kept.first().map(|d| d.id.as_str()), Some("a"));
        assert!(kept.iter().all(|d| d.id != "b" && d.id != "empty-folders"));
    }

    #[test]
    fn lenient_settings_list_skips_unparseable_entries() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "lenient_list")]
            list: Vec<CleanupDef>,
        }
        let h: Holder = serde_json::from_value(serde_json::json!({"list": [
            {"id": "a", "name": "A", "action": "recycle", "selector": {"names": ["*.x"], "future": 1}, "more": 2},
            {"id": "b", "name": "B", "action": "explode"},
            5,
            {"id": "c", "name": "C", "action": "reveal"}
        ]}))
        .unwrap();
        assert_eq!(h.list.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    }

    fn tree() -> Tree {
        let mut t = Tree::new("root");
        t.add_child(t.root(), "cache.TMP", NodeKind::File, 100).unwrap();
        let sub = t.add_child(t.root(), "sub", NodeKind::Dir, 0).unwrap();
        t.add_child(sub, "a.temp", NodeKind::File, 5).unwrap();
        t.add_child(sub, "keep.txt", NodeKind::File, 7).unwrap();
        let build = t.add_child(t.root(), "target", NodeKind::Dir, 0).unwrap();
        t.add_child(build, "inner.tmp", NodeKind::File, 50).unwrap();
        t.add_child(build, "target", NodeKind::Dir, 0).unwrap();
        let empty = t.add_child(t.root(), "empty", NodeKind::Dir, 0).unwrap();
        t.add_child(empty, "nested", NodeKind::Dir, 0).unwrap();
        t.aggregate();
        t
    }

    fn pick(t: &Tree, selector: Selector) -> Vec<String> {
        let c = Compiled::new(&def(selector), 0).unwrap();
        select(t, t.root(), &c, SizeMode::Logical, 100)
            .nodes
            .iter()
            .filter_map(|&id| t.get(id).map(|n| n.name().to_owned()))
            .collect()
    }

    #[test]
    fn selector_matches_wildcards_extensions_sizes_and_kinds() {
        let t = tree();
        assert_eq!(pick(&t, names(&["*.tmp", "*.temp"])), ["cache.TMP", "inner.tmp", "a.temp"]);
        assert_eq!(pick(&t, Selector { extensions: vec![".TEMP".into()], ..Selector::default() }), ["a.temp"]);
        assert_eq!(
            pick(&t, Selector { min_size: Some(6), kind: SelectorKind::File, ..Selector::default() }),
            ["cache.TMP", "inner.tmp", "keep.txt"]
        );
        assert_eq!(
            pick(&t, Selector { max_size: Some(5), kind: SelectorKind::File, ..Selector::default() }),
            ["a.temp"]
        );
        // The build folder matches; the nested `target` inside it is not reported separately.
        assert_eq!(pick(&t, Selector { kind: SelectorKind::Dir, ..names(&["target"]) }), ["target"]);
        assert_eq!(
            pick(&t, Selector { kind: SelectorKind::Dir, empty: true, ..Selector::default() }),
            ["target", "empty"]
        );
        assert!(pick(&t, names(&["nomatch*"])).is_empty());
    }

    #[test]
    fn matches_inside_a_match_are_deduplicated_and_the_root_is_never_selected() {
        let t = tree();
        // `*` matches the root's name too, but the walk starts below the root.
        let all = pick(&t, Selector { kind: SelectorKind::Dir, ..names(&["*"]) });
        assert!(!all.contains(&"root".to_owned()));
        assert_eq!(all.len(), 3);
        let c = Compiled::new(&def(Selector { kind: SelectorKind::Dir, ..names(&["*"]) }), 0).unwrap();
        let sub = t.children(t.root()).find(|&c| t.get(c).is_some_and(|n| n.name() == "sub")).unwrap();
        let from_sub = select(&t, sub, &c, SizeMode::Logical, 100);
        assert!(from_sub.nodes.is_empty());
    }

    #[test]
    fn older_than_needs_a_known_mtime_and_the_cap_truncates() {
        let mut t = Tree::new("r");
        let mut data = dirustat_model::NodeData::with_size(1);
        data.mtime = Some(1_000);
        t.add_node(t.root(), "old.log", NodeKind::File, data).unwrap();
        t.add_child(t.root(), "unknown.log", NodeKind::File, 1).unwrap();
        data.mtime = Some(1_000_000);
        t.add_node(t.root(), "new.log", NodeKind::File, data).unwrap();
        t.aggregate();
        let sel = Selector { older_than_days: Some(1), ..names(&["*.log"]) };
        let c = Compiled::new(&def(sel.clone()), 1_000_000).unwrap();
        let out = select(&t, t.root(), &c, SizeMode::Logical, 100);
        assert_eq!(out.nodes.len(), 1);
        let c = Compiled::new(&def(names(&["*.log"])), 0).unwrap();
        let out = select(&t, t.root(), &c, SizeMode::Logical, 2);
        assert!(out.truncated && out.nodes.len() == 2);
        // Hostile `now` values do not overflow.
        assert!(Compiled::new(&def(sel), i64::MIN).is_ok());
    }

    #[test]
    fn disk_emptiness_guard() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a").join("b")).unwrap();
        assert!(dir_is_empty_on_disk(dir.path()));
        std::fs::write(dir.path().join("a").join("b").join("f"), b"x").unwrap();
        assert!(!dir_is_empty_on_disk(dir.path()));
        assert!(!dir_is_empty_on_disk(&dir.path().join("missing")));
    }
}
