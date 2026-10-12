//! Treemap view state of a `Session`: cached hierarchical layout, hit testing and selection.
//!
//! The layout is cached by (tree revision, bounds, size mode), so it is never recomputed per frame.

use crate::Session;
use dirustat_model::{NodeId, SizeMode, Tree};
use dirustat_treemap::{LayoutOptions, Rect, layout_tree};
use serde::Serialize;

/// One laid-out node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapTile {
    pub node: NodeId,
    pub rect: Rect,
    pub depth: u32,
    pub leaf: bool,
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
                Some(MapTile { node, rect: t.rect, depth: t.depth, leaf: t.leaf })
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
    cache: Option<(CacheKey, MapLayout)>,
}

impl MapState {
    pub(crate) fn new() -> Self {
        Self { mode: SizeMode::default(), selected: None, options: LayoutOptions::default(), cache: None }
    }
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
    }

    /// Called when the tree was replaced: bumps the revision and drops selection and cache.
    pub(crate) fn tree_replaced(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.map.selected = None;
        self.map.cache = None;
    }

    /// Cached layout for `bounds` and the current size mode; recomputed only when the key changes.
    pub fn map_layout(&mut self, bounds: Rect) -> &MapLayout {
        let key = CacheKey { revision: self.revision, bounds: bounds_bits(bounds), mode: self.map.mode };
        let fresh = matches!(&self.map.cache, Some((k, _)) if *k == key);
        if !fresh {
            let layout = MapLayout::build(&self.tree, bounds, self.map.mode, self.map.options);
            self.map.cache = Some((key, layout));
        }
        match &self.map.cache {
            Some((_, layout)) => layout,
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
        match &self.map.cache {
            Some((_, layout)) => (&self.tree, layout),
            None => {
                static EMPTY: MapLayout = MapLayout { tiles: Vec::new() };
                (&self.tree, &EMPTY)
            }
        }
    }

    /// Node under the point in the last computed layout (empty when none was computed yet).
    pub fn tile_at(&self, px: f32, py: f32) -> Option<NodeId> {
        let (_, layout) = self.map.cache.as_ref()?;
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
        true
    }

    pub fn clear_selection(&mut self) {
        self.map.selected = None;
    }

    /// Serializable snapshot of the map state.
    pub fn map_state_view(&self) -> MapStateView {
        MapStateView {
            revision: self.revision,
            size_mode: match self.map.mode {
                SizeMode::Logical => "logical",
                SizeMode::Allocated => "allocated",
            },
            tile_count: self.map.cache.as_ref().map_or(0, |(_, l)| l.tiles.len()),
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
        s.tree = tree;
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
        s.tree = tree;
        s.tree_replaced();
        let n = s.map_layout(bounds()).tiles().len();
        assert!(n > 200 && n <= LayoutOptions::default().max_tiles, "tile count {n}");
        assert!(s.tile_at(400.0, 300.0).is_some());
    }
}
