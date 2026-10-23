use super::*;
use crate::{LayoutOptions, layout_tree};

/// Small deterministic generator so the property tests need no dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    /// Uniform in `[lo, hi)`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next() % 1_000_000) as f32 / 1_000_000.0;
        lo + (hi - lo) * unit
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn hostile_values() -> Vec<f32> {
    vec![
        0.0,
        -0.0,
        1.0,
        -1.0,
        1e-30,
        1e30,
        f32::MAX,
        f32::MIN,
        f32::MIN_POSITIVE,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ]
}

#[test]
fn brightness_is_finite_and_in_unit_range_for_random_surfaces() {
    let mut rng = Lcg(7);
    let shader = Shader::default();
    for _ in 0..2_000 {
        let mut s = shader.base();
        for _ in 0..(rng.next() % 12) {
            let r =
                rect(rng.range(-50.0, 500.0), rng.range(-50.0, 500.0), rng.range(0.0, 300.0), rng.range(0.0, 300.0));
            s = shader.add_ridge(s, r);
        }
        for _ in 0..16 {
            let b = shader.brightness(&s, rng.range(-100.0, 700.0), rng.range(-100.0, 700.0));
            assert!(b.is_finite() && (0.0..=1.0).contains(&b), "brightness {b}");
        }
    }
}

#[test]
fn hostile_rects_configs_and_points_never_break_the_range() {
    let values = hostile_values();
    for &height in &values {
        for &light in &values {
            let cfg =
                CushionConfig { height, scale: light, ambient: height, diffuse: light, light: [light, height, light] };
            let shader = Shader::new(cfg);
            let c = shader.config();
            assert!(c.height.is_finite() && (0.0..=MAX_HEIGHT).contains(&c.height));
            for &v in &values {
                let r = rect(v, height, light, v);
                let s = shader.add_ridge(shader.add_ridge(shader.base(), r), rect(0.0, 0.0, 10.0, 10.0));
                for &p in &values {
                    let b = shader.brightness(&s, p, v);
                    assert!(b.is_finite() && (0.0..=1.0).contains(&b), "{cfg:?} {r:?} {p}: {b}");
                }
                let mut seen = 0_u32;
                shader.fill_rect(&s, r, (8, 8), |_, _, b| {
                    assert!((0.0..=1.0).contains(&b));
                    seen += 1;
                });
                assert!(seen <= 64);
            }
        }
    }
}

#[test]
fn shading_is_deterministic() {
    let shader = Shader::default();
    let build = || {
        let mut s = shader.base();
        for r in [rect(0.0, 0.0, 400.0, 300.0), rect(10.5, 20.25, 120.0, 90.5), rect(12.0, 22.0, 30.0, 40.0)] {
            s = shader.add_ridge(s, r);
        }
        s
    };
    let (a, b) = (build(), build());
    assert_eq!(a, b);
    for i in 0..50_u16 {
        let (x, y) = (f32::from(i) * 7.3, f32::from(i) * 5.1);
        assert_eq!(shader.brightness(&a, x, y).to_bits(), shader.brightness(&b, x, y).to_bits());
    }
}

#[test]
fn flat_surface_has_constant_ambient_plus_diffuse() {
    let cfg = CushionConfig { height: 0.0, ..CushionConfig::default() };
    let shader = Shader::new(cfg);
    let [lx, ly, lz] = cfg.light.map(f64::from);
    let expected = f64::from(cfg.ambient) + f64::from(cfg.diffuse) * lz / lx.hypot(ly).hypot(lz);
    let mut s = shader.base();
    for r in [rect(0.0, 0.0, 400.0, 300.0), rect(10.0, 10.0, 50.0, 50.0)] {
        s = shader.add_ridge(s, r);
    }
    assert!(s.is_flat());
    for (x, y) in [(0.0, 0.0), (15.0, 15.0), (399.0, 299.0), (-5.0, 1e6)] {
        let b = f64::from(shader.brightness(&s, x, y));
        assert!((b - expected).abs() < 1e-6, "{b} vs {expected}");
    }
    // The same holds for the surface before any ridge, at the default height.
    let plain = Shader::default();
    let b = plain.brightness(&plain.base(), 3.0, 4.0);
    assert!((f64::from(b) - 0.4 - 0.6 * 2.5 / (1.0_f64 + 1.0 + 6.25).sqrt()).abs() < 1e-6);
}

