//! Measures cushion shading on synthetic trees of 100k and 1M nodes: `cargo run --release -p
//! dirustat-treemap --example cushion_bench`. Prints layout, surface accumulation and per-pixel shading
//! times and the number of pixels no leaf tile covers (must be 0).

#![allow(clippy::print_stdout)]

use dirustat_treemap::{LayoutOptions, Rect, Shader, Tile, layout_tree};
use std::time::Instant;

/// A three-level synthetic tree with skewed weights. Ids are dense: 0 is the root, `1..=dirs` the
/// directories, then `dirs * subdirs` subdirectories, then the files (`files` per subdirectory).
#[derive(Clone, Copy)]
struct Shape {
    dirs: usize,
    subdirs: usize,
    files: usize,
}

impl Shape {
    fn nodes(self) -> usize {
        1 + self.dirs + self.dirs * self.subdirs + self.dirs * self.subdirs * self.files
    }

    fn first_file(self) -> usize {
        1 + self.dirs + self.dirs * self.subdirs
    }

    fn weight(k: usize) -> u64 {
        1 + u64::try_from(k.wrapping_mul(2_654_435_761) % 1000).unwrap_or(1).pow(2)
    }

    fn children(self, id: usize, out: &mut Vec<(usize, u64)>) {
        let sub_base = 1 + self.dirs;
        if id == 0 {
            out.extend((1..=self.dirs).map(|d| (d, Self::weight(d) * 50)));
        } else if id <= self.dirs {
            let first = sub_base + (id - 1) * self.subdirs;
            out.extend((0..self.subdirs).map(|s| (first + s, Self::weight(first + s) * 5)));
        } else if id < self.first_file() {
            let first = self.first_file() + (id - sub_base) * self.files;
            out.extend((0..self.files).map(|f| (first + f, Self::weight(first + f))));
        }
    }

    fn parent(self, id: usize) -> Option<usize> {
        let sub_base = 1 + self.dirs;
        match id {
            0 => None,
            _ if id <= self.dirs => Some(0),
            _ if id < self.first_file() => Some((id - sub_base) / self.subdirs + 1),
            _ => Some((id - self.first_file()) / self.files + sub_base),
        }
    }
}

fn parents_of(shape: Shape, tiles: &[Tile]) -> Vec<Option<usize>> {
    let mut by_id = std::collections::HashMap::new();
    for (i, t) in tiles.iter().enumerate().filter(|(_, t)| !t.remainder) {
        by_id.insert(t.id, i);
    }
    tiles
        .iter()
        .map(|t| {
            let key = if t.remainder { Some(t.id) } else { shape.parent(t.id) };
            key.and_then(|k| by_id.get(&k).copied())
        })
        .collect()
}

fn run(shape: Shape, width: u32, height: u32, max_tiles: usize) {
    let bounds = Rect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 };
    let opts = LayoutOptions { max_tiles, ..LayoutOptions::default() };
    let t0 = Instant::now();
    let tiles = layout_tree(0, bounds, opts, |id, out| shape.children(id, out));
    let layout_ms = t0.elapsed().as_secs_f64() * 1e3;
    let parents = parents_of(shape, &tiles);
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
                *p = 1 + (b * 255.0) as u32;
            }
        });
    }
    let fill_ms = t2.elapsed().as_secs_f64() * 1e3;
    let covered = pixels.iter().filter(|&&p| p != 0).count();
    println!(
        "{} nodes, {width}x{height} max_tiles={max_tiles}: {} tiles ({} leaves), layout {layout_ms:.1} ms, \
         surfaces {surf_ms:.2} ms, shading {fill_ms:.1} ms for {covered} px ({:.1} ns/px), uncovered {}, \
         total shading {:.1} ms",
        shape.nodes(),
        tiles.len(),
        tiles.iter().filter(|t| t.leaf).count(),
        fill_ms * 1e6 / covered.max(1) as f64,
        pixels.len() - covered,
        surf_ms + fill_ms,
    );
}

fn main() {
    let k100 = Shape { dirs: 100, subdirs: 100, files: 9 };
    let m1 = Shape { dirs: 100, subdirs: 100, files: 99 };
    for _ in 0..2 {
        run(k100, 1920, 1080, 50_000);
    }
    run(k100, 1920, 1080, 200_000);
    run(k100, 3840, 2160, 50_000);
    for _ in 0..2 {
        run(m1, 1920, 1080, 50_000);
    }
    run(m1, 1920, 1080, 200_000);
    run(m1, 3840, 2160, 50_000);
}
