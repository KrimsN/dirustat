//! Comparison of two trees by path (G-019): what was added, removed, grew or shrank.
//!
//! The walk is iterative and pairs the children of matching directories by exact name, so memory is
//! one name map per directory on the stack, not one entry per node. An added or removed subtree is
//! reported once, as its top node, with the size of the whole subtree. Only entries that can reach
//! the top-N list get a path string built. Paths are relative to the compared subtree root and use
//! `/` as separator. Sibling nodes with equal names are paired in order; extras count as added or
//! removed.

use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Grown,
    Shrunk,
}

impl ChangeKind {
    pub const ALL: [Self; 4] = [Self::Added, Self::Removed, Self::Grown, Self::Shrunk];

    pub fn name(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Grown => "grown",
            Self::Shrunk => "shrunk",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: SizeMode,
    /// Entries returned (the counts always cover everything).
    pub limit: usize,
    /// Path segments of the compared subtree, below each root.
    pub prefix: Vec<String>,
    /// Also report matched directories whose total changed (not part of the counts).
    pub include_dirs: bool,
    /// Entries with a smaller absolute size change are neither listed nor counted as matching.
    pub min_delta: u64,
    /// Which kinds are listed.
    pub kinds: Vec<ChangeKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: String,
    pub node_kind: NodeKind,
    pub old_size: u64,
    pub new_size: u64,
    /// `new_size - old_size`, saturated to `i64`.
    pub delta: i64,
    /// Id in the base tree (`None` for added entries).
    pub old_id: Option<NodeId>,
    /// Id in the other tree (`None` for removed entries).
    pub new_id: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub added: u64,
    pub removed: u64,
    pub grown: u64,
    pub shrunk: u64,
    /// Matched files and links with the same size.
    pub unchanged: u64,
    pub added_bytes: u64,
    pub removed_bytes: u64,
    pub grown_bytes: u64,
    pub shrunk_bytes: u64,
    /// Matched directories whose total changed (only when `include_dirs`).
    pub changed_dirs: u64,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub base_total: u64,
    pub other_total: u64,
    pub counts: Counts,
    pub identical: bool,
    /// Entries that matched the filters (may exceed `changes.len()`).
    pub matching: u64,
    /// Biggest changes first (by absolute delta, then path).
    pub changes: Vec<Change>,
}

struct Cand(Change);

impl Cand {
    fn key(&self) -> (u64, Reverse<&str>) {
        (self.0.delta.unsigned_abs(), Reverse(self.0.path.as_str()))
    }
}
impl PartialEq for Cand {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Cand {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

fn find_child(tree: &Tree, parent: NodeId, name: &str) -> Option<NodeId> {
    tree.children(parent).find(|&c| tree.get(c).is_some_and(|n| n.name() == name))
}

fn descend(tree: &Tree, segments: &[String]) -> Option<NodeId> {
    segments.iter().try_fold(tree.root(), |at, seg| find_child(tree, at, seg))
}

/// Names from `node` up to (not including) `base`, joined with `/`.
fn relative_path(tree: &Tree, node: NodeId, base: NodeId) -> String {
    let mut names: Vec<&str> = Vec::new();
    let mut cur = node;
    for _ in 0..=tree.len() {
        if cur == base {
            break;
        }
        let Some(n) = tree.get(cur) else { break };
        names.push(n.name());
        match n.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    names.reverse();
    names.join("/")
}

pub(crate) fn saturating_delta(old: u64, new: u64) -> i64 {
    let d = i128::from(new) - i128::from(old);
    i64::try_from(d).unwrap_or(if d < 0 { i64::MIN } else { i64::MAX })
}

struct Walker<'t> {
    a: &'t Tree,
    b: &'t Tree,
    a_base: Option<NodeId>,
    b_base: Option<NodeId>,
    opts: &'t Options,
    prefix: String,
    counts: Counts,
    matching: u64,
    heap: BinaryHeap<Reverse<Cand>>,
    stack: Vec<(NodeId, NodeId)>,
}

impl<'t> Walker<'t> {
    fn path_of(&self, old: Option<NodeId>, new: Option<NodeId>) -> String {
        let rel = match (new, self.b_base, old, self.a_base) {
            (Some(n), Some(base), _, _) => relative_path(self.b, n, base),
            (_, _, Some(o), Some(base)) => relative_path(self.a, o, base),
            _ => String::new(),
        };
        match (self.prefix.is_empty(), rel.is_empty()) {
            (true, _) => rel,
            (false, true) => self.prefix.clone(),
            (false, false) => format!("{}/{rel}", self.prefix),
        }
    }

    /// Lists the entry when it passes the filters and can make the top N.
    fn list(
        &mut self,
        kind: ChangeKind,
        node_kind: NodeKind,
        sizes: (u64, u64),
        ids: (Option<NodeId>, Option<NodeId>),
    ) {
        let (old_size, new_size) = sizes;
        let delta = saturating_delta(old_size, new_size);
        let abs = delta.unsigned_abs();
        if abs < self.opts.min_delta || !self.opts.kinds.contains(&kind) {
            return;
        }
        self.matching = self.matching.saturating_add(1);
        let full = self.heap.len() >= self.opts.limit;
        if full && self.heap.peek().is_some_and(|Reverse(least)| abs < least.0.delta.unsigned_abs()) {
            return;
        }
        let path = self.path_of(ids.0, ids.1);
        self.heap.push(Reverse(Cand(Change {
            kind,
            path,
            node_kind,
            old_size,
            new_size,
            delta,
            old_id: ids.0,
            new_id: ids.1,
        })));
        if self.heap.len() > self.opts.limit {
            self.heap.pop();
        }
    }