#[test]
fn ridge_follows_the_paper_formula_and_lights_the_side_facing_the_light() {
    let shader = Shader::default();
    let r = rect(100.0, 50.0, 200.0, 100.0);
    let s = shader.add_ridge(shader.base(), r);
    let h = f64::from(shader.config().height);
    assert!((s.ax + 4.0 * h / 200.0).abs() < 1e-12);
    assert!((s.bx - 4.0 * h * 400.0 / 200.0).abs() < 1e-9);
    assert!((s.ay + 4.0 * h / 100.0).abs() < 1e-12);
    assert!((s.by - 4.0 * h * 200.0 / 100.0).abs() < 1e-9);
    // The ridge peaks at the centre (no slope) and falls to zero height at the edges.
    let centre_b = shader.brightness(&s, 200.0, 100.0);
    let flat_b = shader.brightness(&shader.base(), 200.0, 100.0);
    assert!((centre_b - flat_b).abs() < 1e-6);
    // Light comes from the upper left: the left/top slope is brighter than the right/bottom slope.
    assert!(shader.brightness(&s, 110.0, 100.0) > shader.brightness(&s, 290.0, 100.0));
    assert!(shader.brightness(&s, 200.0, 55.0) > shader.brightness(&s, 200.0, 145.0));
}

#[test]
fn deeper_levels_add_smaller_ridges() {
    let shader = Shader::default();
    let mut s = shader.base();
    let mut last = f64::INFINITY;
    for _ in 0..10 {
        let before = s;
        s = shader.add_ridge(s, rect(0.0, 0.0, 100.0, 100.0));
        let step = (s.ax - before.ax).abs();
        assert!(step < last && step > 0.0);
        last = step;
    }
}

#[test]
fn degenerate_rects_add_no_ridge_but_count_a_level() {
    let shader = Shader::default();
    let base = shader.base();
    for bad in [
        rect(0.0, 0.0, 0.0, 10.0),
        rect(0.0, 0.0, 10.0, -1.0),
        rect(f32::NAN, 0.0, 10.0, 10.0),
        rect(0.0, 0.0, f32::INFINITY, 10.0),
        rect(0.0, 0.0, f32::NAN, 10.0),
    ] {
        let s = shader.add_ridge(base, bad);
        assert!(s.is_flat(), "{bad:?}");
        assert!(s.next_height < base.next_height);
    }
    // A width so small that 4h/w overflows the accumulator is discarded, not turned into infinity.
    let s = shader.add_ridge(base, rect(1e38, 0.0, f32::MIN_POSITIVE, 10.0));
    assert!(s.ax.is_finite() && s.bx.is_finite());
}

#[test]
fn sanitized_config_is_always_valid() {
    let d = CushionConfig::default();
    let bad = CushionConfig { height: f32::NAN, scale: 7.0, ambient: -1.0, diffuse: f32::INFINITY, light: [0.0; 3] };
    let c = bad.sanitized();
    assert_eq!((c.height, c.light, c.diffuse), (d.height, d.light, d.diffuse));
    assert_eq!((c.scale, c.ambient), (1.0, 0.0));
    assert_eq!(d.sanitized(), d);
    assert_eq!(CushionConfig { height: 100.0, ..d }.sanitized().height, MAX_HEIGHT);
}

#[test]
fn tile_surfaces_accumulate_ancestors_and_ignore_bad_parents() {
    let shader = Shader::default();
    let rects = [
        rect(0.0, 0.0, 100.0, 100.0),
        rect(0.0, 0.0, 50.0, 100.0),
        rect(50.0, 0.0, 50.0, 100.0),
        rect(0.0, 0.0, 25.0, 50.0),
    ];
    let parents = [None, Some(0), Some(0), Some(1)];
    let s = shader.tile_surfaces(&parents, &rects);
    assert_eq!(s.len(), 4);
    let by_hand = rects
        .iter()
        .enumerate()
        .filter(|(i, _)| [0, 1, 3].contains(i))
        .fold(shader.base(), |acc, (_, r)| shader.add_ridge(acc, *r));
    assert_eq!(s.get(3), Some(&by_hand));
    // Self, forward and out-of-range parents (cycles included) count as "no parent"; short slices are fine.
    let wild = [Some(0), Some(5), Some(2), Some(usize::MAX)];
    let w = shader.tile_surfaces(&wild, &rects);
    let alone = shader.add_ridge(shader.base(), rect(0.0, 0.0, 100.0, 100.0));
    assert_eq!(w.first(), Some(&alone));
    assert_eq!(w.get(1), Some(&shader.add_ridge(shader.base(), rect(0.0, 0.0, 50.0, 100.0))));
    assert_eq!(shader.tile_surfaces(&[], &rects).len(), 4);
    assert!(shader.tile_surfaces(&parents, &[]).is_empty());
}

#[test]
fn a_very_deep_chain_is_iterative_and_converges() {
    let shader = Shader::default();
    let n = 200_000_usize;
    let rects: Vec<Rect> = (0..n).map(|_| rect(0.0, 0.0, 100.0, 100.0)).collect();
    let parents: Vec<Option<usize>> = (0..n).map(|i| i.checked_sub(1)).collect();
    let s = shader.tile_surfaces(&parents, &rects);
    assert_eq!(s.len(), n);
    let last = s.last().copied().unwrap_or_default();
    assert_eq!(last.next_height, 0.0, "ridge heights die out instead of accumulating forever");
    let b = shader.brightness(&last, 5.0, 5.0);
    assert!((0.0..=1.0).contains(&b));
}

