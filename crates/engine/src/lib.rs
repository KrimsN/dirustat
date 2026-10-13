//! L2: `Session` and the command registry. **All user-visible behaviour is a command** (ADR 0003).
//!
//! UI, CLI, control channel and MCP invoke commands by `id`. A command has an id, label, menu group,
//! shortcut, params doc, `enabled` and `run`. A programmatic call never opens a dialog.
//!
//! Status: command registry in place (milestone M2); see `cmd`.

mod cmd;
mod drives;
mod ext;
mod guard;
mod launch;
mod map;
mod settings;

pub use cmd::{CommandInfo, CommandSpec, FastPathStatus, fast_path_status, volume_root_of};
pub use map::{MapLayout, MapState, MapStateView, MapTile};

pub use drives::{DriveInfo, MAX_DRIVES};
pub use launch::{LaunchArgs, LaunchError, USAGE as LAUNCH_USAGE};
pub use settings::{
    LoadOutcome, MAX_RECENT_PATHS, SETTINGS_VERSION, Settings, SettingsError, SizeModeName, SortKey, SortSetting,
    TreemapSettings, default_dir as settings_dir, default_path as settings_path,
};

pub use ext::{ExtStat, NO_EXT_COLOR, ext_color, extension_of};

use dirustat_model::{SizeMode, Tree};
use dirustat_scan::{ScanHandle, empty_tree};
use dirustat_treemap::{Rect, squarify};
use std::fmt;

/// Command execution error. `run` must return it instead of panicking, for any params.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    UnknownCommand(String),
    InvalidParams(String),
    Unsupported(String),
    /// A destructive command was called without `confirm: true` (`docs/safety.md`).
    ConfirmationRequired(String),
    /// The command exists but its `enabled` predicate is false in the current session state.
    NotEnabled(String),
    /// A panic was caught by the last-resort guard; the session is intact.
    Panicked {
        what: String,
        message: String,
    },
    /// The scan failed (root unreadable, thread pool, scan thread panic).
    Scan(String),
}

impl CommandError {
    /// Stable machine-readable code from `docs/control-protocol.md`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownCommand(_) => "unknown_command",
            Self::InvalidParams(_) => "invalid_params",
            Self::Unsupported(_) => "unsupported",
            Self::ConfirmationRequired(_) => "confirmation_required",
            Self::NotEnabled(_) => "not_enabled",
            Self::Panicked { .. } | Self::Scan(_) => "internal",
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "unknown command: {id}"),
            Self::InvalidParams(why) => write!(f, "invalid params: {why}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::ConfirmationRequired(id) => write!(f, "confirmation required: {id}"),
            Self::NotEnabled(id) => write!(f, "command not enabled: {id}"),
            Self::Panicked { what, message } => write!(f, "internal error in {what}: {message}"),
            Self::Scan(why) => write!(f, "scan failed: {why}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Session state: the current tree and derived data.
#[derive(Debug)]
pub struct Session {
    tree: Tree,
    /// Bumped whenever the tree is replaced (cache key for derived data).
    revision: u64,
    map: MapState,
    /// Background scan started by `scan.start`, not yet joined.
    scan: Option<PendingScan>,
    /// Persistent user settings (`settings.*` commands).
    settings: Settings,
    drives: drives::DrivesProvider,
}

/// A running scan owned by the session.
#[derive(Debug)]
struct PendingScan {
    handle: ScanHandle,
    root: String,
}

impl Session {
    pub fn new() -> Self {
        Self {
            tree: empty_tree(""),
            revision: 0,
            map: MapState::new(),
            scan: None,
            settings: Settings::default(),
            drives: drives::DrivesProvider(None),
        }
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Per-extension statistics of the current tree, recomputed on every call (one iterative pass).
    /// Callers that call it per frame should cache the result by `revision()`.
    pub fn extension_stats(&self, mode: SizeMode) -> Vec<ExtStat> {
        ext::extension_stats(&self.tree, mode)
    }

    /// Layout of the root's children inside `bounds` — a minimal model + treemap example.
    pub fn layout_root(&self, bounds: Rect) -> Vec<Rect> {
        let mut weights: Vec<u64> = self
            .tree
            .children(self.tree.root())
            .filter_map(|id| self.tree.get(id).map(|n| n.total_size(SizeMode::default())))
            .collect();
        weights.sort_unstable_by(|a, b| b.cmp(a));
        squarify(&weights, bounds)
    }

    /// Thin text wrapper over [`Session::execute`]: parses `params` as JSON (empty means no
    /// params; invalid JSON is `InvalidParams`) and discards the result value.
    pub fn run(&mut self, id: &str, params: &str) -> Result<(), CommandError> {
        let value = if params.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(params).map_err(|e| CommandError::InvalidParams(format!("invalid JSON: {e}")))?
        };
        self.execute(id, &value).map(|_| ())
    }

    /// Installs a prebuilt tree as the current one (demo data, tests); bumps the revision.
    pub fn replace_tree(&mut self, tree: Tree) {
        self.tree = tree;
        self.tree_replaced();
    }

    /// Waits for a scan and, unless it was cancelled, makes its tree current. A panic in the scan
    /// thread is reported as an error and the previous tree is kept.
    pub fn finish_scan(&mut self, handle: ScanHandle) -> Result<bool, CommandError> {
        let outcome = guard::guarded("scan", || {
            handle.join().map_err(|err| match err {
                // A bad root is the caller's mistake, found on the scan thread (see `start_scan`).
                dirustat_scan::ScanError::Root(_) | dirustat_scan::ScanError::NotADirectory => {
                    CommandError::InvalidParams(format!("cannot scan: {err}"))
                }
                other => CommandError::Scan(other.to_string()),
            })
        })?;
        if outcome.cancelled {
            return Ok(false);
        }
        self.tree = outcome.tree;
        self.tree_replaced();
        Ok(true)
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_tree_bumps_revision() {
        let mut s = Session::new();
        let before = s.revision();
        s.replace_tree(empty_tree("demo"));
        assert!(s.revision() > before);
        assert_eq!(s.tree().get(s.tree().root()).map(|n| n.name()), Some("demo"));
    }

    #[test]
    fn unknown_command_is_an_error_not_a_panic() {
        let mut s = Session::new();
        assert_eq!(s.run("nope", "{}"), Err(CommandError::UnknownCommand("nope".into())));
    }

    #[test]
    fn panicking_command_is_an_error_and_session_survives() {
        let mut s = Session::new();
        let err = s.run("test.panic", "{}").unwrap_err();
        assert!(matches!(&err, CommandError::Panicked { what, .. } if what == "test.panic"));
        assert_eq!(s.run("nope", "{}"), Err(CommandError::UnknownCommand("nope".into())));
        assert_eq!(s.tree().len(), 1);
    }

    #[test]
    fn finish_scan_replaces_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        let mut s = Session::new();
        assert_eq!(s.finish_scan(handle), Ok(true));
        assert!(s.tree().len() >= 2);
    }

    #[test]
    fn empty_session_lays_out_nothing() {
        let s = Session::new();
        assert!(s.layout_root(Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }).is_empty());
    }
}
