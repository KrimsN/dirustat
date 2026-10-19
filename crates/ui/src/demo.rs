//! Synthetic demo data for screenshots and tests. Never reads a real disk: the tree is generated
//! in a fresh temporary directory (sparse files of fixed sizes), scanned, relabelled with a neutral
//! root name and removed again.
//!
//! The default demo has no files behind it, so every delete is refused up front with an honest
//! message. `DIRUSTAT_DEMO_DELETABLE=1` builds a [`DemoFixture`] instead: the same synthetic files
//! stay in a private temporary directory for the lifetime of the app, the session scans it like any
//! folder and uses `FakePlatform` (the "recycle bin" is a private temp directory too), so the
//! delete dialogs, cleanups and the journal can be exercised for real without touching user data.
//! The engine protections are not relaxed in any way; the fixture is just an ordinary folder.

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

const FIXTURE_PREFIX: &str = "dirustat-demo-";

fn fresh_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let dir = std::env::temp_dir().join(format!("{FIXTURE_PREFIX}{}-{nanos}", std::process::id()));
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

/// The private directory of a deletable demo; removed when dropped (and so when the app exits).
#[derive(Debug)]
pub struct DemoFixture {
    dir: PathBuf,
}

impl DemoFixture {
    /// The fixture folder (the scan root of the demo session).
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// True for a directory this module created: inside the system temp dir with the demo prefix.
    fn is_ours(dir: &Path) -> bool {
        dir.parent() == Some(std::env::temp_dir().as_path())
            && dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(FIXTURE_PREFIX))
    }
}

impl Drop for DemoFixture {
    fn drop(&mut self) {
        // Only ever removes the unique directory created by `fresh_dir`.
        if Self::is_ours(&self.dir) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Builds a session that scans a real, disposable fixture folder (see the module docs). The root
/// shows the temporary path, so screenshots of this mode are not path-neutral.
pub fn deletable_demo_session() -> Result<(Session, DemoFixture), String> {
    let dir = fresh_dir().map_err(|e| format!("cannot create demo directory: {e}"))?;
    let fixture = DemoFixture { dir };
    write_demo_files(fixture.path()).map_err(|e| format!("cannot write demo files: {e}"))?;
    let mut session = Session::new();
    session.set_platform(demo_platform());
    session.start_scan(&fixture.path().to_string_lossy()).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(60);
    while session.is_scanning() {
        if Instant::now() > deadline {
            session.cancel_scan();
            return Err("demo scan timed out".to_owned());
        }
        session.poll_scan().map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok((session, fixture))
}

/// Synthetic volumes and free space, so demo screenshots show no real drive and the demo can never
/// launch the file manager or touch the recycle bin.
fn demo_platform() -> std::sync::Arc<dyn dirustat_platform::PlatformServices> {
    use dirustat_platform::{DriveInfo, FakePlatform, SpaceInfo};
    const GIB: u64 = 1 << 30;
    let fake = FakePlatform::new();
    fake.set_drives(vec![
        DriveInfo { path: "C:\\".to_owned(), label: "System".to_owned(), total: 476 * GIB, free: 118 * GIB },
        DriveInfo { path: "D:\\".to_owned(), label: "Data".to_owned(), total: 1863 * GIB, free: 1204 * GIB },
        DriveInfo { path: "E:\\".to_owned(), label: "Backup".to_owned(), total: 931 * GIB, free: 22 * GIB },
    ]);
    fake.set_space(SpaceInfo { total: 476 * GIB, free: 118 * GIB });
    std::sync::Arc::new(fake)
}

/// A session without a tree and with the synthetic platform: the start screen with the drive picker
/// for screenshots (`DIRUSTAT_DEMO_EMPTY=1 dirustat --demo`).
pub fn empty_demo_session() -> Session {
    let mut session = Session::new();
    session.set_platform(demo_platform());
    session
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
    session.set_platform(demo_platform());
    session.replace_tree(tree);
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletable_demo_scans_a_real_folder_and_cleans_up_on_drop() {
        let built = deletable_demo_session();
        assert!(built.is_ok(), "{:?}", built.as_ref().err());
        let Ok((mut session, fixture)) = built else { return };
        let dir = fixture.path().to_path_buf();
        assert!(dir.is_dir());
        assert!(session.tree().len() > FILES.len());
        // The root is the real folder, so the engine's path rules apply as for any scan.
        let root = session.node_path(session.tree().root()).unwrap_or_default();
        assert!(std::path::Path::new(&root).is_absolute(), "{root}");
        // Previewing a delete works (the engine accepts the fixture) ...
        let id = session
            .execute("search.find", &serde_json::json!({ "query": "holiday.mp4" }))
            .ok()
            .and_then(|v| v["items"][0]["id"].as_u64())
            .unwrap_or(0);
        let revision = session.revision();
        let preview = session
            .execute("file.delete_preview", &serde_json::json!({ "id": id, "revision": revision, "mode": "recycle" }));
        assert!(preview.is_ok(), "{preview:?}");
        // ... and a confirmed recycle goes to the fake bin, never to a real one.
        let done =
            session.execute("file.delete", &serde_json::json!({ "id": id, "revision": revision, "confirm": true }));
        assert!(done.is_ok(), "{done:?}");
        assert!(!dir.join("media").join("video").join("holiday.mp4").exists());
        drop(session);
        drop(fixture);
        assert!(!dir.exists(), "the fixture is removed on drop");
    }

    #[test]
    fn only_our_own_directories_are_ever_removed() {
        let other = tempfile::tempdir().unwrap();
        assert!(!DemoFixture::is_ours(other.path()));
        assert!(!DemoFixture::is_ours(&std::env::temp_dir()));
        assert!(!DemoFixture::is_ours(&std::env::temp_dir().join("something-else")));
        assert!(DemoFixture::is_ours(&std::env::temp_dir().join("dirustat-demo-1-2")));
        // Dropping a fixture that points elsewhere deletes nothing.
        let keep = other.path().join("keep.txt");
        std::fs::write(&keep, b"x").unwrap();
        drop(DemoFixture { dir: other.path().to_path_buf() });
        assert!(keep.exists());
    }

    #[test]
    fn demo_session_has_a_tree_and_cleans_up() {
        let session = demo_session();
        assert!(session.is_ok(), "{:?}", session.as_ref().err());
        let Ok(session) = session else { return };
        assert!(session.tree().len() > FILES.len());
        assert!(session.revision() >= 1);
        assert_eq!(session.tree().get(session.tree().root()).map(|n| n.name()), Some(DEMO_ROOT_NAME));
    }

    #[test]
    fn the_default_demo_explains_why_it_cannot_delete_and_the_fixture_does_not() {
        let Ok(session) = demo_session() else { return };
        assert_eq!(crate::delete_dialog::block_reason(&session), Some(crate::delete_dialog::DEMO_REASON));
        let Ok((fixture_session, _fixture)) = deletable_demo_session() else { return };
        assert_eq!(crate::delete_dialog::block_reason(&fixture_session), None);
    }
}
