//! [`FakePlatform`]: an in-memory [`PlatformServices`] for tests of other crates. It records every
//! call, serves a configurable drive list and "recycles" by moving into a private temporary
//! directory, so no test ever touches the real recycle bin or launches a window.

use crate::error::{PlatformError, Result};
use crate::path::{require_exists, validate_absolute};
use crate::{DriveInfo, PlatformServices, SpaceInfo, sanitize_drives};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// One recorded service call (the path is as given by the caller).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformCall {
    Drives,
    FreeSpace(PathBuf),
    MoveToRecycleBin(PathBuf),
    Reveal(PathBuf),
    Properties(PathBuf),
    OpenDefault(PathBuf),
    Rename(PathBuf, PathBuf),
    PickFolder,
    /// `pick_save_file(suggested_name, extension)`.
    PickSaveFile(String, String),
    /// `pick_open_file(extension)`.
    PickOpenFile(String),
}

static NEXT_BIN: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
struct State {
    calls: Vec<PlatformCall>,
    drives: Vec<DriveInfo>,
    space: Option<SpaceInfo>,
    fail_with: Option<PlatformError>,
    binned: Vec<PathBuf>,
    next_item: u64,
    /// Behave like a volume without a recycle bin (network, removable).
    no_bin: bool,
    /// Report a successful recycle without moving anything (a misbehaving shell).
    leave_in_place: bool,
    /// Answer of `pick_folder` (`None` = the dialog was cancelled).
    picked: Option<PathBuf>,
    /// Answer of `pick_save_file` (`None` = the dialog was cancelled).
    picked_save: Option<PathBuf>,
    /// Answer of `pick_open_file` (`None` = the dialog was cancelled).
    picked_open: Option<PathBuf>,
}

/// Fake services. Input is validated like the real implementation, then the call is recorded.
pub struct FakePlatform {
    state: Mutex<State>,
    bin: PathBuf,
    owns_bin: bool,
}

impl fmt::Debug for FakePlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakePlatform").field("bin", &self.bin).finish_non_exhaustive()
    }
}

impl Default for FakePlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl FakePlatform {
    /// A fake with one drive `Z:\` (100 total, 40 free) and a fresh private "bin" directory under
    /// the system temp dir, removed on drop. The directory is created lazily by the first recycle.
    pub fn new() -> Self {
        let n = NEXT_BIN.fetch_add(1, Ordering::Relaxed);
        let bin = std::env::temp_dir().join(format!("dirustat-fake-bin-{}-{n}", std::process::id()));
        let mut fake = Self::with_bin_dir(bin);
        fake.owns_bin = true;
        fake
    }

    /// Like [`FakePlatform::new`] but recycling into `bin` (not removed on drop).
    pub fn with_bin_dir(bin: PathBuf) -> Self {
        let drives = vec![DriveInfo { path: "Z:\\".to_owned(), label: "Fake".to_owned(), total: 100, free: 40 }];
        Self { state: Mutex::new(State { drives, ..State::default() }), bin, owns_bin: false }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Replaces the drive list returned by `drives`.
    pub fn set_drives(&self, drives: Vec<DriveInfo>) {
        self.lock().drives = drives;
    }

    /// Fixes the answer of `free_space` (default: `total 100, free 40`).
    pub fn set_space(&self, space: SpaceInfo) {
        self.lock().space = Some(space);
    }

    /// Makes every following service call fail with `err` (after recording it); `None` clears it.
    pub fn set_failure(&self, err: Option<PlatformError>) {
        self.lock().fail_with = err;
    }

    /// Fixes the answer of `pick_folder`: the "chosen" folder, or `None` for a cancelled dialog.
    pub fn set_picked_folder(&self, folder: Option<PathBuf>) {
        self.lock().picked = folder;
    }

    /// Fixes the answer of `pick_save_file`: the "chosen" file, or `None` for a cancelled dialog.
    pub fn set_picked_save_file(&self, file: Option<PathBuf>) {
        self.lock().picked_save = file;
    }

    /// Fixes the answer of `pick_open_file`: the "chosen" file, or `None` for a cancelled dialog.
    pub fn set_picked_open_file(&self, file: Option<PathBuf>) {
        self.lock().picked_open = file;
    }

    /// Makes `move_to_recycle_bin` fail with `Unsupported` like a volume that has no recycle bin.
    pub fn set_no_recycle_bin(&self, no_bin: bool) {
        self.lock().no_bin = no_bin;
    }

    /// Makes `move_to_recycle_bin` report success without moving the item (a shell that lied).
    pub fn set_leave_in_place(&self, leave: bool) {
        self.lock().leave_in_place = leave;
    }

    /// All calls so far, oldest first.
    pub fn calls(&self) -> Vec<PlatformCall> {
        self.lock().calls.clone()
    }

    /// Items moved into the fake bin so far (their locations inside the bin directory).
    pub fn binned(&self) -> Vec<PathBuf> {
        self.lock().binned.clone()
    }

    /// The directory that receives "recycled" items.
    pub fn bin_dir(&self) -> &Path {
        &self.bin
    }

    /// Records `call`, then returns the injected failure if one is set.
    fn begin(&self, call: PlatformCall) -> Result<()> {
        let mut st = self.lock();
        st.calls.push(call);
        st.fail_with.clone().map_or(Ok(()), Err)
    }
}

impl Drop for FakePlatform {
    fn drop(&mut self) {
        if self.owns_bin {
            // Only the unique directory this fake created itself; never a caller-supplied path.
            let _ = std::fs::remove_dir_all(&self.bin);
        }
    }
}

impl PlatformServices for FakePlatform {
    fn drives(&self) -> Result<Vec<DriveInfo>> {
        self.begin(PlatformCall::Drives)?;
        Ok(sanitize_drives(self.lock().drives.clone()))
    }

