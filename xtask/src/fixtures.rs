//! `cargo xtask fixtures`: deterministic, seeded synthetic directory trees for scan tests and benchmarks.
//!
//! Generation is split in two: the *plan* (which entries exist, their sizes) depends only on
//! `(nodes, seed)` and is emitted through the [`Sink`] trait; [`FsSink`] materialises it on disk and
//! the tests use an in-memory sink to prove determinism without touching the file system.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Directory depth after which the remaining budget becomes plain files.
const MAX_DEPTH: usize = 14;
/// Files placed directly into a directory are drawn from `0..=MAX_FILES_PER_DIR`.
const MAX_FILES_PER_DIR: u64 = 30;
/// Maximum number of sub-directories per directory.
const MAX_SUBDIRS: u64 = 6;

const STEMS: &[&str] = &["report", "image", "data", "archive", "cache", "log", "build", "track", "note", "chunk"];
const UNICODE_STEMS: &[&str] = &["файл", "日本語", "ünïcödé", "café", "αβγ", "emoji_😀"];
const EXTENSIONS: &[&str] = &["txt", "bin", "dat", "log", "jpg", "mp4", "zip", "rs", "json", "tmp"];

/// Deterministic splitmix64 generator: no dependencies, identical output on every platform.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform value in `lo..=hi` (`hi < lo` yields `lo`).
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        let span = hi.saturating_sub(lo).saturating_add(1);
        lo.saturating_add(self.next_u64().checked_rem(span).unwrap_or(0))
    }
}

/// Receiver of the generated plan. Paths are relative to the fixture root, `/`-separated.
pub trait Sink {
    fn dir(&mut self, rel: &str) -> io::Result<()>;
    fn file(&mut self, rel: &str, size: u64) -> io::Result<()>;
}

/// Summary of what was generated.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    pub dirs: u64,
    pub files: u64,
    pub bytes: u64,
    /// Special cases that could not be created on this machine (e.g. symlinks without privilege).
    pub skipped: Vec<String>,
}

/// Named size presets (node counts include directories, excluding the special-case folder).
pub fn preset(name: &str) -> Option<u64> {
    match name {
        "small" => Some(1_000),
        "100k" => Some(100_000),
        "1m" => Some(1_000_000),
        "5m" => Some(5_000_000),
        _ => None,
    }
}

fn file_size(rng: &mut Rng) -> u64 {
    match rng.range(0, 999) {
        0 => rng.range(65_537, 1_048_576),
        1..=90 => rng.range(2_049, 65_536),
        _ => rng.range(0, 2_048),
    }
}

fn file_name(rng: &mut Rng, index: u64) -> String {
    let unicode = rng.range(0, 19) == 0;
    let pool = if unicode { UNICODE_STEMS } else { STEMS };
    let stem = pool.get(rng.range(0, pool.len() as u64 - 1) as usize).copied().unwrap_or("file");
    let ext = EXTENSIONS.get(rng.range(0, EXTENSIONS.len() as u64 - 1) as usize).copied().unwrap_or("bin");
    format!("{stem}_{index}.{ext}")
}

/// Splits `total` into `parts` positive pieces with random proportions.
fn split(rng: &mut Rng, total: u64, parts: u64) -> Vec<u64> {
    let parts = parts.clamp(1, total.max(1));
    let weights: Vec<u64> = (0..parts).map(|_| rng.range(1, 100)).collect();
    let sum: u64 = weights.iter().sum();
    // Every part gets 1; the surplus is shared by weight, rounding leftovers go to the first part.
    let surplus = total.saturating_sub(parts);
    let mut out: Vec<u64> = weights.iter().map(|w| 1 + surplus.saturating_mul(*w) / sum.max(1)).collect();
    let assigned: u64 = out.iter().sum();
    if let Some(first) = out.first_mut() {
        *first = first.saturating_add(total.saturating_sub(assigned));
    }
    out
}

