//! L0: file tree model.
//!
//! The tree lives in a flat arena vector: a node refers to its parent, first child and next
//! sibling by `NodeId` index. A parent is always added before its children, so size aggregation
//! is a single reverse pass with no recursion (millions of nodes, deep trees).

use std::fmt;

/// Node limit: `NodeId` is a `u32`, minus headroom for reserved values.
pub const MAX_NODES: usize = (u32::MAX - 1) as usize;

/// Index of a node in the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    /// Node index as `usize` for arena access.
    pub fn index(self) -> usize {
        usize::try_from(self.0).unwrap_or(usize::MAX)
    }
}

/// Kind of a tree node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    /// Symlink/junction/reparse point: has its own size, is never descended into.
    Link,
}

/// Which size a caller wants: file size or space occupied on disk (`AGENTS.md`, rule 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SizeMode {
    /// Logical size: bytes a reader sees.
    Logical,
    /// Allocated size: clusters, sparse files and compression taken into account.
    #[default]
    Allocated,
}

/// Per-node payload supplied by the scanner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeData {
    /// Logical size in bytes (usually 0 for a directory).
    pub logical_size: u64,
    /// Allocated size in bytes (usually 0 for a directory).
    pub allocated_size: u64,
    /// Modification time, seconds since the Unix epoch; `None` when unknown.
    pub mtime: Option<i64>,
    /// Raw platform attributes (Windows file attributes bitmask).
    pub attributes: u32,
    /// The node could not be read completely (access denied, vanished mid-walk, ...).
    pub error: bool,
    /// The stored name is lossy: the original contained invalid UTF-16 (lone surrogates) that
    /// was replaced with U+FFFD (ADR 0008).
    pub name_lossy: bool,
}

/// Decodes a UTF-16 name (NTFS/Win32) into a `String`. Unpaired surrogates become U+FFFD; the
/// flag is `true` when any replacement happened (ADR 0008). Never fails.
pub fn name_from_wide(wide: &[u16]) -> (String, bool) {
    let mut out = String::with_capacity(wide.len());
    let mut lossy = false;
    for unit in char::decode_utf16(wide.iter().copied()) {
        match unit {
            Ok(c) => out.push(c),
            Err(_) => {
                out.push(char::REPLACEMENT_CHARACTER);
                lossy = true;
            }
        }
    }
    (out, lossy)
}

impl NodeData {
    /// Data for a node whose logical and allocated sizes are equal.
    pub fn with_size(size: u64) -> Self {
        Self { logical_size: size, allocated_size: size, ..Self::default() }
    }
}

/// Aggregated figures of a subtree, valid after `Tree::aggregate`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Totals {
    logical: u64,
    allocated: u64,
    files: u64,
    dirs: u64,
    errors: u64,
}

/// A tree node. Fields are private: `Tree` maintains the invariants.
#[derive(Clone, Debug)]
pub struct Node {
    name: Box<str>,
    kind: NodeKind,
    data: NodeData,
    totals: Totals,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    next_sibling: Option<NodeId>,
}

impl Node {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn kind(&self) -> NodeKind {
        self.kind
    }
    /// The node's own payload as given by the scanner.
    pub fn data(&self) -> &NodeData {
        &self.data
    }
    /// Size of the node itself (usually 0 for a directory).
    pub fn own_size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Logical => self.data.logical_size,
            SizeMode::Allocated => self.data.allocated_size,
        }
    }
    /// Size of the node including descendants; valid after `Tree::aggregate`.
    pub fn total_size(&self, mode: SizeMode) -> u64 {
        match mode {
            SizeMode::Logical => self.totals.logical,
            SizeMode::Allocated => self.totals.allocated,
        }
    }
    /// Number of files and links in the subtree; valid after `Tree::aggregate`.
    pub fn file_count(&self) -> u64 {
        self.totals.files
    }
    /// Number of directories in the subtree (the node itself included); valid after `aggregate`.
    pub fn dir_count(&self) -> u64 {
        self.totals.dirs
    }
    /// Number of nodes with the error flag in the subtree; valid after `aggregate`.
    pub fn error_count(&self) -> u64 {
        self.totals.errors
    }
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
}