    fn free_space(&self, path: &Path) -> Result<SpaceInfo> {
        validate_absolute(path)?;
        self.begin(PlatformCall::FreeSpace(path.to_path_buf()))?;
        Ok(self.lock().space.unwrap_or(SpaceInfo { total: 100, free: 40 }))
    }

    fn move_to_recycle_bin(&self, path: &Path) -> Result<()> {
        let text = validate_absolute(path)?;
        self.begin(PlatformCall::MoveToRecycleBin(path.to_path_buf()))?;
        require_exists(path, text)?;
        let item = {
            let mut st = self.lock();
            if st.no_bin {
                return Err(PlatformError::Unsupported("no recycle bin for this volume"));
            }
            if st.leave_in_place {
                return Ok(());
            }
            st.next_item = st.next_item.saturating_add(1);
            st.next_item
        };
        std::fs::create_dir_all(&self.bin).map_err(|e| PlatformError::from_io("create bin", text, &e))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("item");
        let target = self.bin.join(format!("{item}-{name}"));
        std::fs::rename(path, &target).map_err(|e| PlatformError::from_io("recycle", text, &e))?;
        self.lock().binned.push(target);
        Ok(())
    }

    fn rename_no_replace(&self, from: &Path, to: &Path) -> Result<()> {
        let from_text = validate_absolute(from)?;
        validate_absolute(to)?;
        self.begin(PlatformCall::Rename(from.to_path_buf(), to.to_path_buf()))?;
        require_exists(from, from_text)?;
        if to.symlink_metadata().is_ok() {
            return Err(PlatformError::Failed {
                op: "move",
                message: "the destination already exists; nothing was overwritten".to_owned(),
            });
        }
        std::fs::rename(from, to).map_err(|e| PlatformError::from_io("move", from_text, &e))
    }

    fn reveal_in_file_manager(&self, path: &Path) -> Result<()> {
        validate_absolute(path)?;
        self.begin(PlatformCall::Reveal(path.to_path_buf()))
    }

    fn show_properties(&self, path: &Path) -> Result<()> {
        validate_absolute(path)?;
        self.begin(PlatformCall::Properties(path.to_path_buf()))
    }

    fn open_default(&self, path: &Path) -> Result<()> {
        validate_absolute(path)?;
        self.begin(PlatformCall::OpenDefault(path.to_path_buf()))
    }

    fn pick_folder(&self) -> Result<Option<PathBuf>> {
        self.begin(PlatformCall::PickFolder)?;
        Ok(self.lock().picked.clone())
    }

    fn pick_save_file(&self, suggested_name: &str, extension: &str) -> Result<Option<PathBuf>> {
        self.begin(PlatformCall::PickSaveFile(suggested_name.to_owned(), extension.to_owned()))?;
        Ok(self.lock().picked_save.clone())
    }

