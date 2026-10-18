//! The real [`PlatformServices`] for the current OS.

use crate::error::{PlatformError, Result};
use crate::{DriveInfo, PlatformServices, SpaceInfo};
use std::path::Path;

/// Real services: full on Windows, `Unsupported` elsewhere (after validating the input, so a
/// hostile path is reported the same way on every OS).
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPlatform;

#[cfg(windows)]
impl PlatformServices for SystemPlatform {
    fn drives(&self) -> Result<Vec<DriveInfo>> {
        Ok(crate::sanitize_drives(crate::win32::drives()))
    }

    fn free_space(&self, path: &Path) -> Result<SpaceInfo> {
        let text = crate::path::validate_absolute(path)?;
        crate::path::require_exists(path, text)?;
        crate::win32::free_space(text)
            .ok_or_else(|| PlatformError::Failed { op: "free_space", message: "GetDiskFreeSpaceExW failed".to_owned() })
    }

    fn move_to_recycle_bin(&self, path: &Path) -> Result<()> {
        let text = crate::path::validate_absolute(path)?;
        crate::path::require_exists(path, text)?;
        trash::delete(path).map_err(|err| PlatformError::Failed { op: "recycle", message: err.to_string() })
    }

    fn reveal_in_file_manager(&self, path: &Path) -> Result<()> {
        use std::os::windows::process::CommandExt;
        let text = crate::path::validate_absolute(path)?;
        crate::path::require_exists(path, text)?;
        let arg = crate::path::explorer_select_arg(text)?;
        // No shell: `explorer.exe` is started directly. It always exits with 1, so the exit code is
        // not inspected and the child is not waited for.
        std::process::Command::new("explorer.exe")
            .raw_arg(arg)
            .spawn()
            .map(drop)
            .map_err(|err| PlatformError::from_io("explorer", text, &err))
    }

    fn show_properties(&self, path: &Path) -> Result<()> {
        let text = crate::path::validate_absolute(path)?;
        crate::path::require_exists(path, text)?;
        if crate::win32::show_properties(text) {
            Ok(())
        } else {
            Err(PlatformError::Failed { op: "properties", message: "ShellExecuteExW failed".to_owned() })
        }
    }
}

#[cfg(not(windows))]
impl PlatformServices for SystemPlatform {
    fn drives(&self) -> Result<Vec<DriveInfo>> {
        Err(PlatformError::Unsupported("drives"))
    }

    fn free_space(&self, path: &Path) -> Result<SpaceInfo> {
        crate::path::validate_absolute(path)?;
        Err(PlatformError::Unsupported("free_space"))
    }

    fn move_to_recycle_bin(&self, path: &Path) -> Result<()> {
        crate::path::validate_absolute(path)?;
        Err(PlatformError::Unsupported("move_to_recycle_bin"))
    }

    fn reveal_in_file_manager(&self, path: &Path) -> Result<()> {
        crate::path::validate_absolute(path)?;
        Err(PlatformError::Unsupported("reveal_in_file_manager"))
    }

    fn show_properties(&self, path: &Path) -> Result<()> {
        crate::path::validate_absolute(path)?;
        Err(PlatformError::Unsupported("show_properties"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn drives_are_sane() {
        let drives = SystemPlatform.drives().unwrap();
        assert!(!drives.is_empty());
        for d in &drives {
            assert!(d.total >= d.free, "{d:?}");
            assert!(d.path.ends_with('\\'));
        }
    }

    #[test]
    fn free_space_of_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = SystemPlatform.free_space(dir.path()).unwrap();
        assert!(s.total > 0 && s.total >= s.free);
    }
}
