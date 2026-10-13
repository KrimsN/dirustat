//! Synthetic demo data for screenshots and tests. Never reads a real disk: the tree is generated
//! in a fresh temporary directory (sparse files of fixed sizes), scanned, relabelled with a neutral
//! root name and removed again.

use dirustat_engine::Session;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// (relative path, size in bytes) of the generated files.
const FILES: &[(&str, u64)] = &[
    ("media/video/holiday.mp4", 48_000_000),
    ("media/video/intro.mkv", 22_000_000),
    ("media/photos/img_0001.jpg", 4_200_000),
    ("media/photos/img_0002.jpg", 3_900_000),
    ("media/photos/img_0003.png", 6_100_000),
    ("media/music/album.flac", 31_000_000),
    ("media/music/single.mp3", 7_500_000),
    ("projects/app/src/main.rs", 21_000),
    ("projects/app/src/lib.rs", 54_000),
    ("projects/app/target/debug/app.exe", 18_000_000),
    ("projects/app/target/debug/app.pdb", 9_500_000),
    ("projects/app/Cargo.lock", 64_000),
    ("projects/notes/todo.md", 8_000),
    ("projects/notes/design.pdf", 2_400_000),
    ("docs/manual.pdf", 5_800_000),
    ("docs/report.docx", 1_300_000),
    ("docs/data.csv", 3_100_000),
    ("downloads/setup.zip", 27_000_000),
    ("downloads/tools.7z", 12_000_000),
    ("downloads/readme.txt", 3_000),
    ("system/cache/blob1.bin", 14_000_000),
    ("system/cache/blob2.bin", 9_000_000),
    ("system/logs/app.log", 2_700_000),
    ("system/logs/old.log", 1_100_000),
];

/// Writes the demo files below `root` (which must exist). Errors are returned, never panics.
pub fn write_demo_files(root: &Path) -> std::io::Result<()> {
    for (rel, size) in FILES {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create(&path)?.set_len(*size)?;
    }
    Ok(())
}

fn fresh_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("dirustat-demo-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Builds a session holding the synthetic demo tree. The root is renamed to [`DEMO_ROOT_NAME`].
pub fn demo_session() -> Result<Session, String> {
    let dir = fresh_dir().map_err(|e| format!("cannot create demo directory: {e}"))?;
    let result = scan_demo(&dir);
    // Only removes the directory created above.
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Label of the demo root, so screenshots never show the temporary directory path.
pub const DEMO_ROOT_NAME: &str = "demo";

fn scan_demo(dir: &Path) -> Result<Session, String> {
    write_demo_files(dir).map_err(|e| format!("cannot write demo files: {e}"))?;
    let handle = dirustat_scan::start(dir, dirustat_scan::ScanOptions::default()).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while !handle.is_finished() {
        if Instant::now() > deadline {
            handle.cancel();
            return Err("demo scan timed out".to_owned());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut tree = handle.join().map_err(|e| e.to_string())?.tree;
    let root = tree.root();
    tree.set_node_name(root, DEMO_ROOT_NAME).map_err(|e| e.to_string())?;
    let mut session = Session::new();
    session.replace_tree(tree);
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_session_has_a_tree_and_cleans_up() {
        let session = demo_session();
        assert!(session.is_ok(), "{:?}", session.as_ref().err());
        let Ok(session) = session else { return };
        assert!(session.tree().len() > FILES.len());
        assert!(session.revision() >= 1);
        assert_eq!(session.tree().get(session.tree().root()).map(|n| n.name()), Some(DEMO_ROOT_NAME));
    }
}