/// Emits the random part of the tree: exactly `nodes` entries (directories + files) under the root.
pub fn generate<S: Sink>(sink: &mut S, nodes: u64, seed: u64, summary: &mut Summary) -> io::Result<()> {
    let mut rng = Rng::new(seed);
    let mut counter = 0u64;
    // Explicit stack: (relative path, depth, node budget including the directory itself).
    let mut stack: Vec<(String, usize, u64)> = vec![(String::new(), 0, nodes.saturating_add(1))];
    while let Some((path, depth, budget)) = stack.pop() {
        if !path.is_empty() {
            sink.dir(&path)?;
            summary.dirs += 1;
        }
        let remaining = budget.saturating_sub(1);
        let files_here = if depth >= MAX_DEPTH { remaining } else { rng.range(0, MAX_FILES_PER_DIR).min(remaining) };
        for _ in 0..files_here {
            counter += 1;
            let name = file_name(&mut rng, counter);
            let size = file_size(&mut rng);
            let rel = if path.is_empty() { name } else { format!("{path}/{name}") };
            sink.file(&rel, size)?;
            summary.files += 1;
            summary.bytes = summary.bytes.saturating_add(size);
        }
        let rest = remaining - files_here;
        if rest == 0 {
            continue;
        }
        let subdirs = rng.range(1, MAX_SUBDIRS);
        for (i, share) in split(&mut rng, rest, subdirs).into_iter().enumerate() {
            counter += 1;
            let name = format!("dir_{counter}_{i}");
            let rel = if path.is_empty() { name } else { format!("{path}/{name}") };
            stack.push((rel, depth + 1, share));
        }
    }
    Ok(())
}

/// Writes the plan to disk. Sizes are set with `set_len`, so no real data is written.
#[derive(Debug)]
pub struct FsSink {
    root: PathBuf,
}

impl FsSink {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn resolve(&self, rel: &str) -> PathBuf {
        extended(&self.root.join(rel))
    }
}

impl Sink for FsSink {
    fn dir(&mut self, rel: &str) -> io::Result<()> {
        fs::create_dir_all(self.resolve(rel))
    }

    fn file(&mut self, rel: &str, size: u64) -> io::Result<()> {
        let file = fs::File::create(self.resolve(rel))?;
        file.set_len(size)
    }
}

/// On Windows, prefix absolute paths with `\\?\` so paths over 260 characters work.
fn extended(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if path.is_absolute() && !text.starts_with(r"\\?\") {
            return PathBuf::from(format!(r"\\?\{}", text.replace('/', "\\")));
        }
    }
    path.to_path_buf()
}

