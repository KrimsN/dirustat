//! Phase 2: turn the collected entries and links into a `Tree`, top-down and iteratively.

use std::sync::atomic::{AtomicBool, Ordering};

use dirustat_model::{ModelError, NodeData, NodeId, NodeKind, Tree};

use super::{
    ATTR_DIRECTORY, Collected, Entry, EntryKind, Link, MftError, MftStats, NO_MTIME, NO_PARENT, ORPHAN_DIR,
    ROOT_RECORD, SYSTEM_PARENT,
};
use crate::Counters;

/// The entry has a node (or, for system subtrees, was deliberately skipped).
const REACHED: u8 = 1;
/// The size of this (file) entry has been charged to a node.
const CHARGED: u8 = 2;
/// The cancel flag is read once per this many created nodes.
const CANCEL_EVERY: u64 = 4096;

/// Why node creation stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Cancelled,
    /// The model refused more nodes; what exists is kept.
    Full,
}

/// Resolves a parent record number to an entry index, [`SYSTEM_PARENT`] or [`NO_PARENT`].
fn resolve_parent(entries: &[Entry], record: u32, sequence: u16) -> u32 {
    if record >= SYSTEM_PARENT {
        return NO_PARENT;
    }
    if record < super::FIRST_USER_RECORD && record != ROOT_RECORD {
        return SYSTEM_PARENT;
    }
    let Ok(index) = entries.binary_search_by_key(&record, |e| e.number) else { return NO_PARENT };
    let usable = entries.get(index).is_some_and(|e| e.kind == EntryKind::Dir && e.sequence == sequence);
    match u32::try_from(index) {
        Ok(i) if usable => i,
        _ => NO_PARENT,
    }
}

struct Builder {
    entries: Vec<Entry>,
    links: Vec<Link>,
    names: Vec<u16>,
    /// `child_start[e]..child_start[e + 1]` indexes `child_links` for the children of entry `e`.
    child_start: Vec<u32>,
    child_links: Vec<u32>,
    state: Vec<u8>,
    tree: Tree,
    stats: MftStats,
    root_entry: u32,
    orphan_dir: Option<NodeId>,
    created: u64,
}

pub(super) fn build(
    collected: Collected,
    label: &str,
    cancel: &AtomicBool,
    counters: &Counters,
) -> Result<(Tree, bool, MftStats), MftError> {
    let Collected { entries, mut links, names, stats, .. } = collected;
    let root_entry = entries
        .binary_search_by_key(&ROOT_RECORD, |e| e.number)
        .ok()
        .and_then(|i| u32::try_from(i).ok())
        .filter(|&i| entries.get(usize_of(i)).is_some_and(|e| e.kind == EntryKind::Dir))
        .ok_or(MftError::NoRoot)?;
    let cancelled_result = |stats: MftStats| {
        let mut tree = Tree::new(label);
        tree.aggregate();
        Ok((tree, true, stats))
    };
    for (i, link) in links.iter_mut().enumerate() {
        if polled(i, cancel) {
            return cancelled_result(stats);
        }
        link.parent = resolve_parent(&entries, link.parent, link.parent_sequence);
    }
    let Some((child_start, child_links)) = group_children(entries.len(), &links, root_entry, cancel)? else {
        return cancelled_result(stats);
    };
    let state = zeroed(entries.len(), 0u8, "state")?;
    let mut b = Builder {
        entries,
        links,
        names,
        child_start,
        child_links,
        state,
        tree: Tree::new(label),
        stats,
        root_entry,
        orphan_dir: None,
        created: 0,
    };
    let stop = b.run(cancel);
    let cancelled = stop == Some(Stop::Cancelled);
    if stop == Some(Stop::Full) {
        tracing::warn!("MFT scan stopped at the node limit; the tree is partial");
    }
    b.tree.aggregate();
    if let Some(root) = b.tree.get(b.tree.root()) {
        counters.store_totals(
            root.dir_count(),
            root.file_count(),
            root.total_size(dirustat_model::SizeMode::Logical),
            root.error_count().saturating_add(b.stats.corrupt),
        );
    }
    Ok((b.tree, cancelled, b.stats))
}

/// Rewrites a name from the volume so joining it to a path can never leave the folder: path separators,
/// `:` and control characters become `_`, and `.`, `..` and the empty name get a `_` prefix. `None` when the
/// name is already safe (the common case allocates nothing).
fn safe_name(units: &[u16]) -> Option<Vec<u16>> {
    let unsafe_unit = |u: u16| matches!(u, 0x2F | 0x5C | 0x3A) || u < 0x20 || u == 0x7F;
    let dots = matches!(units, [] | [0x2E] | [0x2E, 0x2E]);
    if !dots && !units.iter().copied().any(unsafe_unit) {
        return None;
    }
    let mut out = Vec::with_capacity(units.len().saturating_add(1));
    if dots {
        out.push(0x5F);
    }
    out.extend(units.iter().map(|&u| if unsafe_unit(u) { 0x5F } else { u }));
    Some(out)
}

