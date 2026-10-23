//! Cushion shading for treemaps (van Wijk and van de Wetering, "Cushion Treemaps: Visualization of
//! Hierarchical Information", InfoVis 1999). Pure geometry and arithmetic, no dependencies.
//!
//! Every tile of the hierarchy adds one parabolic *ridge* to a surface. A tile's surface is the sum of
//! the ridges of all its ancestors and its own; the height of the ridge added at nesting level `n` is
//! `height * scale^n`, so deeper levels are flatter. For a ridge over `[x1, x2]` of relative height `h`
//! the paper gives `z(x) = -4h/(x2-x1) * (x - x1) * (x - x2)`, i.e. the coefficients of the surface
//! `z = ax x^2 + bx x + ay y^2 + by y + c` change by
//!
//! ```text
//! ax -= 4h / (x2 - x1)        bx += 4h (x2 + x1) / (x2 - x1)
//! ```
//!
//! (and the same for `y`). The constant `c` never matters for lighting and is not stored. The surface
//! normal at a point is `(-dz/dx, -dz/dy, 1)`; the brightness is Lambert shading with an ambient term:
//! `ambient + diffuse * max(0, n . l / |n|)` for the unit light direction `l`.
//!
//! Layout and shading are independent: nothing here changes a rectangle. Hostile numbers (NaN, infinity,
//! zero-size rectangles, absurd parents) are neutralised and never panic; the brightness is always a
//! finite number in `[0, 1]`.

use crate::Rect;

/// Ridge heights at or below this are treated as zero: deeper levels no longer change the picture.
const MIN_RIDGE_HEIGHT: f64 = 1e-9;
/// Upper bound for the root ridge height; larger values only produce black and white noise.
const MAX_HEIGHT: f32 = 4.0;

/// Parameters of the cushion look. [`CushionConfig::sanitized`] makes any value usable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CushionConfig {
    /// Relative height `h0` of the ridge of the root tile (peak height is `h0` times the tile width).
    /// `0` gives a flat picture. Larger values make edges darker/brighter. Clamped to `0..=4`.
    pub height: f32,
    /// Factor `f` in `0..=1` by which the ridge height shrinks per nesting level.
    pub scale: f32,
    /// Ambient light `Ia` in `0..=1`: the brightness of a surface facing away from the light.
    pub ambient: f32,
    /// Diffuse light `Is` in `0..=1`: added in proportion to the cosine between normal and light. For
    /// `ambient + diffuse <= 1` a surface facing the light is at most fully bright.
    pub diffuse: f32,
    /// Direction *towards* the light in surface space: x to the right, y down, z towards the viewer.
    /// Need not be normalised. A zero or non-finite vector falls back to the default direction.
    pub light: [f32; 3],
}

impl Default for CushionConfig {
    /// A mild cushion lit from the upper left: ambient 0.4, diffuse 0.6, root ridge height 0.22,
    /// scale factor 0.85 per level.
    fn default() -> Self {
        Self { height: 0.22, scale: 0.85, ambient: 0.4, diffuse: 0.6, light: [-1.0, -1.0, 2.5] }
    }
}

impl CushionConfig {
    /// The nearest valid configuration: non-finite values become the default, the rest is clamped.
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let unit = |v: f32, fallback: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { fallback };
        let [lx, ly, lz] = self.light;
        let light_ok = lx.is_finite() && ly.is_finite() && lz.is_finite() && lx.hypot(ly).hypot(lz) > 1e-6;
        Self {
            height: if self.height.is_finite() { self.height.clamp(0.0, MAX_HEIGHT) } else { d.height },
            scale: unit(self.scale, d.scale),
            ambient: unit(self.ambient, d.ambient),
            diffuse: unit(self.diffuse, d.diffuse),
            light: if light_ok { self.light } else { d.light },
        }
    }
}

/// The accumulated surface of one tile: the sum of the ridges of the tile and all its ancestors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Surface {
    ax: f64,
    bx: f64,
    ay: f64,
    by: f64,
    /// Relative height of the ridge the next nesting level adds.
    next_height: f64,
}

impl Surface {
    /// True when no ridge was added (or all of them were flat).
    pub fn is_flat(&self) -> bool {
        self.ax == 0.0 && self.bx == 0.0 && self.ay == 0.0 && self.by == 0.0
    }
}

/// A [`CushionConfig`] prepared for evaluation (sanitised, light normalised).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shader {
    config: CushionConfig,
    light: [f64; 3],
}

impl Default for Shader {
    fn default() -> Self {
        Self::new(CushionConfig::default())
    }
}

impl Shader {
    /// Prepares `config` (sanitised first).
    pub fn new(config: CushionConfig) -> Self {
        let config = config.sanitized();
        let [lx, ly, lz] = config.light.map(f64::from);
        let len = lx.hypot(ly).hypot(lz);
        // `sanitized` guarantees a finite, non-zero light; the guard keeps the division honest anyway.
        let light = if len.is_finite() && len > 0.0 { [lx / len, ly / len, lz / len] } else { [0.0, 0.0, 1.0] };
        Self { config, light }
    }

    /// The sanitised configuration in use.
    pub fn config(&self) -> CushionConfig {
        self.config
    }

    /// The surface before any ridge: flat, the next ridge being the root ridge.
    pub fn base(&self) -> Surface {
        Surface { next_height: f64::from(self.config.height), ..Surface::default() }
    }

