//! Per-extension statistics (the extension list): pure logic over a `model::Tree`.
//!
//! Extension rules: the extension is the text after the last dot of the file name, lowercased
//! (Unicode `to_lowercase`) and without the dot. A name without a dot, a dotfile whose only dot
//! is the first character (`.gitignore`) and a name ending in a dot (`name.`) have **no**
//! extension (empty string). Only the last component counts (`a.tar.gz` -> `gz`).
//! Files and links are counted; directories are not.

use dirustat_model::{NodeKind, SizeMode, Tree};
use std::collections::HashMap;

/// Aggregated figures for one extension.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtStat {
    /// Lowercase extension without the dot; empty for files without one.
    pub ext: String,
    pub file_count: u64,
    /// Sum of file sizes by the requested `SizeMode` (saturating).
    pub size: u64,
    /// Share of the total size in percent (0.0 when the total is zero).
    pub percent: f64,
}

/// Extension of a file name under the rules in the module docs.
pub fn extension_of(name: &str) -> String {
    match name.rfind('.') {
        Some(0) | None => String::new(),
        Some(i) => name.get(i.saturating_add(1)..).unwrap_or_default().to_lowercase(),
    }
}

/// Aggregates `tree` by extension, sorted by size descending, then extension ascending.
/// Iterative; sums saturate.
pub fn extension_stats(tree: &Tree, mode: SizeMode) -> Vec<ExtStat> {
    let mut acc: HashMap<String, (u64, u64)> = HashMap::new();
    let mut stack = vec![tree.root()];
    while let Some(id) = stack.pop() {
        let Some(node) = tree.get(id) else { continue };
        match node.kind() {
            NodeKind::Dir => stack.extend(tree.children(id)),
            NodeKind::File | NodeKind::Link => {
                let entry = acc.entry(extension_of(node.name())).or_insert((0, 0));
                entry.0 = entry.0.saturating_add(1);
                entry.1 = entry.1.saturating_add(node.own_size(mode));
            }
        }
    }
    let total = acc.values().fold(0u64, |t, (_, s)| t.saturating_add(*s));
    let mut out: Vec<ExtStat> = acc
        .into_iter()
        .map(|(ext, (file_count, size))| ExtStat { ext, file_count, size, percent: percent_of(size, total) })
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.ext.cmp(&b.ext)));
    out
}

fn percent_of(part: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    // Precision loss above 2^53 bytes is irrelevant for a percentage.
    #[allow(clippy::cast_precision_loss)]
    let p = part as f64 / total as f64 * 100.0;
    if p.is_finite() { p.clamp(0.0, 100.0) } else { 0.0 }
}

/// Colour for files without an extension.
pub const NO_EXT_COLOR: [u8; 3] = [128, 128, 128];

const PALETTE: [[u8; 3]; 16] = [
    [230, 25, 75],
    [60, 180, 75],
    [255, 225, 25],
    [0, 130, 200],
    [245, 130, 48],
    [145, 30, 180],
    [70, 240, 240],
    [240, 50, 230],
    [210, 245, 60],
    [250, 190, 212],
    [0, 128, 128],
    [220, 190, 255],
    [170, 110, 40],
    [255, 250, 200],
    [128, 0, 0],
    [170, 255, 195],
];

/// Deterministic colour for an extension (case-insensitive, FNV-1a hash into a fixed palette).
/// Stable across runs and platforms; the empty extension gets `NO_EXT_COLOR`.
pub fn ext_color(ext: &str) -> [u8; 3] {
    if ext.is_empty() {
        return NO_EXT_COLOR;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in ext.to_lowercase().bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let idx = usize::try_from(hash % PALETTE.len() as u64).unwrap_or(0);
    PALETTE.get(idx).copied().unwrap_or(NO_EXT_COLOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        let mut t = Tree::new("root");
        let root = t.root();
        let d = t.add_child(root, "sub", NodeKind::Dir, 0).unwrap();
        t.add_child(root, "A.TXT", NodeKind::File, 100).unwrap();
        t.add_child(d, "b.txt", NodeKind::File, 50).unwrap();
        t.add_child(d, "Makefile", NodeKind::File, 10).unwrap();
        t.add_child(d, ".gitignore", NodeKind::File, 5).unwrap();
        t.add_child(d, "x.tar.gz", NodeKind::File, 300).unwrap();
        t.add_child(d, "dir.d", NodeKind::Dir, 0).unwrap();
        t.aggregate();
        t
    }

    #[test]
    fn extension_rules() {
        assert_eq!(extension_of("A.TXT"), "txt");
        assert_eq!(extension_of("x.tar.gz"), "gz");
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("Makefile"), "");
        assert_eq!(extension_of("name."), "");
        assert_eq!(extension_of(""), "");
        assert_eq!(extension_of("..x"), "x");
    }

    #[test]
    fn aggregates_sorted_and_case_folded() {
        let stats = extension_stats(&tree(), SizeMode::Logical);
        let got: Vec<(&str, u64, u64)> = stats.iter().map(|s| (s.ext.as_str(), s.file_count, s.size)).collect();
        assert_eq!(got, vec![("gz", 1, 300), ("txt", 2, 150), ("", 2, 15)]);
    }

    #[test]
    fn percent_sums_to_100() {
        let stats = extension_stats(&tree(), SizeMode::Allocated);
        let sum: f64 = stats.iter().map(|s| s.percent).sum();
        assert!((sum - 100.0).abs() < 1e-9, "{sum}");
    }

    #[test]
    fn empty_tree_and_zero_sizes() {
        assert!(extension_stats(&Tree::new("r"), SizeMode::Logical).is_empty());
        let mut t = Tree::new("r");
        let root = t.root();
        t.add_child(root, "a.bin", NodeKind::File, 0).unwrap();
        t.aggregate();
        let stats = extension_stats(&t, SizeMode::Logical);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].percent, 0.0);
    }

    #[test]
    fn ties_sort_by_extension() {
        let mut t = Tree::new("r");
        let root = t.root();
        t.add_child(root, "a.zz", NodeKind::File, 1).unwrap();
        t.add_child(root, "a.aa", NodeKind::File, 1).unwrap();
        t.aggregate();
        let exts: Vec<String> = extension_stats(&t, SizeMode::Logical).into_iter().map(|s| s.ext).collect();
        assert_eq!(exts, vec!["aa", "zz"]);
    }

    #[test]
    fn colours_are_deterministic_and_case_insensitive() {
        assert_eq!(ext_color("rs"), ext_color("rs"));
        assert_eq!(ext_color("RS"), ext_color("rs"));
        assert_eq!(ext_color(""), NO_EXT_COLOR);
        // Pinned value guards against accidental hash/palette changes.
        assert_eq!(ext_color("txt"), [170, 255, 195]);
        let distinct: std::collections::HashSet<[u8; 3]> =
            ["rs", "txt", "png", "mp4", "zip", "dll", "exe", "iso"].iter().map(|e| ext_color(e)).collect();
        assert!(distinct.len() > 3);
    }
}