/// Model errors. Panics are forbidden in this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// No such `NodeId` in the tree.
    UnknownNode,
    /// A child cannot be added to a file or a link.
    NotADirectory,
    /// `MAX_NODES` exceeded.
    TooManyNodes,
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNode => f.write_str("unknown node id"),
            Self::NotADirectory => f.write_str("parent is not a directory"),
            Self::TooManyNodes => f.write_str("node limit exceeded"),
        }
    }
}

impl std::error::Error for ModelError {}

/// File tree. The root always exists and is always a directory.
#[derive(Clone, Debug)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// Creates a tree with a root directory named `root_name` (usually the scanned path).
    pub fn new(root_name: &str) -> Self {
        let root = Node {
            name: root_name.into(),
            kind: NodeKind::Dir,
            data: NodeData::default(),
            totals: Totals::default(),
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
        };
        Self { nodes: vec![root] }
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// The tree is never empty (the root exists); kept for clippy and symmetry with `len`.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.index())
    }

    /// The id of the node stored at arena `index`, if there is one. O(1); used to resolve indices
    /// reported to (and echoed back by) agents without walking the tree.
    pub fn node_at(&self, index: usize) -> Option<NodeId> {
        if index >= self.nodes.len() {
            return None;
        }
        u32::try_from(index).ok().map(NodeId)
    }

    /// Adds a child with equal logical and allocated size. Totals are valid after `aggregate`.
    pub fn add_child(&mut self, parent: NodeId, name: &str, kind: NodeKind, size: u64) -> Result<NodeId, ModelError> {
        self.add_node(parent, name, kind, NodeData::with_size(size))
    }

    /// Adds a child whose name is raw UTF-16. Invalid sequences become U+FFFD and
    /// `NodeData::name_lossy` is set (ADR 0008); a `name_lossy` already set in `data` is kept.
    pub fn add_node_from_wide(
        &mut self,
        parent: NodeId,
        wide_name: &[u16],
        kind: NodeKind,
        mut data: NodeData,
    ) -> Result<NodeId, ModelError> {
        let (name, lossy) = name_from_wide(wide_name);
        data.name_lossy |= lossy;
        self.add_node(parent, &name, kind, data)
    }

    /// Adds a child with full scanner data. Totals are valid after `aggregate`.
    pub fn add_node(
        &mut self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        data: NodeData,
    ) -> Result<NodeId, ModelError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(ModelError::TooManyNodes);
        }
        let parent_node = self.nodes.get(parent.index()).ok_or(ModelError::UnknownNode)?;
        if parent_node.kind != NodeKind::Dir {
            return Err(ModelError::NotADirectory);
        }
        let prev_last = parent_node.last_child;
        let new_id = NodeId(u32::try_from(self.nodes.len()).map_err(|_| ModelError::TooManyNodes)?);
        self.nodes.push(Node {
            name: name.into(),
            kind,
            data,
            totals: Totals::default(),
            parent: Some(parent),
            first_child: None,
            last_child: None,
            next_sibling: None,
        });
        if let Some(prev) = prev_last.and_then(|p| self.nodes.get_mut(p.index())) {
            prev.next_sibling = Some(new_id);
        }
        if let Some(p) = self.nodes.get_mut(parent.index()) {
            if p.first_child.is_none() {
                p.first_child = Some(new_id);
            }
            p.last_child = Some(new_id);
        }
        Ok(new_id)
    }

    /// Renames a node (for example to give a scanned root a neutral display label).
    pub fn set_node_name(&mut self, id: NodeId, name: &str) -> Result<(), ModelError> {
        let node = self.nodes.get_mut(id.index()).ok_or(ModelError::UnknownNode)?;
        node.name = name.into();
        Ok(())
    }

    /// Flags a node as unreadable (e.g. a directory that denied access after it was added).
    pub fn mark_error(&mut self, id: NodeId) -> Result<(), ModelError> {
        let node = self.nodes.get_mut(id.index()).ok_or(ModelError::UnknownNode)?;
        node.data.error = true;
        Ok(())
    }

    /// Iterates over direct children.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children { tree: self, next: self.get(id).and_then(|n| n.first_child) }
    }

    /// Recomputes the totals of every node in one reverse pass (a parent precedes its children).
    /// Sums saturate: hostile sizes cannot overflow.
    pub fn aggregate(&mut self) {
        for node in &mut self.nodes {
            let is_dir = node.kind == NodeKind::Dir;
            node.totals = Totals {
                logical: node.data.logical_size,
                allocated: node.data.allocated_size,
                files: u64::from(!is_dir),
                dirs: u64::from(is_dir),
                errors: u64::from(node.data.error),
            };
        }
        for idx in (1..self.nodes.len()).rev() {
            let Some((child, parent)) = self.nodes.get(idx).and_then(|n| n.parent.map(|p| (n.totals, p))) else {
                continue;
            };
            if let Some(p) = self.nodes.get_mut(parent.index()) {
                p.totals.logical = p.totals.logical.saturating_add(child.logical);
                p.totals.allocated = p.totals.allocated.saturating_add(child.allocated);
                p.totals.files = p.totals.files.saturating_add(child.files);
                p.totals.dirs = p.totals.dirs.saturating_add(child.dirs);
                p.totals.errors = p.totals.errors.saturating_add(child.errors);
            }
        }
    }
}