    fn total(tree: &Tree, id: NodeId, mode: SizeMode) -> (u64, NodeKind) {
        tree.get(id).map_or((0, NodeKind::File), |n| (n.total_size(mode), n.kind()))
    }

    fn added(&mut self, id: NodeId) {
        let (size, kind) = Self::total(self.b, id, self.opts.mode);
        self.counts.added = self.counts.added.saturating_add(1);
        self.counts.added_bytes = self.counts.added_bytes.saturating_add(size);
        self.list(ChangeKind::Added, kind, (0, size), (None, Some(id)));
    }

    fn removed(&mut self, id: NodeId) {
        let (size, kind) = Self::total(self.a, id, self.opts.mode);
        self.counts.removed = self.counts.removed.saturating_add(1);
        self.counts.removed_bytes = self.counts.removed_bytes.saturating_add(size);
        self.list(ChangeKind::Removed, kind, (size, 0), (Some(id), None));
    }

    fn matched(&mut self, x: NodeId, y: NodeId) {
        let mode = self.opts.mode;
        let (Some(nx), Some(ny)) = (self.a.get(x), self.b.get(y)) else { return };
        let (dx, dy) = (nx.kind() == NodeKind::Dir, ny.kind() == NodeKind::Dir);
        if dx != dy {
            self.removed(x);
            self.added(y);
            return;
        }
        let (old, new) = (nx.own_size(mode), ny.own_size(mode));
        let ids = (Some(x), Some(y));
        match new.cmp(&old) {
            Ordering::Greater => {
                self.counts.grown = self.counts.grown.saturating_add(1);
                self.counts.grown_bytes = self.counts.grown_bytes.saturating_add(new - old);
                self.list(ChangeKind::Grown, ny.kind(), (old, new), ids);
            }
            Ordering::Less => {
                self.counts.shrunk = self.counts.shrunk.saturating_add(1);
                self.counts.shrunk_bytes = self.counts.shrunk_bytes.saturating_add(old - new);
                self.list(ChangeKind::Shrunk, ny.kind(), (old, new), ids);
            }
            Ordering::Equal if !dx => self.counts.unchanged = self.counts.unchanged.saturating_add(1),
            Ordering::Equal => {}
        }
        if dx {
            let (told, tnew) = (nx.total_size(mode), ny.total_size(mode));
            if self.opts.include_dirs && told != tnew {
                self.counts.changed_dirs = self.counts.changed_dirs.saturating_add(1);
                let kind = if tnew > told { ChangeKind::Grown } else { ChangeKind::Shrunk };
                self.list(kind, NodeKind::Dir, (told, tnew), ids);
            }
            self.stack.push((x, y));
        }
    }

    fn run(&mut self) {
        while let Some((x, y)) = self.stack.pop() {
            let (a, b) = (self.a, self.b);
            let mut by_name: HashMap<&str, NodeId> = HashMap::new();
            for child in b.children(y) {
                let Some(node) = b.get(child) else { continue };
                if by_name.contains_key(node.name()) {
                    self.added(child);
                } else {
                    by_name.insert(node.name(), child);
                }
            }
            for child in a.children(x) {
                let Some(node) = a.get(child) else { continue };
                match by_name.remove(node.name()) {
                    Some(other) => self.matched(child, other),
                    None => self.removed(child),
                }
            }
            for (_, child) in by_name {
                self.added(child);
            }
        }
    }
}

/// Compares `base` (old) with `other` (new) below `opts.prefix`.
pub fn compare(base: &Tree, other: &Tree, opts: &Options) -> Report {
    let a_base = descend(base, &opts.prefix);
    let b_base = descend(other, &opts.prefix);
    let mut w = Walker {
        a: base,
        b: other,
        a_base,
        b_base,
        opts,
        prefix: opts.prefix.join("/"),
        counts: Counts::default(),
        matching: 0,
        heap: BinaryHeap::new(),
        stack: Vec::new(),
    };
    let total = |tree: &Tree, id: Option<NodeId>| id.map_or(0, |i| Walker::total(tree, i, opts.mode).0);
    let (base_total, other_total) = (total(base, a_base), total(other, b_base));
    match (a_base, b_base) {
        (Some(x), Some(y)) => w.matched(x, y),
        (Some(x), None) => w.removed(x),
        (None, Some(y)) => w.added(y),
        (None, None) => {}
    }
    w.run();
    let c = w.counts;
    let identical = c.added == 0 && c.removed == 0 && c.grown == 0 && c.shrunk == 0;
    let mut changes: Vec<Change> = w.heap.into_iter().map(|Reverse(Cand(c))| c).collect();
    changes.sort_by(|p, q| q.delta.unsigned_abs().cmp(&p.delta.unsigned_abs()).then_with(|| p.path.cmp(&q.path)));
    Report { base_total, other_total, counts: c, identical, matching: w.matching, changes }
}
