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

/// A tree node. Fields are private: `Tree` maintains the invariants.
#[derive(Clone, Debug)]
pub struct Node {
    name: Box<str>,
    kind: NodeKind,
    own_size: u64,
    total_size: u64,
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
    /// Size of the node itself (usually 0 for a directory).
    pub fn own_size(&self) -> u64 {
        self.own_size
    }
    /// Size of the node including descendants; valid after `Tree::aggregate`.
    pub fn total_size(&self) -> u64 {
        self.total_size
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
            own_size: 0,
            total_size: 0,
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

    /// Adds a child. `own_size` is reflected in totals after `aggregate`.
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: &str,
        kind: NodeKind,
        own_size: u64,
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
            own_size,
            total_size: own_size,
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

    /// Iterates over direct children.
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children { tree: self, next: self.get(id).and_then(|n| n.first_child) }
    }

    /// Recomputes `total_size` of every node in one reverse pass (a parent precedes its children).
    /// Sums saturate: hostile sizes cannot overflow.
    pub fn aggregate(&mut self) {
        for node in &mut self.nodes {
            node.total_size = node.own_size;
        }
        for idx in (1..self.nodes.len()).rev() {
            let Some((child_total, parent)) = self.nodes.get(idx).and_then(|n| n.parent.map(|p| (n.total_size, p)))
            else {
                continue;
            };
            if let Some(p) = self.nodes.get_mut(parent.index()) {
                p.total_size = p.total_size.saturating_add(child_total);
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

    #[test]
    fn aggregates_sizes_bottom_up() {
        let mut t = Tree::new("C:\\");
        let dir = t.add_child(t.root(), "dir", NodeKind::Dir, 0).unwrap();
        t.add_child(dir, "a.bin", NodeKind::File, 100).unwrap();
        t.add_child(dir, "b.bin", NodeKind::File, 50).unwrap();
        t.add_child(t.root(), "c.bin", NodeKind::File, 7).unwrap();
        t.aggregate();
        assert_eq!(t.get(dir).unwrap().total_size(), 150);
        assert_eq!(t.get(t.root()).unwrap().total_size(), 157);
        assert_eq!(t.children(t.root()).count(), 2);
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
        assert_eq!(t.get(t.root()).unwrap().total_size(), u64::MAX);
    }
}
