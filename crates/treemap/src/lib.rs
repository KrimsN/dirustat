//! L0: treemap layout (squarified, Bruls/Huizing/van Wijk). Pure geometry, no dependencies.
//!
//! Input is weights in descending order plus a rectangle; output is one rectangle per weight. A weight
//! of 0 yields a zero-area rectangle. The function never panics on any input (NaN, empty bounds).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Rectangle in pixels or abstract units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn area(&self) -> f32 {
        self.w * self.h
    }
}

#[allow(clippy::cast_precision_loss)]
fn weight_to_f64(w: u64) -> f64 {
    // Precision loss on huge weights is acceptable: the layout is visual.
    w as f64
}

fn at(values: &[f64], i: usize) -> f64 {
    values.get(i).copied().unwrap_or(0.0)
}

/// Worst aspect ratio in a row; `max` and `min` are the areas of the row's extreme items.
fn worst_ratio(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    if sum <= 0.0 || min <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = side * side;
    (s2 * max / (sum * sum)).max(sum * sum / (s2 * min))
}

/// Lays out `weights` (descending) inside `bounds`. The result has the same length as `weights`.
pub fn squarify(weights: &[u64], bounds: Rect) -> Vec<Rect> {
    let n = weights.len();
    let mut out = vec![Rect::default(); n];
    let total: f64 = weights.iter().map(|&w| weight_to_f64(w)).sum();
    let (bw, bh) = (f64::from(bounds.w), f64::from(bounds.h));
    if n == 0 || total <= 0.0 || !total.is_finite() || !(bw > 0.0 && bh > 0.0) {
        return out;
    }
    let scale = bw * bh / total;
    let areas: Vec<f64> = weights.iter().map(|&w| weight_to_f64(w) * scale).collect();
    let (mut rx, mut ry, mut rw, mut rh) = (f64::from(bounds.x), f64::from(bounds.y), bw, bh);

    let mut i = 0;
    while i < n && rw > 0.0 && rh > 0.0 {
        let side = rw.min(rh);
        let (mut j, mut sum, mut best) = (i, 0.0, f64::INFINITY);
        while j < n {
            let new_sum = sum + at(&areas, j);
            let worst = worst_ratio(new_sum, at(&areas, i), at(&areas, j), side);
            if j > i && worst > best {
                break;
            }
            best = worst;
            sum = new_sum;
            j += 1;
        }
        if sum <= 0.0 {
            break; // only zero weights remain
        }
        if rw >= rh {
            // column on the left with width sum / rh
            let cw = sum / rh;
            let mut y = ry;
            for k in i..j {
                let h = at(&areas, k) / cw;
                if let Some(slot) = out.get_mut(k) {
                    *slot = to_rect(rx, y, cw, h);
                }
                y += h;
            }
            rx += cw;
            rw -= cw;
        } else {
            // row on top with height sum / rw
            let row_h = sum / rw;
            let mut x = rx;
            for k in i..j {
                let w = at(&areas, k) / row_h;
                if let Some(slot) = out.get_mut(k) {
                    *slot = to_rect(x, ry, w, row_h);
                }
                x += w;
            }
            ry += row_h;
            rh -= row_h;
        }
        i = j;
    }
    out
}

#[allow(clippy::cast_possible_truncation)]
fn to_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }
}

impl Rect {
    /// Half-open containment: left/top edges belong to the rect, right/bottom edges do not, so
    /// adjacent tiles never both contain a shared edge. NaN coordinates and empty rects contain nothing.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        self.w > 0.0 && self.h > 0.0 && px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
}

/// Index of the first rect containing the point, or `None`. For non-overlapping rects the answer is
/// unique and edges are deterministic (see [`Rect::contains`]).
pub fn hit_test(rects: &[Rect], px: f32, py: f32) -> Option<usize> {
    rects.iter().position(|r| r.contains(px, py))
}

/// One laid-out tile of a hierarchical treemap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tile {
    /// Caller-defined node handle (the root is the handle passed to [`layout_tree`]).
    pub id: usize,
    pub rect: Rect,
    /// 0 for the root tile.
    pub depth: u32,
    /// True when the tile was not subdivided (no children, depth cap, too small, or tile cap hit).
    pub leaf: bool,
}

/// Limits for [`layout_tree`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutOptions {
    /// Tiles at this depth are not subdivided.
    pub max_depth: u32,
    /// Children whose rect is narrower or shorter than this are dropped (and not subdivided).
    pub min_size: f32,
    /// Hard cap on emitted tiles.
    pub max_tiles: usize,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self { max_depth: 64, min_size: 2.0, max_tiles: 50_000 }
    }
}