/// True every `CANCEL_EVERY` iterations when the cancel flag is set.
fn polled(iteration: usize, cancel: &AtomicBool) -> bool {
    iteration.is_multiple_of(usize::try_from(CANCEL_EVERY).unwrap_or(4096)) && cancel.load(Ordering::Relaxed)
}

/// A zero-initialised vector that reports allocation failure as an error instead of aborting.
fn zeroed<T: Clone>(len: usize, value: T, what: &'static str) -> Result<Vec<T>, MftError> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| MftError::TooLarge(what))?;
    v.resize(len, value);
    Ok(v)
}

fn usize_of(i: u32) -> usize {
    usize::try_from(i).unwrap_or(usize::MAX)
}

/// Counting sort of the resolved links by parent entry. The root's own links are left out.
/// Children index: `(start, grouped)`.
type ChildIndex = (Vec<u32>, Vec<u32>);

/// `Ok(None)` means the scan was cancelled meanwhile.
fn group_children(
    entry_count: usize,
    links: &[Link],
    root_entry: u32,
    cancel: &AtomicBool,
) -> Result<Option<ChildIndex>, MftError> {
    let mut start = zeroed(entry_count.saturating_add(1), 0u32, "link index")?;
    let mut total = 0u32;
    let attached = |l: &Link| l.owner != root_entry && usize_of(l.parent) < entry_count;
    for (i, link) in links.iter().enumerate() {
        if polled(i, cancel) {
            return Ok(None);
        }
        if !attached(link) {
            continue;
        }
        if let Some(slot) = start.get_mut(usize_of(link.parent).saturating_add(1)) {
            *slot = slot.saturating_add(1);
        }
        total = total.checked_add(1).ok_or(MftError::TooLarge("link"))?;
    }
    let mut acc = 0u32;
    for slot in &mut start {
        acc = acc.saturating_add(*slot);
        *slot = acc;
    }
    let mut cursor = start.clone();
    let mut grouped = zeroed(usize_of(total), 0u32, "link index")?;
    for (index, link) in links.iter().enumerate() {
        if polled(index, cancel) {
            return Ok(None);
        }
        if !attached(link) {
            continue;
        }
        let Some(pos) = cursor.get_mut(usize_of(link.parent)) else { continue };
        if let Some(dst) = grouped.get_mut(usize_of(*pos)) {
            *dst = u32::try_from(index).map_err(|_| MftError::TooLarge("link"))?;
        }
        *pos = pos.saturating_add(1);
    }
    Ok(Some((start, grouped)))
}

impl Builder {
    fn run(&mut self, cancel: &AtomicBool) -> Option<Stop> {
        let result = self.run_passes(cancel);
        result.err()
    }

    fn run_passes(&mut self, cancel: &AtomicBool) -> Result<(), Stop> {
        let mut stack: Vec<(NodeId, u32)> = Vec::new();
        // Pass 1: everything reachable from the volume root.
        self.mark(self.root_entry, REACHED);
        stack.push((self.tree.root(), self.root_entry));
        self.drain(&mut stack, cancel)?;
        // Pass 2: system subtrees (children of metafiles) are skipped without making nodes.
        self.skip_system_subtrees(cancel)?;
        // Pass 3: links whose parent cannot be used go below `<orphaned>`.
        for index in 0..self.links.len() {
            if polled(index, cancel) {
                return Err(Stop::Cancelled);
            }
            let Some(&link) = self.links.get(index) else { continue };
            if link.parent != NO_PARENT || link.owner == self.root_entry {
                continue;
            }
            if self.is_dir_reached(link.owner) {
                continue;
            }
            let parent = self.orphan_node()?;
            self.stats.orphan_links = self.stats.orphan_links.saturating_add(1);
            self.attach(parent, index, &mut stack, cancel)?;
            self.drain(&mut stack, cancel)?;
        }
        // Pass 4: whatever is still unreachable sits in a parent cycle (or below one).
        for entry in 0..self.entries.len() {
            if polled(entry, cancel) {
                return Err(Stop::Cancelled);
            }
            let Some(&e) = self.entries.get(entry) else { continue };
            if self.state.get(entry).copied().unwrap_or(REACHED) & REACHED != 0 || e.link_count == 0 {
                continue;
            }
            let parent = self.orphan_node()?;
            self.stats.cycle_entries = self.stats.cycle_entries.saturating_add(1);
            self.attach(parent, usize_of(e.link_start), &mut stack, cancel)?;
            self.drain(&mut stack, cancel)?;
        }
        Ok(())
    }

    fn mark(&mut self, entry: u32, bits: u8) {
        if let Some(s) = self.state.get_mut(usize_of(entry)) {
            *s |= bits;
        }
    }

    fn is_dir_reached(&self, entry: u32) -> bool {
        let reached = self.state.get(usize_of(entry)).is_some_and(|s| s & REACHED != 0);
        reached && self.entries.get(usize_of(entry)).is_some_and(|e| e.kind == EntryKind::Dir)
    }

