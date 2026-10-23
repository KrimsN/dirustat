//! Treemap view state of a `Session`: cached hierarchical layout, hit testing and selection.
//!
//! The layout is cached by (tree revision, bounds, size mode), so it is never recomputed per frame.

use crate::Session;
use dirustat_model::{NodeId, SizeMode, Tree};
use dirustat_treemap::{LayoutOptions, Rect, layout_tree};
use serde::Serialize;
use std::sync::{Arc, mpsc};

/// One laid-out node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapTile {
    pub node: NodeId,
    pub rect: Rect,
    pub depth: u32,
    pub leaf: bool,
    /// Stands for the children of `node` too small to get their own tile; always a leaf.
    pub remainder: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CacheKey {
    revision: u64,
    bounds: [u32; 4],
    mode: SizeMode,
}

fn bounds_bits(b: Rect) -> [u32; 4] {
    [b.x.to_bits(), b.y.to_bits(), b.w.to_bits(), b.h.to_bits()]
}

/// Hierarchical treemap layout of a tree. Tiles are in parent-before-children order.
#[derive(Clone, Debug, Default)]
pub struct MapLayout {
    tiles: Vec<MapTile>,
}

impl MapLayout {
    /// Lays out `tree` inside `bounds`, weighting nodes by `mode`.
    pub fn build(tree: &Tree, bounds: Rect, mode: SizeMode, opts: LayoutOptions) -> Self {
        // Handles given to the treemap are positions in `ids`.
        let mut ids: Vec<NodeId> = vec![tree.root()];
        let tiles = layout_tree(0, bounds, opts, |handle, out| {
            let Some(&node) = ids.get(handle) else { return };
            for child in tree.children(node) {
                let weight = tree.get(child).map_or(0, |n| n.total_size(mode));
                ids.push(child);
                out.push((ids.len().saturating_sub(1), weight));
            }
        });
        let tiles = tiles
            .into_iter()
            .filter_map(|t| {
                let node = *ids.get(t.id)?;
                Some(MapTile { node, rect: t.rect, depth: t.depth, leaf: t.leaf, remainder: t.remainder })
            })
            .collect();
        Self { tiles }
    }

    /// Maps tiles laid out in `from` onto `to` (affine scale and shift), clamping every rect into `to`
    /// so float error never lets a tile stick out of the bounds. Invalid bounds give an empty layout.
    fn scaled(&self, from: Rect, to: Rect) -> Self {
        if !is_valid(from) || !is_valid(to) {
            return Self::default();
        }
        let (sx, sy) = (to.w / from.w, to.h / from.h);
        let (right, bottom) = (to.x + to.w, to.y + to.h);
        let tiles = self
            .tiles
            .iter()
            .map(|t| {
                let x = (to.x + (t.rect.x - from.x) * sx).clamp(to.x, right);
                let y = (to.y + (t.rect.y - from.y) * sy).clamp(to.y, bottom);
                let w = (t.rect.w * sx).min(right - x).max(0.0);
                let h = (t.rect.h * sy).min(bottom - y).max(0.0);
                MapTile { rect: Rect { x, y, w, h }, ..*t }
            })
            .collect();
        Self { tiles }
    }

    pub fn tiles(&self) -> &[MapTile] {
        &self.tiles
    }

    /// Deepest tile under the point (ties: the later one). Allocation-free.
    pub fn tile_at(&self, px: f32, py: f32) -> Option<&MapTile> {
        let mut best: Option<&MapTile> = None;
        for t in &self.tiles {
            if t.rect.contains(px, py) && best.is_none_or(|b| t.depth >= b.depth) {
                best = Some(t);
            }
        }
        best
    }
}

/// Serializable snapshot of the map state for the control channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MapStateView {
    pub revision: u64,
    pub size_mode: &'static str,
    pub tile_count: usize,
    /// Arena index of the selected node.
    pub selected: Option<usize>,
    pub selected_name: Option<String>,
}

/// Map state owned by `Session`.
#[derive(Debug)]
pub struct MapState {
    mode: SizeMode,
    selected: Option<NodeId>,
    options: LayoutOptions,
    /// Layout computed for the quantized bounds.
    cache: Option<(CacheKey, MapLayout)>,
    /// `cache` rescaled to the exact bounds (key of the base layout, exact bounds bits).
    view: Option<(CacheKey, [u32; 4], MapLayout)>,
    /// Layout being computed on a background thread (see [`Session::map_view_async`]).
    inflight: Option<InFlight>,
    /// Bumped whenever `cache` or `view` receives different tiles; lets the UI key its own caches.
    generation: u64,
    /// Path of a selection that could not be carried over to the tree of the last finished scan
    /// (the node is gone there); cleared by the next selection change.
    lost_selection: Option<String>,
}