    fn pick_open_file(&self, extension: &str) -> Result<Option<PathBuf>> {
        self.begin(PlatformCall::PickOpenFile(extension.to_owned()))?;
        Ok(self.lock().picked_open.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_folder_serves_the_configured_answer() {
        let fake = FakePlatform::new();
        assert_eq!(fake.pick_folder().unwrap(), None, "a cancelled dialog by default");
        fake.set_picked_folder(Some(PathBuf::from("C:\\data")));
        assert_eq!(fake.pick_folder().unwrap(), Some(PathBuf::from("C:\\data")));
        assert_eq!(fake.calls(), vec![PlatformCall::PickFolder, PlatformCall::PickFolder]);
        fake.set_failure(Some(PlatformError::Unsupported("pick_folder")));
        assert!(fake.pick_folder().is_err());
    }

    #[test]
    fn records_calls_and_serves_drives() {
        let fake = FakePlatform::new();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fake.drives().unwrap().len(), 1);
        fake.reveal_in_file_manager(dir.path()).unwrap();
        fake.show_properties(dir.path()).unwrap();
        assert_eq!(fake.free_space(dir.path()).unwrap(), SpaceInfo { total: 100, free: 40 });
        assert_eq!(
            fake.calls(),
            vec![
                PlatformCall::Drives,
                PlatformCall::Reveal(dir.path().to_path_buf()),
                PlatformCall::Properties(dir.path().to_path_buf()),
                PlatformCall::FreeSpace(dir.path().to_path_buf()),
            ]
        );
    }

    #[test]
    fn recycle_moves_into_private_bin_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let bin;
        {
            let fake = FakePlatform::new();
            bin = fake.bin_dir().to_path_buf();
            fake.move_to_recycle_bin(&file).unwrap();
            assert!(!file.exists());
            let binned = fake.binned();
            assert_eq!(binned.len(), 1);
            assert!(binned.iter().all(|p| p.starts_with(&bin) && p.exists()));
            assert!(matches!(fake.move_to_recycle_bin(&file), Err(PlatformError::NotFound(_))));
        }
        assert!(!bin.exists());
    }

    #[test]
    fn hostile_paths_and_injected_failures() {
        let fake = FakePlatform::new();
        for bad in ["", "rel", "a\0b"] {
            assert!(matches!(fake.reveal_in_file_manager(Path::new(bad)), Err(PlatformError::InvalidPath(_))));
            assert!(matches!(fake.move_to_recycle_bin(Path::new(bad)), Err(PlatformError::InvalidPath(_))));
        }
        assert!(fake.calls().is_empty());
        fake.set_failure(Some(PlatformError::Unsupported("test")));
        assert_eq!(fake.drives(), Err(PlatformError::Unsupported("test")));
        fake.set_failure(None);
        assert!(fake.drives().is_ok());
    }

    #[test]
    fn no_bin_and_lying_shell_behaviours() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let fake = FakePlatform::new();
        fake.set_no_recycle_bin(true);
        assert!(matches!(fake.move_to_recycle_bin(&file), Err(PlatformError::Unsupported(_))));
        assert!(file.exists());
        fake.set_no_recycle_bin(false);
        fake.set_leave_in_place(true);
        assert_eq!(fake.move_to_recycle_bin(&file), Ok(()));
        assert!(file.exists() && fake.binned().is_empty());
    }

    #[test]
    fn rename_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (dir.path().join("a"), dir.path().join("b"), dir.path().join("c"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let fake = FakePlatform::new();
        assert!(matches!(fake.rename_no_replace(&a, &b), Err(PlatformError::Failed { .. })));
        assert_eq!(std::fs::read(&b).unwrap(), b"b");
        assert_eq!(fake.rename_no_replace(&a, &c), Ok(()));
        assert!(!a.exists() && c.exists());
        assert!(matches!(fake.rename_no_replace(&a, &c), Err(PlatformError::NotFound(_))));
    }

    #[test]
    fn drive_list_is_sanitized() {
        let fake = FakePlatform::new();
        fake.set_drives(vec![DriveInfo { path: "Q:\\".into(), label: String::new(), total: 1, free: 5 }]);
        assert_eq!(fake.drives().unwrap().first().map(|d| d.free), Some(1));
    }
}