    fn children(&self, entry: u32) -> std::ops::Range<usize> {
        let e = usize_of(entry);
        let start = self.child_start.get(e).copied().unwrap_or(0);
        let end = self.child_start.get(e.saturating_add(1)).copied().unwrap_or(start);
        usize_of(start)..usize_of(end)
    }

    fn orphan_node(&mut self) -> Result<NodeId, Stop> {
        if let Some(id) = self.orphan_dir {
            return Ok(id);
        }
        let data = NodeData { error: true, attributes: ATTR_DIRECTORY, ..NodeData::default() };
        let id = self.tree.add_node(self.tree.root(), ORPHAN_DIR, NodeKind::Dir, data).map_err(|_| Stop::Full)?;
        self.orphan_dir = Some(id);
        Ok(id)
    }

    /// Marks every entry below a metafile as handled, without creating nodes.
    fn skip_system_subtrees(&mut self, cancel: &AtomicBool) -> Result<(), Stop> {
        let mut pending: Vec<u32> = Vec::new();
        for (i, link) in self.links.iter().enumerate() {
            if polled(i, cancel) {
                return Err(Stop::Cancelled);
            }
            if link.parent == SYSTEM_PARENT && link.owner != self.root_entry {
                pending.push(link.owner);
            }
        }
        let mut handled = 0usize;
        while let Some(entry) = pending.pop() {
            handled += 1;
            if polled(handled, cancel) {
                return Err(Stop::Cancelled);
            }
            let already = self.state.get(usize_of(entry)).is_some_and(|s| s & REACHED != 0);
            if already {
                continue;
            }
            self.mark(entry, REACHED | CHARGED);
            self.stats.system_entries = self.stats.system_entries.saturating_add(1);
            for k in self.children(entry) {
                let owner = self.child_links.get(k).and_then(|&li| self.links.get(usize_of(li))).map(|l| l.owner);
                pending.extend(owner);
            }
        }
        Ok(())
    }

    /// Creates the children of every directory on the stack, depth-first, until it is empty.
    fn drain(&mut self, stack: &mut Vec<(NodeId, u32)>, cancel: &AtomicBool) -> Result<(), Stop> {
        while let Some((node, entry)) = stack.pop() {
            for k in self.children(entry) {
                let Some(&li) = self.child_links.get(k) else { continue };
                self.attach(node, usize_of(li), stack, cancel)?;
            }
        }
        Ok(())
    }

    /// Creates the node for link `index` below `parent`; directories are queued on `stack`.
    fn attach(
        &mut self,
        parent: NodeId,
        index: usize,
        stack: &mut Vec<(NodeId, u32)>,
        cancel: &AtomicBool,
    ) -> Result<(), Stop> {
        self.created = self.created.saturating_add(1);
        if self.created.is_multiple_of(CANCEL_EVERY) && cancel.load(Ordering::Relaxed) {
            return Err(Stop::Cancelled);
        }
        let Some(&link) = self.links.get(index) else { return Ok(()) };
        let Some(&entry) = self.entries.get(usize_of(link.owner)) else { return Ok(()) };
        let Some(state) = self.state.get(usize_of(link.owner)).copied() else { return Ok(()) };
        let first = state & CHARGED == 0;
        match entry.kind {
            EntryKind::Dir => {
                if state & REACHED != 0 {
                    // A second link to a directory (corrupt volume or parent cycle): keep the first place.
                    self.stats.duplicate_dirs = self.stats.duplicate_dirs.saturating_add(1);
                    return Ok(());
                }
                self.mark(link.owner, REACHED | CHARGED);
            }
            EntryKind::File | EntryKind::Link => self.mark(link.owner, REACHED | CHARGED),
        }
        let (logical, allocated) = if first { (entry.logical, entry.allocated) } else { (0, 0) };
        let attributes =
            if entry.kind == EntryKind::Dir { entry.attributes | ATTR_DIRECTORY } else { entry.attributes };
        let data = NodeData {
            logical_size: logical,
            allocated_size: allocated,
            mtime: (entry.mtime != NO_MTIME).then_some(entry.mtime),
            attributes,
            error: entry.error,
            name_lossy: false,
        };
        let start = usize_of(link.name_offset);
        let raw = self.names.get(start..start.saturating_add(usize::from(link.name_len))).unwrap_or(&[]);
        let safe = safe_name(raw);
        let mut data = data;
        if safe.is_some() {
            data.name_lossy = true;
            self.stats.names_sanitized = self.stats.names_sanitized.saturating_add(1);
        }
        let name = safe.as_deref().unwrap_or(raw);
        match self.tree.add_node_from_wide(parent, name, entry.kind.node_kind(), data) {
            Ok(id) => {
                if entry.kind == EntryKind::Dir {
                    stack.push((id, link.owner));
                }
                Ok(())
            }
            Err(ModelError::TooManyNodes) => {
                self.stats.node_errors = self.stats.node_errors.saturating_add(1);
                Err(Stop::Full)
            }
            Err(_) => {
                self.stats.node_errors = self.stats.node_errors.saturating_add(1);
                Ok(())
            }
        }
    }
}
