//! Scan snapshots (G-019): saving a tree to a `.dirsnap` file, loading one back as a read-only
//! view, and comparing two trees by path. The commands live in `cmd/snapshot.rs`.

pub mod compare;
pub mod format;

#[cfg(test)]
pub(crate) mod tests;

pub use format::{Loaded, Meta, SnapshotError};

use dirustat_model::Tree;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Most snapshots held in memory at once (each one is a whole tree).
pub const MAX_LOADED: usize = 4;
/// File extension of a snapshot, without the dot.
pub const EXTENSION: &str = "dirsnap";

/// A snapshot loaded into the session.
#[derive(Clone, Debug)]
pub struct LoadedSnapshot {
    pub id: u64,
    /// File it was read from.
    pub path: String,
    pub meta: Meta,
    pub tree: Arc<Tree>,
}

/// The loaded snapshots and which of them (if any) is the tree the session shows.
#[derive(Debug, Default)]
pub struct Store {
    next_id: u64,
    pub(crate) items: Vec<LoadedSnapshot>,
    /// Id of the snapshot that is the current tree. While set, the tree is not the live disk and
    /// destructive commands are refused.
    pub(crate) viewing: Option<u64>,
    /// Loads that have begun but are not stored yet; they count against [`MAX_LOADED`].
    in_flight: Arc<AtomicUsize>,
}

/// A reserved place for a snapshot that is being read; released when dropped.
#[derive(Debug)]
pub(crate) struct LoadSlot(Arc<AtomicUsize>);

impl Drop for LoadSlot {
    fn drop(&mut self) {
        let _ = self.0.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| Some(n.saturating_sub(1)));
    }
}

impl Store {
    /// Reserves a place for a load, or `None` when stored plus in-flight snapshots reach the cap.
    /// Called under the session lock, so the check and the increment cannot interleave.
    pub(crate) fn reserve(&self) -> Option<LoadSlot> {
        let pending = self.in_flight.load(Ordering::SeqCst);
        if self.items.len().saturating_add(pending) >= MAX_LOADED {
            return None;
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        Some(LoadSlot(Arc::clone(&self.in_flight)))
    }

    pub(crate) fn add(&mut self, path: String, meta: Meta, tree: Tree) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        self.items.push(LoadedSnapshot { id, path, meta, tree: Arc::new(tree) });
        id
    }

    pub(crate) fn get(&self, id: u64) -> Option<&LoadedSnapshot> {
        self.items.iter().find(|s| s.id == id)
    }
}
