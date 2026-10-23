//! Measures cushion shading on a synthetic 100k-node tree: `cargo run --release -p dirustat-treemap
//! --example cushion_bench`. Prints layout, surface accumulation and per-pixel shading times.

#![allow(clippy::print_stdout)]

use dirustat_treemap::{LayoutOptions, Rect, Shader, Tile, layout_tree};
use std::time::Instant;

/// Dirs per level and files per dir are derived from the node id so the tree is deterministic and
/// skewed: 1 root, 100 dirs, 100 subdirs each, 9 files per subdir = 100_101 + 90_000 nodes.
fn children(id: usize, out: &mut Vec<(usize, u64)>) {
    const DIRS: usize = 100;
    const SUBDIRS: usize = 100;
    const FILES: usize = 9;
    let weight = |k: usize| 1 + u64::try_from(k.wrapping_mul(2_654_435_761) % 1000).unwrap_or(1).pow(2);
    match id {
        0 => out.extend((1..=DIRS).map(|d| (d, weight(d) * 50))),
        1..=DIRS => out.extend((0..SUBDIRS).map(|s| (1000 * id + s + 1, weight(id * 1000 + s) * 5))),
        _ if id >= 1000 && id % 1000 <= SUBDIRS => {
            out.extend((0..FILES).map(|f| (1_000_000 * id + f + 1, weight(id * 31 + f))));
        }
        _ => {}
    }
}

fn parents_of(tiles: &[Tile]) -> Vec<Option<usize>> {
    // Tiles are parent-before-children; the nearest earlier tile one level up that contains the
    // child's centre is its parent. Linear scan per level via the last tile seen at each depth is not
    // exact for a priority-ordered list, so use a map keyed by the id structure instead.
    let mut by_id = std::collections::HashMap::new();
    for (i, t) in tiles.iter().enumerate().filter(|(_, t)| !t.remainder) {
        by_id.insert(t.id, i);
    }
    tiles
        .iter()
        .map(|t| {
            let parent_id = match t.id {
                0 => return None,
                1..=100 => 0,
                id if id < 1_000_000 => id / 1000,
                id => id / 1_000_000,
            };
            let key = if t.remainder { t.id } else { parent_id };
            by_id.get(&key).copied()
        })
        .collect()
}

fn run(width: u32, height: u32, max_tiles: usize) {
    let bounds = Rect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
    let opts = LayoutOptions { max_tiles, ..LayoutOptions::default() };
    let t0 = Instant::now();
    let tiles = layout_tree(0, bounds, opts, children);
    let layout_ms = t0.elapsed().as_secs_f64() * 1e3;
    let parents = parents_of(&tiles);
    let rects: Vec<Rect> = tiles.iter().map(|t| t.rect).collect();
    let shader = Shader::default();

    let t1 = Instant::now();
    let surfaces = shader.tile_surfaces(&parents, &rects);
    let surf_ms = t1.elapsed().as_secs_f64() * 1e3;

    let mut pixels = vec![0_u32; (width * height) as usize];
    let t2 = Instant::now();
    for (tile, surface) in tiles.iter().zip(&surfaces).filter(|(t, _)| t.leaf) {
        shader.fill_rect(surface, tile.rect, (width, height), |px, py, b| {
            if let Some(p) = pixels.get_mut((py * width + px) as usize) {
                *p = (b * 255.0) as u32;
            }
        });
    }
    let fill_ms = t2.elapsed().as_secs_f64() * 1e3;
    let covered = pixels.iter().filter(|&&p| p != 0).count();
    println!(
        "{width}x{height} max_tiles={max_tiles}: {} tiles ({} leaves), layout {layout_ms:.1} ms, surfaces {surf_ms:.2} ms, \
         shading {fill_ms:.1} ms for {covered} px ({:.1} ns/px), total shading {:.1} ms",
        tiles.len(),
        tiles.iter().filter(|t| t.leaf).count(),
        fill_ms * 1e6 / covered.max(1) as f64,
        surf_ms + fill_ms,
    );
}

fn main() {
    for _ in 0..2 {
        run(1920, 1080, 50_000);
    }
    run(1920, 1080, 200_000);
    run(3840, 2160, 50_000);
}
