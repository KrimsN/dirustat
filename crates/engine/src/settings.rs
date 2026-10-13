//! Persistent user settings (G-011): a small serde struct with a schema version, loaded and saved
//! without ever panicking or losing data.
//!
//! Loading never fails: a missing file gives defaults silently, a corrupt, oversized or unreadable
//! file gives defaults plus a warning (and a best-effort `.bad` copy of the corrupt file, so a later
//! save cannot destroy it). Saving is atomic: write a temp file next to the target, then rename.

use dirustat_model::SizeMode;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Current settings schema version.
pub const SETTINGS_VERSION: u32 = 1;
/// Maximum number of remembered paths.
pub const MAX_RECENT_PATHS: usize = 20;
/// Longest accepted remembered path, in bytes.
pub const MAX_PATH_LEN: usize = 4096;
/// Settings files larger than this are not read.
pub const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// Serializable name of a [`SizeMode`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeModeName {
    Logical,
    #[default]
    Allocated,
}

impl SizeModeName {
    pub fn mode(self) -> SizeMode {
        match self {
            Self::Logical => SizeMode::Logical,
            Self::Allocated => SizeMode::Allocated,
        }
    }

    pub fn from_mode(mode: SizeMode) -> Self {
        match mode {
            SizeMode::Logical => Self::Logical,
            SizeMode::Allocated => Self::Allocated,
        }
    }
}

/// Column a list is sorted by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Size,
    Name,
    Count,
}

/// Sort setting of a list (extension panel, tree panel).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SortSetting {
    pub key: SortKey,
    pub descending: bool,
}

impl Default for SortSetting {
    fn default() -> Self {
        Self { key: SortKey::Size, descending: true }
    }
}

/// Treemap display options (placeholder set; the layout does not read them yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreemapSettings {
    pub cushion: bool,
    pub grid: bool,
    pub show_free_space: bool,
}

impl Default for TreemapSettings {
    fn default() -> Self {
        Self { cushion: true, grid: false, show_free_space: false }
    }
}

/// All persisted user settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub size_mode: SizeModeName,
    pub ext_sort: SortSetting,
    pub tree_sort: SortSetting,
    pub treemap: TreemapSettings,
    /// Most recently scanned paths, newest first, at most [`MAX_RECENT_PATHS`].
    pub recent_paths: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            size_mode: SizeModeName::default(),
            ext_sort: SortSetting::default(),
            tree_sort: SortSetting::default(),
            treemap: TreemapSettings::default(),
            recent_paths: Vec::new(),
        }
    }
}

/// Result of [`Settings::load_from`]: always usable settings plus an optional warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOutcome {
    pub settings: Settings,
    /// Set when the file existed but could not be fully used.
    pub warning: Option<String>,
}

/// Error from [`Settings::save_to`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "settings: {}", self.0)
    }
}

impl std::error::Error for SettingsError {}

impl Settings {
    /// Normalizes untrusted values: current version, bounded and de-duplicated recent paths.
    pub fn sanitize(&mut self) {
        self.version = SETTINGS_VERSION;
        let mut kept: Vec<String> = Vec::new();
        for p in std::mem::take(&mut self.recent_paths) {
            if p.is_empty() || p.len() > MAX_PATH_LEN || kept.contains(&p) {
                continue;
            }
            if kept.len() >= MAX_RECENT_PATHS {
                break;
            }
            kept.push(p);
        }
        self.recent_paths = kept;
    }

    /// Moves `path` to the front of the recent list (ignored when empty or too long).
    pub fn push_recent(&mut self, path: &str) {
        if path.is_empty() || path.len() > MAX_PATH_LEN {
            return;
        }
        self.recent_paths.retain(|p| p != path);
        self.recent_paths.insert(0, path.to_owned());
        self.recent_paths.truncate(MAX_RECENT_PATHS);
    }

