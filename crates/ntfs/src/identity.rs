//! File identity for hardlink de-duplication on the portable scan path.
//!
//! `std` exposes the volume serial, file index and link count only through unstable `MetadataExt`
//! methods (still unstable on stable Rust 1.96), so on Windows this module opens the file for attribute
//! queries only (access mask 0, no data access) and asks `GetFileInformationByHandle` through the
//! isolated FFI in `win32.rs`. The public function is safe. On other platforms it returns `None` (the
//! scanner uses `st_dev`/`st_ino` there).

use std::path::Path;

/// Identity of a file on its volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    /// Volume serial number.
    pub volume_serial: u64,
    /// 64-bit file index (the same for every hardlink of one file).
    pub file_index: u64,
    /// Number of hardlinks to the file.
    pub links: u32,
}

/// Reads the identity of the file at `path` without touching its data; `None` when it cannot be queried.
#[cfg(windows)]
pub fn file_identity(path: &Path) -> Option<FileIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
    const SHARE_ALL: u32 = 0x7;
    let file = std::fs::OpenOptions::new().access_mode(0).share_mode(SHARE_ALL).open(path).ok()?;
    let info = crate::win32::file_info_by_handle(file.as_raw_handle()).ok()?;
    Some(FileIdentity { volume_serial: u64::from(info.volume_serial), file_index: info.file_index, links: info.links })
}

/// Not available off Windows.
#[cfg(not(windows))]
pub fn file_identity(_path: &Path) -> Option<FileIdentity> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn hardlinks_share_an_identity() {
        let dir = std::env::temp_dir().join(format!("dirustat-identity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        std::fs::write(&c, b"y").unwrap();
        let (ia, ib, ic) = (file_identity(&a).unwrap(), file_identity(&b).unwrap(), file_identity(&c).unwrap());
        assert_eq!(ia, ib);
        assert_eq!(ia.links, 2);
        assert_ne!(ia.file_index, ic.file_index);
        assert_eq!(ic.links, 1);
        assert!(file_identity(&dir.join("missing")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
