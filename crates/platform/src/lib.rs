//! L1: platform services behind the [`PlatformServices`] trait (ADR 0009).
//!
//! Drives, free space, recycle bin, file manager and the shell properties window. The real
//! implementation ([`SystemPlatform`]) works on Windows; on other OSes every call returns
//! [`PlatformError::Unsupported`], never a panic. [`FakePlatform`] records calls and never touches
//! the real recycle bin, so tests of higher layers use it.
//!
//! The only `unsafe` is in the Windows-only `win32` module (second isolated zone after
//! `dirustat-ntfs`); everything else, including path validation, is safe code.

mod error;
mod fake;
mod path;
mod system;
#[cfg(windows)]
mod win32;

pub use error::{PlatformError, Result};
pub use fake::{FakePlatform, PlatformCall};
pub use path::{
    DRIVE_TYPE_FIXED, DRIVE_TYPE_REMOTE, MAX_PATH_UNITS, PathClass, classify_path, classify_path_for,
    classify_path_with, drive_root, drive_type_has_recycle_bin, drive_type_is_local, explorer_select_arg,
    validate_absolute, validate_local,
};
pub use system::SystemPlatform;

use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Upper bound on drives returned by [`PlatformServices::drives`].
pub const MAX_DRIVES: usize = 256;

/// One mounted volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Mount point or drive root (`C:\`, `/`).
    pub path: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

/// Capacity of the volume that holds a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub total: u64,
    /// Bytes available to the current user; never more than `total`.
    pub free: u64,
}

/// OS services the engine needs. Implementations must be cheap to call from any thread, must
/// validate their input (empty, relative, NUL, over-long paths are [`PlatformError::InvalidPath`])
/// and must never panic.
pub trait PlatformServices: Send + Sync + Debug {
    /// Mounted volumes (fixed and removable with media), at most [`MAX_DRIVES`], `free <= total`.
    fn drives(&self) -> Result<Vec<DriveInfo>>;

    /// Total and available bytes of the volume containing `path` (which must exist).
    fn free_space(&self, path: &Path) -> Result<SpaceInfo>;

    /// Moves a file or folder to the recycle bin (reversible). The only deletion primitive of the
    /// platform layer; callers own confirmation, dry-run and logging (`docs/safety.md`).
    fn move_to_recycle_bin(&self, path: &Path) -> Result<()>;

    /// Renames `from` to `to` on one volume and fails (without changing anything) when `to`
    /// already exists, with no copy fallback. Optional: defaults to `Unsupported`, so a move is
    /// refused rather than risking an overwrite where no atomic primitive exists.
    fn rename_no_replace(&self, _from: &Path, _to: &Path) -> Result<()> {
        Err(PlatformError::Unsupported("rename_no_replace"))
    }

    /// Opens the file manager with `path` selected. Launches an external window.
    fn reveal_in_file_manager(&self, path: &Path) -> Result<()>;

    /// Opens the shell properties window of `path`. Launches an external window.
    fn show_properties(&self, path: &Path) -> Result<()>;

    /// Shows the native "choose a folder" dialog and blocks until it closes: `Ok(Some(path))` for
    /// a chosen folder, `Ok(None)` for Cancel. Interactive only: call it from a helper thread, never
    /// from a command. Optional: defaults to `Unsupported`.
    fn pick_folder(&self) -> Result<Option<PathBuf>> {
        Err(PlatformError::Unsupported("pick_folder"))
    }

    /// Opens `path` with its default application. Optional: defaults to `Unsupported`.
    fn open_default(&self, _path: &Path) -> Result<()> {
        Err(PlatformError::Unsupported("open_default"))
    }
}

/// The real implementation for the current OS.
pub fn default_platform() -> Arc<dyn PlatformServices> {
    Arc::new(SystemPlatform)
}

/// Clamps a drive list to the contract: at most [`MAX_DRIVES`] entries and `free <= total`.
pub fn sanitize_drives(drives: Vec<DriveInfo>) -> Vec<DriveInfo> {
    drives
        .into_iter()
        .take(MAX_DRIVES)
        .map(|mut d| {
            d.free = d.free.min(d.total);
            d
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_caps_and_clamps() {
        let many = (0..MAX_DRIVES + 10)
            .map(|i| DriveInfo { path: format!("{i}:"), label: String::new(), total: 5, free: 9 })
            .collect();
        let out = sanitize_drives(many);
        assert_eq!(out.len(), MAX_DRIVES);
        assert!(out.iter().all(|d| d.free == 5));
    }

    #[test]
    fn default_platform_rejects_hostile_paths_without_side_effects() {
        let p = default_platform();
        for bad in ["", "rel/x", "a\0b"] {
            let path = Path::new(bad);
            assert!(matches!(p.move_to_recycle_bin(path), Err(PlatformError::InvalidPath(_))));
            assert!(matches!(p.reveal_in_file_manager(path), Err(PlatformError::InvalidPath(_))));
            assert!(matches!(p.show_properties(path), Err(PlatformError::InvalidPath(_))));
            assert!(matches!(p.free_space(path), Err(PlatformError::InvalidPath(_))));
        }
    }

    #[test]
    fn default_platform_missing_path_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let p = default_platform();
        if cfg!(windows) {
            assert!(matches!(p.move_to_recycle_bin(&missing), Err(PlatformError::NotFound(_))));
            assert!(matches!(p.reveal_in_file_manager(&missing), Err(PlatformError::NotFound(_))));
            assert!(matches!(p.show_properties(&missing), Err(PlatformError::NotFound(_))));
        }
    }
}
