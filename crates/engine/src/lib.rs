//! L2: `Session` and the command registry. **All user-visible behaviour is a command** (ADR 0003).
//!
//! UI, CLI, control channel and MCP invoke commands by `id`. A command has an id, label, menu group,
//! shortcut, params doc, `enabled` and `run`. A programmatic call never opens a dialog.
//!
//! Status: command registry in place (milestone M2); see `cmd`.

pub mod cleanup;
mod cmd;
mod drives;
pub mod duplicates;
pub mod export;
mod ext;
mod fsops;
mod guard;
mod journal;
mod launch;
mod look;
mod map;
mod paths;
mod protected;
mod prune;
pub mod search;
mod settings;
pub mod snapshot;
mod subtree;

pub use cmd::{
    CommandInfo, CommandSpec, DetachedJob, FastPathStatus, JobOutcome, Prepared, SnapshotView, execute_unlocked,
    fast_path_status, tree_node_path, volume_root_of,
};
pub use map::{MapLayout, MapState, MapStateView, MapTile};
pub use paths::{PathClass, classify_path, is_local_path, is_unc_path, normalize_scan_root};

pub use drives::{DriveInfo, MAX_DRIVES};
pub use launch::{LaunchArgs, LaunchError, USAGE as LAUNCH_USAGE};
pub use look::{CushionLook, LookPatch};
pub use settings::{
    LoadOutcome, MAX_RECENT_PATHS, SETTINGS_VERSION, Settings, SettingsError, SizeModeName, SortKey, SortSetting,
    TreemapSettings, default_dir as settings_dir, default_path as settings_path,
};

pub use ext::{ExtStat, NO_EXT_COLOR, ext_color, extension_of};
pub use journal::{JournalEntry, MAX_JOURNAL_ENTRIES};

use dirustat_model::{SizeMode, Tree};
use dirustat_platform::PlatformServices;
use dirustat_scan::{ScanBackend, ScanHandle, empty_tree};
use dirustat_treemap::{Rect, squarify};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

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
    /// A platform service failed at the OS level (permission, launch failure, ...).
    Platform(String),
    /// A destructive command was refused for a protected path (`docs/safety.md`).
    ProtectedPath(String),
    /// The tree no longer matches the disk; nothing was changed. Rescan and retry.
    StaleTree(String),
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
            Self::ProtectedPath(_) => "protected_path",
            Self::StaleTree(_) => "stale_tree",
            Self::Panicked { .. } | Self::Scan(_) | Self::Platform(_) => "internal",
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
            Self::Platform(why) => write!(f, "platform service failed: {why}"),
            Self::ProtectedPath(why) => write!(f, "refused, protected path: {why}"),
            Self::StaleTree(why) => write!(f, "stale tree, nothing changed: {why}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Session state: the current tree and derived data.
#[derive(Debug)]
pub struct Session {
    tree: Arc<Tree>,
    /// Bumped whenever the tree is replaced (cache key for derived data).
    revision: u64,
    map: MapState,
    /// Background scan started by `scan.start`, not yet joined.
    scan: Option<PendingScan>,
    /// How the last scan that replaced the tree ran (backend and fast-path fallback reason).
    last_scan: Option<LastScan>,
    /// Persistent user settings (`settings.*` commands).
    settings: Settings,
    drives: drives::DrivesProvider,
    /// Last `search.find` result, valid for one tree revision.
    search: search::SearchState,
    /// OS services (drives, recycle bin, file manager); the real one by default.
    platform: Arc<dyn PlatformServices>,
    /// Journal of destructive actions (`file.journal`).
    journal: journal::Journal,
    /// The current tree came from a scan with exclusions: it omits entries that exist on disk.
    partial_scan: bool,
    /// A destructive action failed half-way; the tree may no longer match the disk.
    rescan_recommended: bool,
    /// Set while a detached permanent deletion runs without the session lock (QA-144).
    removal_busy: Arc<AtomicBool>,
    /// Set while a helper thread waits for the OS to answer a volume query (QA-143).
    volume_query_busy: Arc<AtomicBool>,
    volume_query_timeout: Duration,
    /// Duplicate search job and result (`duplicates.*`), valid for one tree revision.
    dups: cmd::DupState,
    /// Loaded snapshots and the one being viewed (`snapshot.*`).
    snapshots: snapshot::Store,
}

/// Default time `platform.free_space` waits for a volume (short in tests).
const DEFAULT_VOLUME_QUERY_TIMEOUT: Duration =
    if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(3) };

/// Which backend produced the current tree and why the MFT fast path was abandoned, if it was.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LastScan {
    backend: ScanBackend,
    fallback_reason: Option<String>,
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
            tree: Arc::new(empty_tree("")),
            revision: 0,
            map: MapState::new(),
            scan: None,
            last_scan: None,
            settings: Settings::default(),
            drives: drives::DrivesProvider(None),
            search: search::SearchState::default(),
            platform: dirustat_platform::default_platform(),
            journal: journal::Journal::new(),
            partial_scan: false,
            rescan_recommended: false,
            removal_busy: Arc::new(AtomicBool::new(false)),
            volume_query_busy: Arc::new(AtomicBool::new(false)),
            volume_query_timeout: DEFAULT_VOLUME_QUERY_TIMEOUT,
            dups: cmd::DupState::default(),
            snapshots: snapshot::Store::default(),
        }
    }

    /// Mirrors the destructive-action journal into a JSONL file (best effort, appended; the
    /// directory is not created). Hosts pass `settings_dir()/journal.jsonl`; `None` keeps it in memory.
    pub fn set_journal_file(&mut self, file: Option<std::path::PathBuf>) {
        self.journal.set_file(file);
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Shared handle to the current tree, for work that runs without the session lock (exports).
    pub(crate) fn tree_arc(&self) -> Arc<Tree> {
        Arc::clone(&self.tree)
    }

    /// Replaces the platform services (tests pass `dirustat_platform::FakePlatform`).
    pub fn set_platform(&mut self, platform: Arc<dyn PlatformServices>) {
        self.platform = platform;
    }

    /// The platform services of this session (shared handle).
    pub fn platform(&self) -> Arc<dyn PlatformServices> {
        Arc::clone(&self.platform)
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
        self.partial_scan = false;
        self.rescan_recommended = false;
        self.tree = Arc::new(tree);
        self.tree_replaced();
    }

    /// Waits for a scan and, unless it was cancelled, makes its tree current. A panic in the scan
    /// thread is reported as an error and the previous tree is kept.
    pub fn finish_scan(&mut self, handle: ScanHandle) -> Result<bool, CommandError> {
        let used_exclusions = handle.used_exclusions();
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
        self.last_scan = Some(LastScan { backend: outcome.backend, fallback_reason: outcome.fallback_reason });
        self.partial_scan = used_exclusions;
        self.rescan_recommended = false;
        // A selection made against the previous tree (also one issued while this scan was
        // finishing) follows its node into the new tree by path instead of being dropped.
        let kept = self.selection_names().zip(self.selection().and_then(|n| self.node_path(n)));
        // A selection whose parent chain is broken cannot be followed: report it, never drop it silently.
        let unfollowable = match (&kept, self.selection()) {
            (None, Some(node)) => Some(self.node_path(node).unwrap_or_default()),
            _ => None,
        };
        self.tree = Arc::new(outcome.tree);
        self.tree_replaced();
        if let Some((names, path)) = kept {
            self.restore_selection(&names, path);
        } else if let Some(path) = unfollowable {
            self.mark_selection_lost(path);
        }
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
