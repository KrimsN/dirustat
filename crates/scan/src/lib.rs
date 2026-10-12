//! L1: filesystem scanning. Builds `dirustat_model::Tree`.
//!
//! Status: stub (milestone M1). Planned: portable parallel walk (rayon), progress stream and
//! cancellation, correct size on disk (cluster size, sparse, hardlinks counted once, reparse
//! points not followed), fast path through `dirustat-ntfs`.

use dirustat_model::Tree;

/// Builds a tree with a single root and no disk walk — a skeleton for testing higher layers.
pub fn empty_tree(root: &str) -> Tree {
    let mut tree = Tree::new(root);
    tree.aggregate();
    tree
}

/// Whether the MFT fast path is available for the volume.
pub fn mft_available(volume: &str) -> bool {
    dirustat_ntfs::probe(volume) != dirustat_ntfs::MftSupport::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::NodeKind;

    #[test]
    fn empty_tree_has_only_root() {
        let t = empty_tree("C:\\");
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(t.root()).map(|n| n.kind()), Some(NodeKind::Dir));
        assert!(!mft_available("C:"));
    }
}