/// A background layout: the key it is computed for and the channel its result arrives on.
#[derive(Debug)]
struct InFlight {
    key: CacheKey,
    rx: mpsc::Receiver<MapLayout>,
}

impl MapState {
    pub(crate) fn new() -> Self {
        Self {
            mode: SizeMode::default(),
            selected: None,
            options: LayoutOptions::default(),
            cache: None,
            view: None,
            inflight: None,
            generation: 0,
            lost_selection: None,
        }
    }
}

/// Upper bound on the depth followed when carrying a selection across a tree replacement
/// (a broken parent chain cannot loop).
const MAX_SELECTION_DEPTH: usize = 4096;

fn bounds_from_bits(bits: [u32; 4]) -> Rect {
    Rect {
        x: f32::from_bits(bits[0]),
        y: f32::from_bits(bits[1]),
        w: f32::from_bits(bits[2]),
        h: f32::from_bits(bits[3]),
    }
}

fn is_valid(b: Rect) -> bool {
    b.x.is_finite() && b.y.is_finite() && b.w.is_finite() && b.h.is_finite() && b.w > 0.0 && b.h > 0.0
}

/// Rounds one dimension up to a multiple of 1/8 of the next power of two (at least 16 px), i.e. about
/// 6-12 % steps. Values beyond 2^20 px are clamped.
fn quantize_dim(v: f32) -> f32 {
    let clamped = v.clamp(1.0, 1_048_576.0);
    // Both casts are in range after the clamp.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = clamped.ceil() as u32;
    let step = (px.next_power_of_two() / 8).max(16);
    let q = px.div_ceil(step).saturating_mul(step);
    #[allow(clippy::cast_precision_loss)]
    let q = q as f32;
    q
}

/// Bounds the expensive layout is computed for: origin (0, 0) and a size rounded up to a coarse grid, so
/// that small window-size changes reuse the cached layout. Invalid bounds are returned unchanged (they
/// lay out to nothing).
fn quantize_bounds(b: Rect) -> Rect {
    if !is_valid(b) {
        return b;
    }
    Rect { x: 0.0, y: 0.0, w: quantize_dim(b.w), h: quantize_dim(b.h) }
}

