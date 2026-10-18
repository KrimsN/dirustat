//! Rebuilds a tree without one subtree, so the session stays consistent after a deletion or move
//! without a rescan. The model has no removal API (an arena with stable indices), so the
//! survivors are copied into a fresh arena: O(n), iterative, sibling order preserved. Arena
//! indices change; callers bump the revision.

use dirustat_model::{NodeId, Tree};
use std::collections::HashSet;

/// `tree` without `remove` and everything below it; `None` for the root or an unknown node.
pub(crate) fn without_subtree(tree: &Tree, remove: NodeId) -> Option<Tree> {
    without_subtrees(tree, &[remove])
}

/// `tree` without every node in `remove` and everything below them, in one pass; `None` when the
/// list contains the root or an unknown node.
pub(crate) fn without_subtrees(tree: &Tree, remove: &[NodeId]) -> Option<Tree> {
    let removed: HashSet<NodeId> = remove.iter().copied().collect();
    for &id in &removed {
        if id == tree.root() {
            return None;
        }
        tree.get(id)?;
    }
    let root = tree.get(tree.root())?;
    let mut out = Tree::new(root.name());
    if root.data().error {
        out.mark_error(out.root()).ok()?;
    }
    // Old arena index -> id in the new tree, filled when a directory is pushed on the stack.
    let mut stack: Vec<(NodeId, NodeId)> = vec![(tree.root(), out.root())];
    while let Some((old_parent, new_parent)) = stack.pop() {
        for child in tree.children(old_parent) {
            if removed.contains(&child) {
                continue;
            }
            let node = tree.get(child)?;
            let new_id = out.add_node(new_parent, node.name(), node.kind(), *node.data()).ok()?;
            if node.kind() == dirustat_model::NodeKind::Dir {
                stack.push((child, new_id));
            }
        }
    }
    out.aggregate();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::{NodeKind, SizeMode};

    #[test]
    fn subtree_is_removed_and_totals_shrink() {
        let mut t = Tree::new("root");
        let a = t.add_child(t.root(), "a", NodeKind::Dir, 0).unwrap();
        t.add_child(a, "x", NodeKind::File, 10).unwrap();
        t.add_child(a, "y", NodeKind::File, 20).unwrap();
        t.add_child(t.root(), "b", NodeKind::File, 5).unwrap();
        t.aggregate();
        let pruned = without_subtree(&t, a).unwrap();
        assert_eq!(pruned.len(), 2);
        let root = pruned.get(pruned.root()).unwrap();
        assert_eq!(root.total_size(SizeMode::Logical), 5);
        assert_eq!(root.file_count(), 1);
        assert_eq!(pruned.children(pruned.root()).count(), 1);
    }

    #[test]
    fn root_and_unknown_nodes_are_refused() {
        let t = Tree::new("root");
        assert!(without_subtree(&t, t.root()).is_none());
        let other = {
            let mut o = Tree::new("o");
            o.add_child(o.root(), "a", NodeKind::File, 1).unwrap()
        };
        assert!(without_subtree(&t, other).is_none());
    }

    #[test]
    fn order_and_nesting_are_preserved() {
        let mut t = Tree::new("r");
        let d1 = t.add_child(t.root(), "d1", NodeKind::Dir, 0).unwrap();
        let gone = t.add_child(t.root(), "gone", NodeKind::Dir, 0).unwrap();
        let d2 = t.add_child(t.root(), "d2", NodeKind::Dir, 0).unwrap();
        for d in [d1, gone, d2] {
            t.add_child(d, "f1", NodeKind::File, 1).unwrap();
            t.add_child(d, "f2", NodeKind::File, 2).unwrap();
        }
        t.aggregate();
        let p = without_subtree(&t, gone).unwrap();
        let names: Vec<&str> = p.children(p.root()).filter_map(|c| p.get(c)).map(|n| n.name()).collect();
        assert_eq!(names, ["d1", "d2"]);
        let first = p.children(p.root()).next().unwrap();
        let kids: Vec<&str> = p.children(first).filter_map(|c| p.get(c)).map(|n| n.name()).collect();
        assert_eq!(kids, ["f1", "f2"]);
        assert_eq!(p.len(), 7);
    }
}