fn child_weights(id: usize, out: &mut Vec<(usize, u64)>) {
    match id {
        0 => out.extend((1..=9_usize).map(|i| (i, 1000 - 50 * u64::try_from(i).unwrap_or(0)))),
        1..=9 => out.extend((0..7_usize).map(|k| (id * 100 + k, 1 + (u64::try_from(k).unwrap_or(0) + 1).pow(2)))),
        _ => {}
    }
}

#[test]
fn shading_leaves_covers_every_pixel_once_and_keeps_the_layout_untouched() {
    for (w, h) in [(160_u32, 120_u32), (333, 177), (97, 301)] {
        let bounds = rect(0.0, 0.0, w as f32, h as f32);
        let tiles = layout_tree(0, bounds, LayoutOptions { min_size: 1.0, ..LayoutOptions::default() }, child_weights);
        let before = tiles.clone();
        // Parent = the non-remainder tile with the parent's id one level up (ids are unique here).
        let parents: Vec<Option<usize>> = tiles
            .iter()
            .map(|t| {
                if t.depth == 0 {
                    return None;
                }
                let want = if t.remainder {
                    t.id
                } else if t.id >= 100 {
                    t.id / 100
                } else {
                    0
                };
                tiles.iter().position(|p| p.id == want && !p.remainder && p.depth + 1 == t.depth)
            })
            .collect();
        let rects: Vec<Rect> = tiles.iter().map(|t| t.rect).collect();
        let shader = Shader::default();
        let surfaces = shader.tile_surfaces(&parents, &rects);
        let mut hits = vec![0_u8; (w * h) as usize];
        for (t, s) in tiles.iter().zip(&surfaces).filter(|(t, _)| t.leaf) {
            shader.fill_rect(s, t.rect, (w, h), |px, py, b| {
                assert!((0.0..=1.0).contains(&b));
                if let Some(c) = hits.get_mut((py * w + px) as usize) {
                    *c += 1;
                }
            });
        }
        assert!(hits.iter().all(|&c| c == 1), "{w}x{h}: every pixel is painted by exactly one leaf");
        assert_eq!(tiles, before, "shading never changes the layout");
        assert_eq!(tiles.iter().map(|t| t.rect).collect::<Vec<_>>(), rects);
    }
}

#[test]
fn neighbours_that_disagree_by_an_ulp_on_a_pixel_centre_edge_leave_no_gap() {
    let shader = Shader::default();
    let s = shader.base();
    // Both tiles meet at x = 6.5, a pixel centre; the left one ends an ulp short, the right starts an ulp late.
    let edge = 6.5_f32;
    let below = f32::from_bits(edge.to_bits() - 1);
    let above = f32::from_bits(edge.to_bits() + 1);
    for (left_end, right_start) in [(below, above), (above, below), (edge, above), (below, edge)] {
        let mut hits = vec![0_u8; 13 * 3];
        let left = rect(0.0, 0.0, left_end, 3.0);
        let right = rect(right_start, 0.0, 13.0 - right_start, 3.0);
        for r in [left, right] {
            shader.fill_rect(&s, r, (13, 3), |px, py, _| {
                if let Some(c) = hits.get_mut((py * 13 + px) as usize) {
                    *c += 1;
                }
            });
        }
        assert!(hits.iter().all(|&c| c == 1), "{left_end} | {right_start}: {hits:?}");
    }
}

#[test]
fn fill_rect_clips_and_skips_empty_rects() {
    let shader = Shader::default();
    let s = shader.base();
    let mut count = 0_u32;
    shader.fill_rect(&s, rect(-10.0, -10.0, 1000.0, 1000.0), (5, 4), |px, py, _| {
        assert!(px < 5 && py < 4);
        count += 1;
    });
    assert_eq!(count, 20);
    for bad in [
        rect(0.0, 0.0, 0.0, 0.0),
        rect(f32::NAN, 0.0, 5.0, 5.0),
        rect(0.0, 0.0, f32::INFINITY, 5.0),
        rect(9.0, 9.0, 3.0, 3.0),
    ] {
        shader.fill_rect(&s, bad, (5, 4), |_, _, _| count += 1);
    }
    assert_eq!(count, 20);
    shader.fill_rect(&s, rect(0.0, 0.0, 5.0, 4.0), (0, 0), |_, _, _| count += 1);
    assert_eq!(count, 20);
    // Pixel centres decide: [0.4, 1.6) holds the centres 0.5 and 1.5 only.
    let mut px = Vec::new();
    shader.fill_rect(&s, rect(0.4, 0.0, 1.2, 1.0), (5, 4), |x, _, _| px.push(x));
    assert_eq!(px, [0, 1]);
}