impl Session {
    /// Monotonic counter bumped whenever the tree is replaced.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn map_size_mode(&self) -> SizeMode {
        self.map.mode
    }

    pub fn set_map_size_mode(&mut self, mode: SizeMode) {
        self.map.mode = mode;
        self.settings.size_mode = crate::SizeModeName::from_mode(mode);
    }

    /// Called when the tree was replaced: bumps the revision and drops selection and cache.
    pub(crate) fn tree_replaced(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.map.selected = None;
        self.map.lost_selection = None;
        self.map.cache = None;
        self.map.view = None;
        // A layout still being computed belongs to the old tree: its result is dropped.
        self.map.inflight = None;
        self.map.generation = self.map.generation.wrapping_add(1);
    }

    /// Generation of the tiles returned by [`Session::map_view_async`]; changes whenever they change.
    pub fn map_generation(&self) -> u64 {
        self.map.generation
    }

    /// Like [`Session::map_view`], but never computes the hierarchical layout on the calling thread.
    ///
    /// When the layout for `bounds` is missing it is computed on a background thread (one at a time)
    /// and the call returns immediately: with the previous layout of the same tree and size mode
    /// rescaled to `bounds` (so a resize keeps showing the old picture), or with an empty layout when
    /// there is none. The third value is `true` while a layout is outstanding; call again later
    /// (e.g. on the next frame) to pick up the result.
    pub fn map_view_async(&mut self, bounds: Rect) -> (&Tree, &MapLayout, bool) {
        let pending = self.map_poll(bounds);
        let (tree, layout) = self.map_ready_view();
        (tree, layout, pending)
    }

    /// The non-blocking half of [`Session::map_view_async`]: advances the background layout for
    /// `bounds` and returns whether it is still outstanding. Read the tiles with
    /// [`Session::map_ready_view`] (after reading [`Session::map_generation`] if you key caches on it).
    pub fn map_poll(&mut self, bounds: Rect) -> bool {
        let pending = self.poll_map_layout(bounds);
        self.refresh_map_view(bounds);
        pending
    }

    /// The tiles last prepared by [`Session::map_poll`] (empty when none), with their tree.
    pub fn map_ready_view(&self) -> (&Tree, &MapLayout) {
        static EMPTY: MapLayout = MapLayout { tiles: Vec::new() };
        match &self.map.view {
            Some((_, _, layout)) => (&self.tree, layout),
            None => (&self.tree, &EMPTY),
        }
    }

    /// Collects a finished background layout and starts one for `bounds` when it is missing.
    /// Returns whether the layout for `bounds` is still outstanding.
    fn poll_map_layout(&mut self, bounds: Rect) -> bool {
        let finished = self.map.inflight.as_ref().map(|f| (f.key, f.rx.try_recv()));
        match finished {
            Some((key, Ok(layout))) => self.install_map_layout(key, layout),
            Some((key, Err(mpsc::TryRecvError::Disconnected))) => {
                // The layout thread died (a panic is contained there): cache an empty layout so the
                // failing key is not retried every frame.
                self.install_map_layout(key, MapLayout::default());
            }
            Some((_, Err(mpsc::TryRecvError::Empty))) | None => {}
        }
        let base_bounds = quantize_bounds(bounds);
        if !is_valid(base_bounds) {
            return false;
        }
        let key = CacheKey { revision: self.revision, bounds: bounds_bits(base_bounds), mode: self.map.mode };
        if matches!(&self.map.cache, Some((k, _)) if *k == key) {
            return false;
        }
        match &self.map.inflight {
            Some(f) if f.key == key => {}
            // Another size is still being computed: wait for it, then start this one.
            Some(_) => {}
            None => self.spawn_map_layout(key, base_bounds),
        }
        true
    }

    fn install_map_layout(&mut self, key: CacheKey, layout: MapLayout) {
        self.map.inflight = None;
        if key.revision == self.revision && key.mode == self.map.mode {
            self.map.cache = Some((key, layout));
            self.map.view = None;
            self.map.generation = self.map.generation.wrapping_add(1);
        }
    }

    fn spawn_map_layout(&mut self, key: CacheKey, base_bounds: Rect) {
        let (tx, rx) = mpsc::channel();
        let tree = Arc::clone(&self.tree);
        let (mode, opts) = (self.map.mode, self.map.options);
        let spawned = std::thread::Builder::new().name("dirustat-map-layout".to_owned()).spawn(move || {
            // A closed receiver only means the result is no longer wanted.
            let _ = tx.send(MapLayout::build(&tree, base_bounds, mode, opts));
        });
        match spawned {
            Ok(_) => self.map.inflight = Some(InFlight { key, rx }),
            // No thread available: compute here rather than leave the map empty.
            Err(_) => {
                let layout = MapLayout::build(&self.tree, base_bounds, mode, opts);
                self.install_map_layout(key, layout);
            }
        }
    }

    /// Rebuilds `view` from the cached layout (when it belongs to the current tree and mode) for `bounds`.
    fn refresh_map_view(&mut self, bounds: Rect) {
        let Some((key, base)) = &self.map.cache else {
            self.map.view = None;
            return;
        };
        if key.revision != self.revision || key.mode != self.map.mode {
            self.map.view = None;
            return;
        }
        let exact = bounds_bits(bounds);
        if matches!(&self.map.view, Some((k, b, _)) if k == key && *b == exact) {
            return;
        }
        let scaled = base.scaled(bounds_from_bits(key.bounds), bounds);
        self.map.view = Some((*key, exact, scaled));
        self.map.generation = self.map.generation.wrapping_add(1);
    }

    /// Cached layout for `bounds` and the current size mode.
    ///
    /// The expensive hierarchical layout is computed for coarsely quantized bounds (see
    /// [`quantize_bounds`]) and only when revision, quantized size or mode change; for the exact `bounds`
    /// the cached tiles are merely rescaled (cheap, at most `max_tiles` rects). Dragging a window edge
    /// therefore recomputes the layout once per size step instead of once per pixel.
    pub fn map_layout(&mut self, bounds: Rect) -> &MapLayout {
        let base_bounds = quantize_bounds(bounds);
        let key = CacheKey { revision: self.revision, bounds: bounds_bits(base_bounds), mode: self.map.mode };
        if !matches!(&self.map.cache, Some((k, _)) if *k == key) {
            let layout = MapLayout::build(&self.tree, base_bounds, self.map.mode, self.map.options);
            self.map.cache = Some((key, layout));
            self.map.view = None;
        }
        let exact = bounds_bits(bounds);
        if !matches!(&self.map.view, Some((k, b, _)) if *k == key && *b == exact)
            && let Some((_, base)) = &self.map.cache
        {
            self.map.view = Some((key, exact, base.scaled(base_bounds, bounds)));
        }
        match &self.map.view {
            Some((_, _, layout)) => layout,
            None => {
                // Unreachable by construction; an empty layout keeps the API total.
                static EMPTY: MapLayout = MapLayout { tiles: Vec::new() };
                &EMPTY
            }
        }
    }

    /// Cached layout for `bounds` together with the tree it was built from.
    pub fn map_view(&mut self, bounds: Rect) -> (&Tree, &MapLayout) {
        self.map_layout(bounds);
        match &self.map.view {
            Some((_, _, layout)) => (&self.tree, layout),
            None => {
                static EMPTY: MapLayout = MapLayout { tiles: Vec::new() };
                (&self.tree, &EMPTY)
            }
        }
    }

    /// Node under the point in the last computed layout (empty when none was computed yet).
    pub fn tile_at(&self, px: f32, py: f32) -> Option<NodeId> {
        let (_, _, layout) = self.map.view.as_ref()?;
        layout.tile_at(px, py).map(|t| t.node)
    }

    /// Selects the node under the point (or clears the selection when nothing is there).
    pub fn select_at(&mut self, px: f32, py: f32) -> Option<NodeId> {
        self.map.selected = self.tile_at(px, py);
        self.map.selected
    }

    pub fn selection(&self) -> Option<NodeId> {
        self.map.selected
    }

    /// Selects `id`; returns false (selection unchanged) when the node does not exist.
    pub fn select_node(&mut self, id: NodeId) -> bool {
        if self.tree.get(id).is_none() {
            return false;
        }
        self.map.selected = Some(id);
        self.map.lost_selection = None;
        true
    }

    pub fn clear_selection(&mut self) {
        self.map.selected = None;
        self.map.lost_selection = None;
    }

    /// Names from the scan root down to the selected node (root first); `None` without a selection
    /// or when the parent chain is broken. Taken before a tree replacement by
    /// [`Session::restore_selection`].
    pub(crate) fn selection_names(&self) -> Option<Vec<String>> {
        let mut names: Vec<String> = Vec::new();
        let mut cur = self.map.selected?;
        for _ in 0..=MAX_SELECTION_DEPTH {
            let n = self.tree.get(cur)?;
            names.push(n.name().to_owned());
            match n.parent() {
                Some(parent) => cur = parent,
                None => {
                    names.reverse();
                    return Some(names);
                }
            }
        }
        None
    }

    /// Re-selects, in the (new) current tree, the node a selection pointed at before the tree was
    /// replaced: same root name and the same chain of names below it. When the node does not exist
    /// any more the selection stays empty and `selection_lost` reports its former path.
    pub(crate) fn restore_selection(&mut self, names: &[String], display_path: String) {
        let mut parts = names.iter();
        let root = self.tree.root();
        let same_root = parts.next().is_some_and(|first| self.tree.get(root).is_some_and(|n| n.name() == first));
        let found = same_root
            .then(|| {
                parts.try_fold(root, |dir, name| {
                    self.tree.children(dir).find(|&c| self.tree.get(c).is_some_and(|n| n.name() == name.as_str()))
                })
            })
            .flatten();
        match found {
            Some(id) => {
                self.select_node(id);
            }
            None => self.map.lost_selection = Some(display_path),
        }
    }

    /// Path of the selection dropped when the last scan finished (the node is not in the new tree).
    pub fn lost_selection(&self) -> Option<&str> {
        self.map.lost_selection.as_deref()
    }

    /// Serializable snapshot of the map state.
    pub fn map_state_view(&self) -> MapStateView {
        MapStateView {
            revision: self.revision,
            size_mode: match self.map.mode {
                SizeMode::Logical => "logical",
                SizeMode::Allocated => "allocated",
            },
            tile_count: self.map.view.as_ref().map_or(0, |(_, _, l)| l.tiles.len()),
            selected: self.map.selected.map(NodeId::index),
            selected_name: self.map.selected.and_then(|id| self.tree.get(id)).map(|n| n.name().to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::NodeKind;

    fn bounds() -> Rect {
        Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 }
    }

    fn sample_session() -> Session {
        let mut tree = Tree::new("root");
        let root = tree.root();
        let d = tree.add_child(root, "d", NodeKind::Dir, 0).unwrap();
        tree.add_child(d, "big", NodeKind::File, 600).unwrap();
        tree.add_child(d, "small", NodeKind::File, 100).unwrap();
        tree.add_child(root, "other", NodeKind::File, 300).unwrap();
        tree.aggregate();
        let mut s = Session::new();
        s.tree = std::sync::Arc::new(tree);
        s.tree_replaced();
        s
    }

    #[test]
    fn layout_select_and_serialize() {
        let mut s = sample_session();
        let layout = s.map_layout(bounds()).clone();
        assert_eq!(layout.tiles().len(), 5);
        let big = layout.tiles().iter().find(|t| s.tree.get(t.node).is_some_and(|n| n.name() == "big")).unwrap();
        let (cx, cy) = (big.rect.x + big.rect.w / 2.0, big.rect.y + big.rect.h / 2.0);
        assert_eq!(s.tile_at(cx, cy), Some(big.node));
        assert_eq!(s.select_at(cx, cy), Some(big.node));
        let json = serde_json::to_string(&s.map_state_view()).unwrap();
        assert!(json.contains("\"selected_name\":\"big\""), "{json}");
        assert_eq!(s.select_at(-5.0, -5.0), None);
        assert_eq!(s.selection(), None);
    }

    #[test]
    fn cache_is_keyed_by_revision_bounds_and_mode() {
        let mut s = sample_session();
        let first = s.map_layout(bounds()).tiles().as_ptr();
        assert_eq!(s.map_layout(bounds()).tiles().as_ptr(), first, "same key reuses the cache");
        let other = Rect { w: 400.0, ..bounds() };
        assert_eq!(s.map_layout(other).tiles().len(), 5);
        let key_after_bounds = s.map.cache.as_ref().map(|(k, _)| *k).unwrap();
        s.set_map_size_mode(SizeMode::Logical);
        s.map_layout(other);
        assert_ne!(s.map.cache.as_ref().map(|(k, _)| *k).unwrap(), key_after_bounds);
        let rev = s.revision();
        s.select_at(1.0, 1.0);
        s.tree_replaced();
        assert_eq!(s.revision(), rev + 1);
        assert_eq!(s.selection(), None);
        assert_eq!(s.tile_at(1.0, 1.0), None, "cache dropped with the old tree");
    }

    #[test]
    fn small_resizes_reuse_the_expensive_layout_and_stay_inside_the_bounds() {
        let mut s = sample_session();
        s.map_layout(Rect { x: 10.0, y: 20.0, w: 800.0, h: 600.0 });
        let base_ptr = s.map.cache.as_ref().map(|(_, l)| l.tiles().as_ptr()).unwrap();
        for w in 790..=800_u16 {
            let b = Rect { x: 10.0, y: 20.0, w: f32::from(w), h: 590.0 };
            let layout = s.map_layout(b).clone();
            assert_eq!(layout.tiles().len(), 5);
            for t in layout.tiles() {
                let r = t.rect;
                assert!(r.x >= b.x && r.y >= b.y && r.x + r.w <= b.x + b.w && r.y + r.h <= b.y + b.h, "{r:?} in {b:?}");
            }
            let root = layout.tiles().first().unwrap().rect;
            assert!((root.w - b.w).abs() < 0.01 && (root.h - b.h).abs() < 0.01, "root fills the exact bounds");
            assert_eq!(s.map.cache.as_ref().map(|(_, l)| l.tiles().as_ptr()), Some(base_ptr), "layout reused");
            assert!(s.tile_at(b.x + 1.0, b.y + 1.0).is_some());
        }
        s.map_layout(Rect { x: 10.0, y: 20.0, w: 300.0, h: 200.0 });
        assert_ne!(s.map.cache.as_ref().map(|(_, l)| l.tiles().as_ptr()), Some(base_ptr), "big change recomputes");
    }

    #[test]
    fn quantized_bounds_are_coarse_monotonic_and_total() {
        for v in [0.5_f32, 1.0, 15.0, 100.0, 800.0, 1920.0, 1e9] {
            let q = quantize_dim(v);
            assert!(q >= v.min(1_048_576.0) && q <= v.min(1_048_576.0) * 1.26 + 16.0, "{v} -> {q}");
        }
        assert!(quantize_dim(f32::NAN).is_finite());
        let bad = Rect { x: 0.0, y: 0.0, w: f32::NAN, h: 5.0 };
        assert!(quantize_bounds(bad).w.is_nan());
    }

    #[test]
    fn select_node_validates_the_id() {
        let mut s = sample_session();
        let root = s.tree().root();
        assert!(s.select_node(root));
        assert_eq!(s.selection(), Some(root));
        let mut other = Tree::new("x");
        let mut last = other.root();
        for i in 0..20 {
            last = other.add_child(last, &format!("n{i}"), NodeKind::Dir, 0).unwrap();
        }
        assert!(!s.select_node(last), "id outside the arena is rejected");
        assert_eq!(s.selection(), Some(root));
    }

    #[test]
    fn empty_session_and_zero_bounds_are_safe() {
        let mut s = Session::new();
        assert_eq!(s.map_layout(bounds()).tiles().len(), 1);
        assert!(s.map_layout(Rect::default()).tiles().is_empty());
        assert_eq!(s.select_at(f32::NAN, 0.0), None);
    }

    /// Calls `map_view_async` until the outstanding layout arrives (at most 10 s).
    fn settle(s: &mut Session, b: Rect) -> usize {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let (_, layout, pending) = s.map_view_async(b);
            if !pending {
                return layout.tiles().len();
            }
            assert!(std::time::Instant::now() < deadline, "background layout never finished");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn async_layout_arrives_from_the_background_and_matches_the_sync_one() {
        let mut s = sample_session();
        let sync_tiles = s.map_layout(bounds()).tiles().to_vec();
        s.tree_replaced();
        s.set_map_size_mode(SizeMode::Allocated);
        let (_, first, pending) = s.map_view_async(bounds());
        assert!(pending && first.tiles().is_empty(), "no layout yet: empty and pending");
        let n = settle(&mut s, bounds());
        assert_eq!(n, sync_tiles.len());
        let (_, layout, pending) = s.map_view_async(bounds());
        assert!(!pending);
        assert_eq!(layout.tiles(), sync_tiles.as_slice());
    }

    #[test]
    fn resize_keeps_the_previous_picture_while_the_new_layout_is_computed() {
        let mut s = sample_session();
        let small = Rect { x: 0.0, y: 0.0, w: 300.0, h: 200.0 };
        let n_small = settle(&mut s, small);
        assert!(n_small > 1);
        let gen_before = s.map_generation();
        // A much bigger window needs a new base layout.
        let big = Rect { x: 0.0, y: 0.0, w: 1900.0, h: 1000.0 };
        let (_, stale, pending) = s.map_view_async(big);
        assert!(pending, "the new size is being computed");
        assert_eq!(stale.tiles().len(), n_small, "the old tiles are shown, rescaled");
        for t in stale.tiles() {
            assert!(t.rect.x >= 0.0 && t.rect.x + t.rect.w <= big.w + 1e-3);
            assert!(t.rect.y >= 0.0 && t.rect.y + t.rect.h <= big.h + 1e-3);
        }
        assert_ne!(s.map_generation(), gen_before, "rescaled tiles are a new generation");
        settle(&mut s, big);
    }

    #[test]
    fn a_background_layout_of_a_replaced_tree_is_dropped() {
        let mut s = sample_session();
        let (_, _, pending) = s.map_view_async(bounds());
        assert!(pending);
        s.replace_tree(Tree::new("other"));
        let n = settle(&mut s, bounds());
        assert_eq!(n, 1, "only the root of the new tree");
        assert_eq!(s.map.cache.as_ref().map(|(k, _)| k.revision), Some(s.revision()));
    }

    #[test]
    fn async_layout_with_invalid_bounds_is_empty_and_not_pending() {
        let mut s = sample_session();
        let (_, layout, pending) = s.map_view_async(Rect::default());
        assert!(layout.tiles().is_empty() && !pending);
    }

    #[test]
    fn large_tree_tile_count_is_bounded() {
        let mut tree = Tree::new("root");
        let root = tree.root();
        for i in 0..200 {
            let d = tree.add_child(root, &format!("d{i}"), NodeKind::Dir, 0).unwrap();
            for j in 0..1000_u64 {
                tree.add_child(d, &format!("f{j}"), NodeKind::File, j % 97 + 1).unwrap();
            }
        }
        tree.aggregate();
        assert!(tree.len() > 200_000);
        let mut s = Session::new();
        s.tree = std::sync::Arc::new(tree);
        s.tree_replaced();
        let n = s.map_layout(bounds()).tiles().len();
        assert!(n > 200 && n <= LayoutOptions::default().max_tiles, "tile count {n}");
        assert!(s.tile_at(400.0, 300.0).is_some());
    }
}