/// Fixed edge cases under `_special/`: empty folders, hardlinks, a symlink cycle, unicode, a 300+ char path.
pub fn special_cases(root: &Path, summary: &mut Summary) -> io::Result<()> {
    let base = root.join("_special");
    for sub in ["empty_a", "empty_b/empty_nested", "unicode_папка_日本語_😀"] {
        fs::create_dir_all(extended(&base.join(sub)))?;
        summary.dirs += sub.split('/').count() as u64;
    }

    let hl = base.join("hardlinks");
    fs::create_dir_all(extended(&hl))?;
    let target = hl.join("target.bin");
    let file = fs::File::create(extended(&target))?;
    file.set_len(100_000)?;
    summary.dirs += 1;
    summary.files += 1;
    summary.bytes = summary.bytes.saturating_add(100_000);
    for i in 1..=2 {
        match fs::hard_link(extended(&target), extended(&hl.join(format!("link_{i}.bin")))) {
            Ok(()) => summary.files += 1,
            Err(err) => summary.skipped.push(format!("hardlink link_{i}: {err}")),
        }
    }

    let cycle = base.join("cycle");
    fs::create_dir_all(extended(&cycle))?;
    summary.dirs += 1;
    match symlink_dir(&base, &cycle.join("loop")) {
        Ok(()) => summary.files += 1,
        Err(err) => summary.skipped.push(format!("symlink cycle: {err}")),
    }

    let mut long = base.join("long_path");
    let mut depth = 0;
    while long.to_string_lossy().len() < 320 {
        long.push(format!("segment_{depth:02}_{}", "x".repeat(30)));
        depth += 1;
    }
    fs::create_dir_all(extended(&long))?;
    summary.dirs += 1 + depth;
    fs::File::create(extended(&long.join("deep.txt")))?.set_len(42)?;
    summary.files += 1;
    summary.bytes = summary.bytes.saturating_add(42);
    Ok(())
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Creates a full fixture in `out` (which must not exist or be empty). Never deletes anything.
pub fn create(out: &Path, nodes: u64, seed: u64) -> io::Result<Summary> {
    if out.exists() && fs::read_dir(out)?.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is not empty; remove it yourself or pick another --out", out.display()),
        ));
    }
    let root = std::path::absolute(out)?;
    fs::create_dir_all(extended(&root))?;
    let mut summary = Summary::default();
    generate(&mut FsSink::new(root.clone()), nodes, seed, &mut summary)?;
    special_cases(&root, &mut summary)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Record(Vec<(String, Option<u64>)>);

    impl Sink for Record {
        fn dir(&mut self, rel: &str) -> io::Result<()> {
            self.0.push((rel.to_owned(), None));
            Ok(())
        }
        fn file(&mut self, rel: &str, size: u64) -> io::Result<()> {
            self.0.push((rel.to_owned(), Some(size)));
            Ok(())
        }
    }

    fn plan(nodes: u64, seed: u64) -> (Record, Summary) {
        let mut sink = Record::default();
        let mut summary = Summary::default();
        assert!(generate(&mut sink, nodes, seed, &mut summary).is_ok());
        (sink, summary)
    }

    #[test]
    fn same_seed_gives_identical_plan() {
        assert_eq!(plan(5_000, 7).0.0, plan(5_000, 7).0.0);
    }

    #[test]
    fn different_seed_gives_different_plan() {
        assert_ne!(plan(5_000, 1).0.0, plan(5_000, 2).0.0);
    }

    #[test]
    fn node_count_is_exact() {
        for nodes in [0, 1, 2, 30, 1_000, 12_345] {
            let (sink, summary) = plan(nodes, 42);
            assert_eq!(summary.dirs + summary.files, nodes, "nodes={nodes}");
            assert_eq!(sink.0.len() as u64, nodes);
        }
    }

    #[test]
    fn paths_are_unique_and_parents_come_first() {
        let (sink, _) = plan(20_000, 3);
        let mut seen = std::collections::HashSet::new();
        for (path, _) in &sink.0 {
            assert!(seen.insert(path.clone()), "duplicate {path}");
            if let Some((parent, _)) = path.rsplit_once('/') {
                assert!(seen.contains(parent), "{path} emitted before its parent");
            }
        }
    }

    #[test]
    fn depth_is_bounded() {
        let (sink, _) = plan(50_000, 9);
        assert!(sink.0.iter().all(|(p, _)| p.matches('/').count() <= MAX_DEPTH));
    }

    #[test]
    fn rng_range_handles_degenerate_bounds() {
        let mut rng = Rng::new(1);
        assert_eq!(rng.range(5, 5), 5);
        assert_eq!(rng.range(9, 3), 9);
        let _ = rng.range(0, u64::MAX);
    }

    #[test]
    fn creates_tree_on_disk_with_special_cases() {
        let tmp = std::env::temp_dir().join(format!("dirustat-fixtures-test-{}", std::process::id()));
        let result = create(&tmp, 300, 11);
        let cleanup = fs::remove_dir_all(extended(&std::path::absolute(&tmp).unwrap_or(tmp.clone())));
        let summary = result.unwrap_or_default();
        assert!(summary.dirs + summary.files >= 300);
        assert!(summary.dirs > 0 && summary.files > 0);
        assert!(cleanup.is_ok());
    }

    #[test]
    fn refuses_non_empty_output() {
        let tmp = std::env::temp_dir().join(format!("dirustat-fixtures-nonempty-{}", std::process::id()));
        assert!(fs::create_dir_all(&tmp).is_ok());
        assert!(fs::write(tmp.join("keep.txt"), b"x").is_ok());
        let err = create(&tmp, 10, 1).err().map(|e| e.kind());
        assert!(fs::remove_dir_all(&tmp).is_ok());
        assert_eq!(err, Some(io::ErrorKind::AlreadyExists));
    }
}