/// Lays out a tree iteratively (no recursion). `children(id, out)` appends `(child id, weight)` for the
/// children of `id`; zero weights are ignored. The result is in parent-before-children order; the first
/// tile is the root. Children of a tile partition (a part of) its rect proportionally to weight.
/// Empty or non-finite bounds yield no tiles.
pub fn layout_tree<F>(root: usize, bounds: Rect, opts: LayoutOptions, mut children: F) -> Vec<Tile>
where
    F: FnMut(usize, &mut Vec<(usize, u64)>),
{
    let mut tiles: Vec<Tile> = Vec::new();
    let valid = bounds.x.is_finite()
        && bounds.y.is_finite()
        && bounds.w.is_finite()
        && bounds.h.is_finite()
        && bounds.w > 0.0
        && bounds.h > 0.0;
    if !valid || opts.max_tiles == 0 {
        return tiles;
    }
    tiles.push(Tile { id: root, rect: bounds, depth: 0, leaf: true });
    // Expansion is largest-area-first (ties: earliest tile), so when `max_tiles` is hit the cap cuts the
    // smallest tiles instead of starving the biggest directories. Entries are (area bits, Reverse(index));
    // the bits of a non-negative f32 order like the value.
    let mut queue: BinaryHeap<(u32, Reverse<usize>)> = BinaryHeap::new();
    queue.push((bounds.area().to_bits(), Reverse(0)));
    let mut kids: Vec<(usize, u64)> = Vec::new();
    let mut weights: Vec<u64> = Vec::new();
    while let Some((_, Reverse(ti))) = queue.pop() {
        let Some(tile) = tiles.get(ti).copied() else { continue };
        if tile.depth >= opts.max_depth || tile.rect.w < opts.min_size || tile.rect.h < opts.min_size {
            continue;
        }
        kids.clear();
        children(tile.id, &mut kids);
        kids.retain(|&(_, w)| w > 0);
        if kids.is_empty() {
            continue;
        }
        kids.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        weights.clear();
        weights.extend(kids.iter().map(|&(_, w)| w));
        let rects = squarify(&weights, tile.rect);
        let mut emitted = false;
        for (&(id, _), rect) in kids.iter().zip(rects) {
            if tiles.len() >= opts.max_tiles {
                break;
            }
            // Keep only visible tiles; the negated comparison also rejects NaN.
            if !(rect.w >= opts.min_size && rect.h >= opts.min_size) {
                continue;
            }
            emitted = true;
            tiles.push(Tile { id, rect, depth: tile.depth.saturating_add(1), leaf: true });
            queue.push((rect.area().to_bits(), Reverse(tiles.len().saturating_sub(1))));
        }
        if emitted && let Some(t) = tiles.get_mut(ti) {
            t.leaf = false;
        }
    }
    tiles
}

