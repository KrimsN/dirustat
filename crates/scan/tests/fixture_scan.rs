//! Scan integration test on a generated fixture. Ignored by default: it needs a tree created by
//! `cargo xtask fixtures` and the path in `DIRUSTAT_FIXTURE`.
//!
//! The path must be absolute (cargo runs tests from the crate directory).
//!
//! ```sh
//! cargo xtask fixtures --size 100k --seed 1 --out fixtures-local/100k-seed1
//! DIRUSTAT_FIXTURE=$PWD/fixtures-local/100k-seed1 cargo test -p dirustat-scan --release --test fixture_scan -- --ignored
//! ```

use std::fs;
use std::path::PathBuf;

use dirustat_model::SizeMode;
use dirustat_scan::{ScanOptions, start};

/// Independent iterative walk with plain `std`: (dirs including root, files, logical bytes).
fn reference_walk(root: &std::path::Path) -> (u64, u64, u64) {
    let (mut dirs, mut files, mut bytes) = (1u64, 0u64, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(iter) = fs::read_dir(&dir) else { continue };
        for entry in iter.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.file_type().is_symlink() || is_reparse(&meta) {
                files += 1;
            } else if meta.is_dir() {
                dirs += 1;
                stack.push(entry.path());
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (dirs, files, bytes)
}

#[cfg(windows)]
fn is_reparse(meta: &fs::Metadata) -> bool {
    std::os::windows::fs::MetadataExt::file_attributes(meta) & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(_meta: &fs::Metadata) -> bool {
    false
}

#[test]
#[ignore = "needs DIRUSTAT_FIXTURE pointing at a `cargo xtask fixtures` tree"]
fn scan_matches_reference_walk_on_fixture() {
    let Some(path) = std::env::var_os("DIRUSTAT_FIXTURE").map(PathBuf::from) else {
        panic!("set DIRUSTAT_FIXTURE to a fixture directory");
    };
    let started = std::time::Instant::now();
    let out = start(&path, ScanOptions::default()).unwrap().join().unwrap();
    let elapsed = started.elapsed();
    assert!(!out.cancelled);

    let root = out.tree.get(out.tree.root()).unwrap();
    let (dirs, files, bytes) = reference_walk(&path);
    assert_eq!(root.dir_count(), dirs);
    assert_eq!(root.file_count(), files);
    assert_eq!(root.error_count(), 0);
    // The fixture has hardlinks only on Unix dedup paths; on Windows every name is counted.
    assert!(root.total_size(SizeMode::Logical) <= bytes);
    println!(
        "scanned {} nodes in {:.2?} ({:.0} nodes/s)",
        out.tree.len(),
        elapsed,
        out.tree.len() as f64 / elapsed.as_secs_f64()
    );
}
