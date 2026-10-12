//! L2: `Session` and the command registry. **All user-visible behaviour is a command** (ADR 0003).
//!
//! UI, CLI, control channel and MCP invoke commands by `id`. A command has an id, label, menu group,
//! shortcut, params doc, `enabled` and `run`. A programmatic call never opens a dialog.
//!
//! Status: skeleton (milestone M1).

mod ext;
mod guard;
mod map;

pub use map::{MapLayout, MapState, MapStateView, MapTile};

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
    /// A panic was caught by the last-resort guard; the session is intact.
    Panicked {
        what: String,
        message: String,
    },
    /// The scan failed (root unreadable, thread pool, scan thread panic).
    Scan(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "unknown command: {id}"),
            Self::InvalidParams(why) => write!(f, "invalid params: {why}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
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
}

impl Session {
    pub fn new() -> Self {
        Self { tree: empty_tree(""), revision: 0, map: MapState::new() }
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

    /// Runs a command by id under the last-resort panic guard. The command registry arrives in M2.
    pub fn run(&mut self, id: &str, params: &str) -> Result<(), CommandError> {
        guard::guarded(id, || self.dispatch(id, params))
    }

    /// Waits for a scan and, unless it was cancelled, makes its tree current. A panic in the scan
    /// thread is reported as an error and the previous tree is kept.
    pub fn finish_scan(&mut self, handle: ScanHandle) -> Result<bool, CommandError> {
        let outcome = guard::guarded("scan", || handle.join().map_err(|err| CommandError::Scan(err.to_string())))?;
        if outcome.cancelled {
            return Ok(false);
        }
        self.tree = outcome.tree;
        self.tree_replaced();
        Ok(true)
    }

    fn dispatch(&mut self, id: &str, _params: &str) -> Result<(), CommandError> {
        match id {
            #[cfg(test)]
            "test.panic" => panic!("deliberate test panic"),
            _ => Err(CommandError::UnknownCommand(id.to_owned())),
        }
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
