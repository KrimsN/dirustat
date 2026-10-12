//! L1: сканирование файловой системы. Строит `dirustat_model::Tree`.
//!
//! Статус: заглушка (веха M1). Планируется: переносимый параллельный обход (rayon), поток
//! прогресса и отмена, корректный размер на диске (cluster size, sparse, hardlinks без
//! двойного счёта, reparse points без захода), быстрый путь через `dirustat-ntfs`.

use dirustat_model::Tree;

/// Строит дерево из одного корня без обхода диска — каркас для тестов вышележащих слоёв.
pub fn empty_tree(root: &str) -> Tree {
    let mut tree = Tree::new(root);
    tree.aggregate();
    tree
}

/// Доступен ли быстрый путь MFT для тома.
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