/// Iterator over the direct children of a node, in insertion order.
#[derive(Debug)]
pub struct Children<'a> {
    tree: &'a Tree,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let current = self.next?;
        self.next = self.tree.get(current).and_then(|n| n.next_sibling);
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SizeMode = SizeMode::Allocated;
    const L: SizeMode = SizeMode::Logical;

    #[test]
    fn node_at_resolves_arena_indices_in_constant_time() {
        let mut t = Tree::new("r");
        let a = t.add_child(t.root(), "a", NodeKind::File, 1).unwrap();
        assert_eq!(t.node_at(0), Some(t.root()));
        assert_eq!(t.node_at(1), Some(a));
        assert_eq!(t.node_at(2), None);
        assert_eq!(t.node_at(usize::MAX), None);
    }

    #[test]
    fn aggregates_sizes_bottom_up() {
        let mut t = Tree::new("C:\\");
        let dir = t.add_child(t.root(), "dir", NodeKind::Dir, 0).unwrap();
        t.add_child(dir, "a.bin", NodeKind::File, 100).unwrap();
        t.add_child(dir, "b.bin", NodeKind::File, 50).unwrap();
        t.add_child(t.root(), "c.bin", NodeKind::File, 7).unwrap();
        t.aggregate();
        assert_eq!(t.get(dir).unwrap().total_size(A), 150);
        assert_eq!(t.get(t.root()).unwrap().total_size(A), 157);
        assert_eq!(t.children(t.root()).count(), 2);
    }

    #[test]
    fn logical_and_allocated_sizes_stay_separate() {
        let mut t = Tree::new("r");
        let small = NodeData { logical_size: 10, allocated_size: 4096, ..NodeData::default() };
        let sparse = NodeData { logical_size: 1 << 30, allocated_size: 0, ..NodeData::default() };
        t.add_node(t.root(), "small", NodeKind::File, small).unwrap();
        t.add_node(t.root(), "sparse", NodeKind::File, sparse).unwrap();
        t.aggregate();
        let root = t.get(t.root()).unwrap();
        assert_eq!(root.total_size(L), 10 + (1 << 30));
        assert_eq!(root.total_size(A), 4096);
    }

    #[test]
    fn counts_files_dirs_and_errors() {
        let mut t = Tree::new("r");
        let d1 = t.add_child(t.root(), "d1", NodeKind::Dir, 0).unwrap();
        let d2 = t.add_child(d1, "d2", NodeKind::Dir, 0).unwrap();
        t.add_child(d2, "f", NodeKind::File, 1).unwrap();
        t.add_child(d1, "l", NodeKind::Link, 0).unwrap();
        t.add_node(t.root(), "locked", NodeKind::Dir, NodeData { error: true, ..NodeData::default() }).unwrap();
        t.aggregate();
        let root = t.get(t.root()).unwrap();
        assert_eq!((root.dir_count(), root.file_count(), root.error_count()), (4, 2, 1));
        let sub = t.get(d1).unwrap();
        assert_eq!((sub.dir_count(), sub.file_count(), sub.error_count()), (2, 2, 0));
    }

    #[test]
    fn mark_error_is_reflected_after_aggregate() {
        let mut t = Tree::new("r");
        let d = t.add_child(t.root(), "d", NodeKind::Dir, 0).unwrap();
        t.mark_error(d).unwrap();
        assert_eq!(t.mark_error(NodeId(999)), Err(ModelError::UnknownNode));
        t.aggregate();
        assert_eq!(t.get(t.root()).unwrap().error_count(), 1);
        assert!(t.get(d).unwrap().data().error);
    }

    #[test]
    fn keeps_mtime_attributes_and_unicode_names() {
        let mut t = Tree::new("r");
        let data = NodeData { mtime: Some(-1), attributes: 0x20, ..NodeData::with_size(3) };
        let id = t.add_node(t.root(), "файл ✓ 日本語.txt", NodeKind::File, data).unwrap();
        let node = t.get(id).unwrap();
        assert_eq!(node.name(), "файл ✓ 日本語.txt");
        assert_eq!((node.data().mtime, node.data().attributes), (Some(-1), 0x20));
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn wide_valid_name_is_not_lossy() {
        let (name, lossy) = name_from_wide(&wide("файл 😀.txt"));
        assert_eq!((name.as_str(), lossy), ("файл 😀.txt", false));
    }

    #[test]
    fn wide_lone_surrogates_are_replaced_and_flagged() {
        let (name, lossy) = name_from_wide(&[0x61, 0xD800, 0x62, 0xDC00, 0xD83D]);
        assert_eq!((name.as_str(), lossy), ("a\u{FFFD}b\u{FFFD}\u{FFFD}", true));
        // A high surrogate followed by a non-low unit keeps the following character.
        let (name, lossy) = name_from_wide(&[0xD800, 0x63]);
        assert_eq!((name.as_str(), lossy), ("\u{FFFD}c", true));
    }

    #[test]
    fn wide_empty_name() {
        assert_eq!(name_from_wide(&[]), (String::new(), false));
    }

    #[test]
    fn wide_long_name_and_flag_position() {
        let mut units = vec![0x78u16; 40_000];
        let (name, lossy) = name_from_wide(&units);
        assert_eq!((name.len(), lossy), (40_000, false));
        units.push(0xDFFF);
        let (name, lossy) = name_from_wide(&units);
        assert_eq!((name.chars().count(), lossy), (40_001, true));
    }

    #[test]
    fn set_node_name_renames_or_reports_unknown() {
        let mut t = Tree::new("/real/path");
        assert_eq!(t.set_node_name(t.root(), "demo"), Ok(()));
        assert_eq!(t.get(t.root()).map(Node::name), Some("demo"));
        assert_eq!(t.set_node_name(NodeId(999), "x"), Err(ModelError::UnknownNode));
    }

    #[test]
    fn wide_flag_propagates_to_node_data() {
        let mut t = Tree::new("r");
        let bad = t.add_node_from_wide(t.root(), &[0x66, 0xD800], NodeKind::File, NodeData::with_size(5)).unwrap();
        let good = t.add_node_from_wide(t.root(), &wide("ok"), NodeKind::File, NodeData::default()).unwrap();
        let kept = NodeData { name_lossy: true, ..NodeData::default() };
        let pre = t.add_node_from_wide(t.root(), &wide("pre"), NodeKind::File, kept).unwrap();
        let bad = t.get(bad).unwrap();
        assert_eq!((bad.name(), bad.data().name_lossy, bad.data().logical_size), ("f\u{FFFD}", true, 5));
        assert!(!t.get(good).unwrap().data().name_lossy);
        assert!(t.get(pre).unwrap().data().name_lossy);
        assert_eq!(
            t.add_node_from_wide(NodeId(999), &wide("x"), NodeKind::File, NodeData::default()),
            Err(ModelError::UnknownNode)
        );
    }

    #[test]
    fn rejects_children_of_files_and_unknown_parents() {
        let mut t = Tree::new("r");
        let f = t.add_child(t.root(), "f", NodeKind::File, 1).unwrap();
        assert_eq!(t.add_child(f, "x", NodeKind::File, 1), Err(ModelError::NotADirectory));
        let bogus = NodeId(999);
        assert_eq!(t.add_child(bogus, "x", NodeKind::File, 1), Err(ModelError::UnknownNode));
    }

    #[test]
    fn hostile_sizes_saturate() {
        let mut t = Tree::new("r");
        t.add_child(t.root(), "a", NodeKind::File, u64::MAX).unwrap();
        t.add_child(t.root(), "b", NodeKind::File, u64::MAX).unwrap();
        t.aggregate();
        let root = t.get(t.root()).unwrap();
        assert_eq!(root.total_size(A), u64::MAX);
        assert_eq!(root.total_size(L), u64::MAX);
    }
}