    /// Loads settings from `path`. Never fails and never panics; see the module docs.
    pub fn load_from(path: &Path) -> LoadOutcome {
        let defaults = |warning: Option<String>| LoadOutcome { settings: Self::default(), warning };
        let file = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return defaults(None),
            Err(e) => return defaults(Some(format!("cannot open {}: {e}", path.display()))),
        };
        let mut bytes = Vec::new();
        if let Err(e) = file.take(MAX_SETTINGS_BYTES.saturating_add(1)).read_to_end(&mut bytes) {
            return defaults(Some(format!("cannot read {}: {e}", path.display())));
        }
        if u64::try_from(bytes.len()).map_or(true, |n| n > MAX_SETTINGS_BYTES) {
            return defaults(Some(format!("{} is larger than {MAX_SETTINGS_BYTES} bytes", path.display())));
        }
        match serde_json::from_slice::<Settings>(&bytes) {
            Ok(mut settings) => {
                let newer = settings.version > SETTINGS_VERSION;
                settings.sanitize();
                let warning =
                    newer.then(|| "settings were written by a newer version; unknown data ignored".to_owned());
                LoadOutcome { settings, warning }
            }
            Err(e) => {
                let mut backup = path.as_os_str().to_owned();
                backup.push(".bad");
                // Best effort: keep the corrupt file so a later save cannot destroy it.
                let _ = fs::copy(path, PathBuf::from(backup));
                defaults(Some(format!("corrupt settings file {}: {e}", path.display())))
            }
        }
    }

    /// Saves atomically: temp file in the same directory, flushed, then renamed over `path`.
    pub fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        let err = |what: &str, e: std::io::Error| SettingsError(format!("{what} {}: {e}", path.display()));
        let mut clean = self.clone();
        clean.sanitize();
        let json = serde_json::to_vec_pretty(&clean).map_err(|e| SettingsError(e.to_string()))?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| err("cannot create directory for", e))?;
        }
        let mut tmp_name = path.as_os_str().to_owned();
        tmp_name.push(format!(".{}.tmp", std::process::id()));
        let tmp = PathBuf::from(tmp_name);
        let written = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(err("cannot write", e));
        }
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(err("cannot replace", e));
        }
        Ok(())
    }
}

/// Per-OS settings directory (`%APPDATA%\dirustat`, `~/Library/Application Support/diRUSTat`,
/// `$XDG_CONFIG_HOME/dirustat` or `~/.config/dirustat`). `None` when the environment has no home.
pub fn default_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("APPDATA").map(|p| p.join("dirustat"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|p| p.join("Library").join("Application Support").join("diRUSTat"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|p| p.join(".config"))).map(|p| p.join("dirustat"))
    }
}

/// Default settings file: `default_dir()/settings.json`.
pub fn default_path() -> Option<PathBuf> {
    default_dir().map(|d| d.join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults_without_warning() {
        let dir = tempfile::tempdir().unwrap();
        let out = Settings::load_from(&dir.path().join("none.json"));
        assert_eq!(out, LoadOutcome { settings: Settings::default(), warning: None });
    }

    #[test]
    fn round_trip_and_atomic_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");
        let mut s = Settings { size_mode: SizeModeName::Logical, ..Settings::default() };
        s.treemap.grid = true;
        s.push_recent("C:\\a");
        s.save_to(&path).unwrap();
        s.push_recent("C:\\b");
        s.save_to(&path).unwrap();
        let out = Settings::load_from(&path);
        assert_eq!(out.warning, None);
        assert_eq!(out.settings, s);
        let names: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(names.len(), 1, "no temp file left behind");
    }

    #[test]
    fn corrupt_file_gives_defaults_warning_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for junk in [&b"{not json"[..], b"\xff\xfe\x00", b"", b"[1,2]", b"{\"size_mode\": 5}"] {
            fs::write(&path, junk).unwrap();
            let out = Settings::load_from(&path);
            assert_eq!(out.settings, Settings::default());
            assert!(out.warning.is_some());
            assert_eq!(fs::read(dir.path().join("settings.json.bad")).unwrap(), junk);
        }
    }

    #[test]
    fn oversized_file_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let big = vec![b' '; usize::try_from(MAX_SETTINGS_BYTES).unwrap() + 10];
        fs::write(&path, big).unwrap();
        let out = Settings::load_from(&path);
        assert!(out.warning.is_some());
        assert_eq!(out.settings, Settings::default());
    }

    #[test]
    fn directory_in_place_of_file_is_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let out = Settings::load_from(dir.path());
        assert!(out.warning.is_some());
        assert!(Settings::default().save_to(dir.path()).is_err());
    }

    #[test]
    fn partial_and_future_files_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, br#"{"size_mode":"logical","future_key":1,"version":99}"#).unwrap();
        let out = Settings::load_from(&path);
        assert_eq!(out.settings.size_mode, SizeModeName::Logical);
        assert_eq!(out.settings.version, SETTINGS_VERSION);
        assert!(out.warning.is_some());
    }

    #[test]
    fn recent_paths_are_bounded_and_deduplicated() {
        let mut s = Settings::default();
        for i in 0..100 {
            s.push_recent(&format!("p{i}"));
        }
        s.push_recent("p99");
        s.push_recent(&"x".repeat(MAX_PATH_LEN + 1));
        s.push_recent("");
        assert_eq!(s.recent_paths.len(), MAX_RECENT_PATHS);
        assert_eq!(s.recent_paths.first().map(String::as_str), Some("p99"));
        let mut hostile =
            Settings { recent_paths: (0..1000).map(|i| (i % 50).to_string()).collect(), ..Settings::default() };
        hostile.recent_paths.push(String::new());
        hostile.sanitize();
        assert_eq!(hostile.recent_paths.len(), MAX_RECENT_PATHS);
    }

    #[test]
    fn default_dir_does_not_panic() {
        let _ = default_dir();
        let _ = default_path();
    }
}