    /// Adds the ridge of a tile with rectangle `rect` and returns the new surface, whose next ridge is
    /// smaller by the scale factor. A rectangle that is empty or not finite adds no ridge (the level is
    /// still counted). Sums that would overflow are discarded the same way.
    pub fn add_ridge(&self, surface: Surface, rect: Rect) -> Surface {
        let h = surface.next_height;
        let next_height =
            if h * f64::from(self.config.scale) > MIN_RIDGE_HEIGHT { h * f64::from(self.config.scale) } else { 0.0 };
        let (x1, y1) = (f64::from(rect.x), f64::from(rect.y));
        let (w, hh) = (f64::from(rect.w), f64::from(rect.h));
        let usable = h > MIN_RIDGE_HEIGHT && x1.is_finite() && y1.is_finite() && w > 0.0 && hh > 0.0;
        if !usable || !w.is_finite() || !hh.is_finite() {
            return Surface { next_height, ..surface };
        }
        let (x2, y2) = (x1 + w, y1 + hh);
        let next = Surface {
            ax: surface.ax - 4.0 * h / w,
            bx: surface.bx + 4.0 * h * (x2 + x1) / w,
            ay: surface.ay - 4.0 * h / hh,
            by: surface.by + 4.0 * h * (y2 + y1) / hh,
            next_height,
        };
        let finite = next.ax.is_finite() && next.bx.is_finite() && next.ay.is_finite() && next.by.is_finite();
        if finite { next } else { Surface { next_height, ..surface } }
    }

    /// Surfaces for a list of tiles in parent-before-children order. `parents[i]` is the index of the
    /// parent of tile `i` (`None` for a root). A parent index that is not smaller than the tile's own
    /// index (which also rules out cycles) or missing counts as "no parent". Missing entries in
    /// `parents` count as roots. The result is parallel to `rects`.
    pub fn tile_surfaces(&self, parents: &[Option<usize>], rects: &[Rect]) -> Vec<Surface> {
        let mut out: Vec<Surface> = Vec::with_capacity(rects.len());
        for (i, rect) in rects.iter().enumerate() {
            let parent = parents.get(i).copied().flatten().filter(|&p| p < i);
            let base = parent.and_then(|p| out.get(p).copied()).unwrap_or_else(|| self.base());
            out.push(self.add_ridge(base, *rect));
        }
        out
    }

    /// Brightness in `[0, 1]` of `surface` at the point `(x, y)` (same coordinates as the rectangles the
    /// surface was built from). Non-finite input gives the brightness of a flat surface.
    pub fn brightness(&self, surface: &Surface, x: f32, y: f32) -> f32 {
        let (px, py) = (f64::from(x), f64::from(y));
        self.shade(2.0 * surface.ax * px + surface.bx, 2.0 * surface.ay * py + surface.by)
    }

    /// Brightness of a surface with slopes `dz/dx = dzdx` and `dz/dy = dzdy`.
    fn shade(&self, dzdx: f64, dzdy: f64) -> f32 {
        let [lx, ly, lz] = self.light;
        let (nx, ny) = (-dzdx, -dzdy);
        let len = (nx * nx + ny * ny + 1.0).sqrt();
        let cos = (nx * lx + ny * ly + lz) / len;
        let cos = if cos.is_finite() { cos } else { lz };
        let value = f64::from(self.config.ambient) + f64::from(self.config.diffuse) * cos.max(0.0);
        narrow_unit(value)
    }

    /// Calls `put(px, py, brightness)` for every pixel of a `clip.0 x clip.1` image whose centre
    /// `(px + 0.5, py + 0.5)` lies inside `rect` (half-open, like [`Rect::contains`]), shaded by
    /// `surface`. Because the test is the centre point, the leaf tiles of a layout that partitions its
    /// bounds cover every pixel exactly once. An empty or non-finite rectangle yields no pixels.
    pub fn fill_rect<F: FnMut(u32, u32, f32)>(&self, surface: &Surface, rect: Rect, clip: (u32, u32), mut put: F) {
        let (x, y, w, h) = (f64::from(rect.x), f64::from(rect.y), f64::from(rect.w), f64::from(rect.h));
        if !(x.is_finite() && y.is_finite() && w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            return;
        }
        let (px0, px1) = pixel_span(x, x + w, clip.0);
        let (py0, py1) = pixel_span(y, y + h, clip.1);
        if px0 >= px1 || py0 >= py1 {
            return;
        }
        for py in py0..py1 {
            let cy = f64::from(py) + 0.5;
            let dzdy = 2.0 * surface.ay * cy + surface.by;
            for px in px0..px1 {
                let cx = f64::from(px) + 0.5;
                put(px, py, self.shade(2.0 * surface.ax * cx + surface.bx, dzdy));
            }
        }
    }
}

/// Edges are snapped to this grid (a power of two, so the snap is exact) before the pixel centres are
/// tested. Neighbouring tiles compute their shared edge independently and can differ by a float ulp;
/// without the snap an edge that lies exactly on a pixel centre (`x.5`) would leave a one-pixel gap or
/// overlap. The pixel-centre threshold is the middle of a grid cell, so such rounding noise never
/// crosses it.
const EDGE_GRID: f64 = 256.0;

fn snap(v: f64) -> f64 {
    (v * EDGE_GRID).round() / EDGE_GRID
}

/// Pixel indices `lo..hi` (clamped to `0..limit`) whose centres `i + 0.5` lie in `[start, end)`.
fn pixel_span(start: f64, end: f64, limit: u32) -> (u32, u32) {
    let first = clamp_index((snap(start) - 0.5).ceil(), limit);
    let last = clamp_index((snap(end) - 0.5).ceil(), limit);
    (first, last.max(first))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_index(v: f64, limit: u32) -> u32 {
    if v.is_nan() {
        return 0;
    }
    // The value is clamped into `0..=limit` first, so the conversion is exact.
    v.clamp(0.0, f64::from(limit)) as u32
}

#[allow(clippy::cast_possible_truncation)]
fn narrow_unit(v: f64) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) as f32 } else { 0.0 }
}

#[cfg(test)]
mod tests;