/// Deepest tile containing the point (ties: the later one). Parents contain their children, so a
/// point in a gap left by dropped tiny children resolves to the parent.
pub fn hit_test_tiles(tiles: &[Tile], px: f32, py: f32) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (i, t) in tiles.iter().enumerate() {
        if t.rect.contains(px, py) && best.is_none_or(|(_, d)| t.depth >= d) {
            best = Some((i, t.depth));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_are_proportional_and_fill_bounds() {
        let weights = [60, 30, 6, 3, 1];
        let bounds = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let rects = squarify(&weights, bounds);
        let total: f32 = rects.iter().map(Rect::area).sum();
        assert!((total - bounds.area()).abs() < 1.0, "sum of areas {total}");
        assert!((rects[0].area() / bounds.area() - 0.6).abs() < 0.01);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let b = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        assert!(squarify(&[], b).is_empty());
        assert_eq!(squarify(&[0, 0], b), vec![Rect::default(); 2]);
        assert_eq!(squarify(&[1, 2], Rect::default()), vec![Rect::default(); 2]);
        let nan = Rect { x: 0.0, y: 0.0, w: f32::NAN, h: 5.0 };
        assert_eq!(squarify(&[1], nan).len(), 1);
        assert_eq!(squarify(&[u64::MAX, u64::MAX], b).len(), 2);
    }

    fn bounds() -> Rect {
        Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 }
    }

    #[test]
    fn hit_test_edges_are_deterministic() {
        let a = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        let c = Rect { x: 10.0, y: 0.0, w: 10.0, h: 10.0 };
        assert_eq!(hit_test(&[a, c], 0.0, 0.0), Some(0));
        assert_eq!(hit_test(&[a, c], 10.0, 5.0), Some(1));
        assert_eq!(hit_test(&[a, c], 20.0, 5.0), None);
        assert_eq!(hit_test(&[a, c], f32::NAN, 5.0), None);
        assert_eq!(hit_test(&[Rect::default()], 0.0, 0.0), None);
    }

    #[test]
    fn hit_test_finds_exactly_one_rect_on_a_grid_of_points() {
        let rects = squarify(&[40, 25, 20, 10, 5], bounds());
        for xi in 0..80_u16 {
            for yi in 0..60_u16 {
                let (px, py) = (f32::from(xi) * 5.0 + 0.5, f32::from(yi) * 5.0 + 0.5);
                let n = rects.iter().filter(|r| r.contains(px, py)).count();
                assert_eq!(n, 1, "point {px},{py}");
            }
        }
    }

    // node 0 -> 1..=3 ; 1 -> 4..=5
    fn small(id: usize, out: &mut Vec<(usize, u64)>) {
        match id {
            0 => out.extend([(1, 60), (2, 30), (3, 10)]),
            1 => out.extend([(4, 40), (5, 20)]),
            _ => {}
        }
    }

    fn overlap(p: &Rect, q: &Rect) -> bool {
        let ox = (p.x + p.w).min(q.x + q.w) - p.x.max(q.x);
        let oy = (p.y + p.h).min(q.y + q.h) - p.y.max(q.y);
        ox > 0.01 && oy > 0.01
    }

    #[test]
    fn layout_tree_partitions_and_is_proportional() {
        let tiles = layout_tree(0, bounds(), LayoutOptions::default(), small);
        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles.first().map(|t| t.id), Some(0));
        assert!(tiles.first().is_some_and(|t| !t.leaf));
        let area = |id: usize| tiles.iter().find(|t| t.id == id).map_or(0.0, |t| t.rect.area());
        assert!((area(1) / bounds().area() - 0.6).abs() < 0.01);
        assert!((area(4) / area(1) - 40.0 / 60.0).abs() < 0.01);
        let kids: f32 = [1, 2, 3].iter().map(|&i| area(i)).sum();
        assert!((kids - bounds().area()).abs() < 1.0);
        let leaves: Vec<&Tile> = tiles.iter().filter(|t| t.leaf).collect();
        for (i, p) in leaves.iter().enumerate() {
            for q in leaves.iter().skip(i + 1) {
                assert!(!overlap(&p.rect, &q.rect), "overlap {p:?} {q:?}");
            }
        }
        let inner = tiles.iter().find(|t| t.id == 4).unwrap();
        let idx = hit_test_tiles(&tiles, inner.rect.x + 0.5, inner.rect.y + 0.5).unwrap();
        assert_eq!(tiles[idx].id, 4);
    }

    #[test]
    fn layout_tree_respects_caps_and_degenerate_input() {
        let depth1 = LayoutOptions { max_depth: 1, ..LayoutOptions::default() };
        assert_eq!(layout_tree(0, bounds(), depth1, small).len(), 4);
        let cap = LayoutOptions { max_tiles: 3, ..LayoutOptions::default() };
        assert_eq!(layout_tree(0, bounds(), cap, small).len(), 3);
        let min = LayoutOptions { min_size: 1000.0, ..LayoutOptions::default() };
        assert_eq!(layout_tree(0, bounds(), min, small).len(), 1);
        let nan = Rect { x: 0.0, y: 0.0, w: f32::NAN, h: 5.0 };
        let inf = Rect { x: 0.0, y: 0.0, w: f32::INFINITY, h: 5.0 };
        for bad in [Rect::default(), nan, inf] {
            assert!(layout_tree(0, bad, LayoutOptions::default(), small).is_empty());
        }
        let zero = layout_tree(0, bounds(), LayoutOptions::default(), |_, out| out.extend([(1, 0), (2, 0)]));
        assert_eq!(zero.len(), 1);
        assert!(zero.first().is_some_and(|t| t.leaf));
        let huge = layout_tree(0, bounds(), LayoutOptions::default(), |id, out| {
            if id == 0 {
                out.extend([(1, u64::MAX), (2, u64::MAX)]);
            }
        });
        assert_eq!(huge.len(), 3);
    }

    /// Root -> big dir 1 (8 files, ids 100..108) plus 20 tiny dirs (ids 2..=21, 2 files each).
    fn big_and_tiny(id: usize, out: &mut Vec<(usize, u64)>) {
        match id {
            0 => {
                out.push((1, 1000));
                out.extend((2..=21).map(|d| (d, 1)));
            }
            1 => out.extend((100..108).map(|f| (f, 10))),
            2..=21 => out.extend([(id * 100, 1), (id * 100 + 1, 1)]),
            _ => {}
        }
    }

    #[test]
    fn tile_cap_cuts_the_smallest_tiles_not_the_largest_directories() {
        // 22 tiles after the root is expanded; 10 spare slots are enough for the big dir only.
        let opts = LayoutOptions { max_tiles: 32, min_size: 0.5, ..LayoutOptions::default() };
        let tiles = layout_tree(0, bounds(), opts, big_and_tiny);
        assert_eq!(tiles.len(), 32);
        let big_files = tiles.iter().filter(|t| (100..108).contains(&t.id)).count();
        assert_eq!(big_files, 8, "the largest directory must be expanded before the cap is spent");
        let big = tiles.iter().find(|t| t.id == 1).unwrap();
        assert!(!big.leaf);
    }

    #[test]
    fn layout_tree_deep_chain_does_not_recurse() {
        let opts = LayoutOptions { max_depth: 100_000, min_size: 0.0, max_tiles: 100_000 };
        let tiles = layout_tree(0, Rect { x: 0.0, y: 0.0, w: 1e6, h: 1e6 }, opts, |id, out| {
            if id < 50_000 {
                out.push((id + 1, 1));
            }
        });
        assert_eq!(tiles.len(), 50_001);
    }
}
