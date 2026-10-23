//! Cushion-shaded treemap image (G-018): turns the laid-out tiles into one texture.
//!
//! The picture is rendered by [`render`] on a background thread (one job at a time) and uploaded as a
//! single texture, so the UI thread only copies the tile list when a job starts and paints one image
//! per frame. The result is cached by the map key (tree revision, layout generation, size, size mode),
//! the theme and the pixel scale; nothing is recomputed while that key stays the same. The shading
//! itself lives in `dirustat_treemap::cushion`.

use crate::theme::{Rgba, ThemeKind, Tokens};
use dirustat_treemap::{CushionConfig, Rect, Shader};
use egui::{Color32, ColorImage, TextureHandle, TextureId, TextureOptions};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Most pixels one cushion image may have (64 MB of RGBA). Larger panels are rendered at a lower
/// resolution and stretched, so a huge window cannot exhaust memory.
const MAX_PIXELS: f32 = 16_000_000.0;
/// Pixel scale (physical pixels per point) used for the image, clamped to this range.
const SCALE_RANGE: (f32, f32) = (0.5, 4.0);

/// What the image depends on. Equal keys give the same picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CushionKey {
    pub revision: u64,
    pub generation: u64,
    pub bounds: [u32; 2],
    pub allocated: bool,
    pub theme: ThemeKind,
    pub scale_bits: u32,
}

impl CushionKey {
    /// An image of `self` may stand in (stretched) for `other` while the right one is rendered: same
    /// tree, size mode, theme and scale, only the layout generation or panel size differs.
    fn can_stand_in_for(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.allocated == other.allocated
            && self.theme == other.theme
            && self.scale_bits == other.scale_bits
    }
}

/// One tile as the render job needs it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CushionTile {
    pub rect: Rect,
    pub color: [u8; 3],
    /// Arena index of the tile's node.
    pub node: usize,
    /// Arena index of the parent node (`None` for the root).
    pub parent: Option<usize>,
    pub leaf: bool,
    /// Stands for the small children of `node`; its surface hangs off the tile of `node` itself.
    pub remainder: bool,
}

/// Everything [`render`] needs; owned, so it can move to a background thread.
#[derive(Clone, Debug)]
pub struct CushionInput {
    /// Image size in pixels.
    pub size: [usize; 2],
    /// Pixels per layout unit (tile rectangles are in layout units).
    pub scale: f32,
    pub background: [u8; 3],
    pub tiles: Vec<CushionTile>,
    pub config: CushionConfig,
}

/// Image size and pixel scale for a panel of `bounds_w x bounds_h` points at `pixels_per_point`.
/// `None` for an empty or non-finite panel. The scale is reduced when the image would exceed
/// [`MAX_PIXELS`].
pub fn image_geometry(bounds_w: f32, bounds_h: f32, pixels_per_point: f32) -> Option<([usize; 2], f32)> {
    if !(bounds_w.is_finite() && bounds_h.is_finite() && bounds_w >= 1.0 && bounds_h >= 1.0) {
        return None;
    }
    let mut scale = if pixels_per_point.is_finite() { pixels_per_point } else { 1.0 };
    scale = scale.clamp(SCALE_RANGE.0, SCALE_RANGE.1);
    let area = bounds_w * bounds_h * scale * scale;
    if area > MAX_PIXELS {
        scale *= (MAX_PIXELS / area).sqrt();
    }
    let side = |points: f32| to_len((points * scale).round().max(1.0));
    Some(([side(bounds_w), side(bounds_h)], scale))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_len(v: f32) -> usize {
    // Callers pass a finite value in 1..=MAX_PIXELS-ish; the clamp keeps the conversion exact.
    v.clamp(1.0, 16_777_216.0) as usize
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn channel(base: u8, brightness: f32) -> u8 {
    (f32::from(base) * brightness).round().clamp(0.0, 255.0) as u8
}

fn scaled(r: Rect, scale: f32) -> Rect {
    Rect { x: r.x * scale, y: r.y * scale, w: r.w * scale, h: r.h * scale }
}

/// Renders the picture: leaf tiles (and remainder tiles) filled with their colour times the cushion
/// brightness, everything else in the background colour. Pure and deterministic.
pub fn render(input: &CushionInput) -> ColorImage {
    let [w, h] = input.size;
    let bg = input.background;
    let mut image = ColorImage::filled([w, h], Color32::from_rgb(bg[0], bg[1], bg[2]));
    let shader = Shader::new(input.config);
    // Parent links: a tile hangs off the tile of its parent node; a remainder tile off the tile of its
    // own node (it has its parent's id).
    let mut tile_of_node: HashMap<usize, usize> = HashMap::with_capacity(input.tiles.len());
    for (i, t) in input.tiles.iter().enumerate().filter(|(_, t)| !t.remainder) {
        tile_of_node.entry(t.node).or_insert(i);
    }
    let parents: Vec<Option<usize>> = input
        .tiles
        .iter()
        .map(|t| {
            let key = if t.remainder { Some(t.node) } else { t.parent };
            key.and_then(|n| tile_of_node.get(&n).copied())
        })
        .collect();
    let rects: Vec<Rect> = input.tiles.iter().map(|t| scaled(t.rect, input.scale)).collect();
    let surfaces = shader.tile_surfaces(&parents, &rects);
    let (iw, ih) = (u32::try_from(w).unwrap_or(u32::MAX), u32::try_from(h).unwrap_or(u32::MAX));
    for ((tile, rect), surface) in input.tiles.iter().zip(&rects).zip(&surfaces) {
        if !tile.leaf {
            continue;
        }
        shader.fill_rect(surface, *rect, (iw, ih), |px, py, brightness| {
            let (col, row) = (usize::try_from(px).unwrap_or(usize::MAX), usize::try_from(py).unwrap_or(usize::MAX));
            let at = row.saturating_mul(w).saturating_add(col);
            if let Some(pixel) = image.pixels.get_mut(at) {
                let [r, g, b] = tile.color;
                *pixel = Color32::from_rgb(channel(r, brightness), channel(g, brightness), channel(b, brightness));
            }
        });
    }
    image
}

/// Background-thread state shared with the UI thread.
#[derive(Default)]
struct Slot {
    /// The uploaded texture and the key it was rendered for.
    texture: Option<(CushionKey, TextureHandle)>,
    /// A finished image waiting for upload.
    result: Option<(CushionKey, ColorImage)>,
    /// A job is outstanding.
    running: Option<CushionKey>,
    /// The last key whose job failed (panic or no thread): not retried until the key changes.
    failed: Option<CushionKey>,
    /// Jobs started so far (tests watch it to prove the cache holds).
    started: u64,
}

type Shared = Arc<Mutex<Slot>>;

fn lock(slot: &Shared) -> MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

fn slot_id() -> egui::Id {
    egui::Id::new("dirustat_cushion_slot")
}

fn shared(ctx: &egui::Context) -> Shared {
    if let Some(s) = ctx.data(|d| d.get_temp::<Shared>(slot_id())) {
        return s;
    }
    let s: Shared = Arc::default();
    ctx.data_mut(|d| d.insert_temp(slot_id(), Arc::clone(&s)));
    s
}

/// Returns the texture to paint for `key` (possibly an older picture of the same tree while the right
/// one renders), or `None` while there is nothing to show yet. When the picture for `key` is missing
/// and `may_start` is set, a background job is started; `build` copies the tiles and is only called
/// then (`None` from it marks the key as failed so it is not retried every frame). Never blocks.
pub fn texture_for(
    ctx: &egui::Context,
    key: &CushionKey,
    may_start: bool,
    build: impl FnOnce() -> Option<CushionInput>,
) -> Option<TextureId> {
    let slot = shared(ctx);
    let mut guard = lock(&slot);
    if let Some((k, image)) = guard.result.take()
        && k.can_stand_in_for(key)
    {
        let handle = ctx.load_texture("dirustat_treemap_cushion", image, TextureOptions::LINEAR);
        guard.texture = Some((k, handle));
    }
    let current = guard.texture.as_ref().is_some_and(|(k, _)| k == key);
    if may_start && !current && guard.running.is_none() && guard.failed != Some(*key) {
        match build() {
            Some(input) => start_job(ctx, &slot, &mut guard, *key, input),
            None => guard.failed = Some(*key),
        }
    }
    guard.texture.as_ref().filter(|(k, _)| k.can_stand_in_for(key)).map(|(_, t)| t.id())
}

fn start_job(ctx: &egui::Context, slot: &Shared, guard: &mut Slot, key: CushionKey, input: CushionInput) {
    guard.running = Some(key);
    guard.started = guard.started.saturating_add(1);
    let (job_slot, job_ctx) = (Arc::clone(slot), ctx.clone());
    let spawned = std::thread::Builder::new().name("dirustat-cushion".to_owned()).spawn(move || {
        let image = catch_unwind(AssertUnwindSafe(|| render(&input))).ok();
        let mut g = lock(&job_slot);
        g.running = None;
        match image {
            Some(image) => g.result = Some((key, image)),
            None => g.failed = Some(key),
        }
        drop(g);
        job_ctx.request_repaint();
    });
    if spawned.is_err() {
        guard.running = None;
        guard.failed = Some(key);
    }
}

/// Drops the texture (the flat painter is in use, so the memory goes back).
pub fn release(ctx: &egui::Context) {
    let slot = shared(ctx);
    let mut guard = lock(&slot);
    guard.texture = None;
    guard.result = None;
}

/// Whether a render job is outstanding (the caller keeps polling while it is).
pub fn is_rendering(ctx: &egui::Context) -> bool {
    lock(&shared(ctx)).running.is_some()
}

/// Number of render jobs started on this context.
#[cfg(test)]
pub fn jobs_started(ctx: &egui::Context) -> u64 {
    lock(&shared(ctx)).started
}

/// Background colour of the image for `tokens`.
pub fn background_rgb(tokens: &Tokens) -> [u8; 3] {
    let [r, g, b, _]: Rgba = tokens.background;
    [r, g, b]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(rect: Rect, node: usize, parent: Option<usize>, leaf: bool) -> CushionTile {
        CushionTile { rect, color: [200, 100, 50], node, parent, leaf, remainder: false }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn input() -> CushionInput {
        CushionInput {
            size: [100, 60],
            scale: 1.0,
            background: [1, 2, 3],
            config: CushionConfig::default(),
            tiles: vec![
                tile(rect(0.0, 0.0, 100.0, 60.0), 0, None, false),
                tile(rect(0.0, 0.0, 60.0, 60.0), 1, Some(0), true),
                tile(rect(60.0, 0.0, 40.0, 30.0), 2, Some(0), true),
                // 40x30 at the bottom right is deliberately missing: it must stay background.
            ],
        }
    }

    #[test]
    fn render_shades_leaves_and_leaves_the_rest_in_the_background() {
        let image = render(&input());
        assert_eq!(image.size, [100, 60]);
        let at = |x: usize, y: usize| image.pixels.get(y * 100 + x).copied().unwrap_or(Color32::TRANSPARENT);
        assert_eq!(at(80, 45), Color32::from_rgb(1, 2, 3), "no leaf covers it");
        // Brightness is at most 1, so no channel exceeds the base colour; shading makes pixels differ.
        let leaf: Vec<Color32> = (0..60).flat_map(|y| (0..60).map(move |x| (x, y))).map(|(x, y)| at(x, y)).collect();
        assert!(leaf.iter().all(|c| c.r() <= 200 && c.g() <= 100 && c.b() <= 50 && c.a() == 255));
        let distinct: std::collections::HashSet<[u8; 3]> = leaf.iter().map(|c| [c.r(), c.g(), c.b()]).collect();
        assert!(distinct.len() > 10, "a cushion has a gradient, got {} colours", distinct.len());
        // The lit upper-left rim is brighter than the shadowed lower-right rim of the big tile.
        assert!(at(3, 30).r() > at(56, 30).r());
        // Deterministic.
        assert_eq!(render(&input()).pixels, image.pixels);
    }

    #[test]
    fn a_flat_config_gives_flat_colours() {
        let mut i = input();
        i.config = CushionConfig { height: 0.0, ..CushionConfig::default() };
        let image = render(&i);
        let distinct: std::collections::HashSet<Color32> = image.pixels.iter().copied().collect();
        assert_eq!(distinct.len(), 2, "background plus one flat leaf colour, got {distinct:?}");
    }

    #[test]
    fn hostile_tiles_do_not_panic_or_paint_outside() {
        let mut i = input();
        let nan = f32::NAN;
        i.tiles.push(tile(rect(nan, 0.0, 10.0, 10.0), 3, Some(0), true));
        i.tiles.push(tile(rect(-50.0, -50.0, 1e30, 1e30), 4, Some(99), true));
        i.tiles.push(tile(rect(0.0, 0.0, f32::INFINITY, 5.0), 5, Some(5), true));
        i.tiles.push(CushionTile { remainder: true, ..tile(rect(10.0, 10.0, 0.0, 0.0), 77, None, true) });
        i.size = [100, 60];
        let image = render(&i);
        assert_eq!(image.pixels.len(), 6000);
        let mut empty = input();
        empty.tiles.clear();
        assert!(render(&empty).pixels.iter().all(|&c| c == Color32::from_rgb(1, 2, 3)));
        empty.size = [0, 0];
        assert!(render(&empty).pixels.is_empty());
    }

    #[test]
    fn remainder_tiles_hang_off_their_own_node() {
        let mut i = input();
        i.tiles = vec![
            tile(rect(0.0, 0.0, 100.0, 60.0), 0, None, false),
            CushionTile { remainder: true, ..tile(rect(0.0, 0.0, 100.0, 60.0), 0, Some(9), true) },
        ];
        let image = render(&i);
        assert!(image.pixels.iter().any(|&c| c != Color32::from_rgb(1, 2, 3)));
        // Same result as a normal leaf child of the root: the remainder has the root's ridge as parent.
        let mut j = input();
        j.tiles = vec![
            tile(rect(0.0, 0.0, 100.0, 60.0), 0, None, false),
            tile(rect(0.0, 0.0, 100.0, 60.0), 1, Some(0), true),
        ];
        assert_eq!(render(&j).pixels, image.pixels);
    }

    #[test]
    fn image_geometry_scales_caps_and_rejects_garbage() {
        assert_eq!(image_geometry(800.0, 600.0, 1.0), Some(([800, 600], 1.0)));
        assert_eq!(image_geometry(800.0, 600.0, 2.0), Some(([1600, 1200], 2.0)));
        assert_eq!(image_geometry(800.0, 600.0, f32::NAN), Some(([800, 600], 1.0)));
        assert_eq!(image_geometry(800.0, 600.0, 100.0), Some(([3200, 2400], 4.0)));
        let (size, scale) = image_geometry(8000.0, 6000.0, 2.0).unwrap_or(([0, 0], 0.0));
        assert!(size[0] * size[1] <= 16_100_000 && scale < 2.0, "{size:?} {scale}");
        for bad in [(0.0, 10.0), (10.0, 0.5), (f32::NAN, 10.0), (f32::INFINITY, 10.0), (-5.0, 10.0)] {
            assert_eq!(image_geometry(bad.0, bad.1, 1.0), None, "{bad:?}");
        }
    }

    fn key(revision: u64, generation: u64) -> CushionKey {
        CushionKey {
            revision,
            generation,
            bounds: [100, 60],
            allocated: true,
            theme: ThemeKind::Dark,
            scale_bits: 1.0_f32.to_bits(),
        }
    }

    fn wait_rendered(ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while is_rendering(ctx) {
            assert!(std::time::Instant::now() < deadline, "render job never finished");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn texture_is_rendered_off_thread_cached_and_stands_in_until_replaced() {
        let ctx = egui::Context::default();
        let k1 = key(1, 1);
        let builds = std::cell::Cell::new(0);
        let ask = |k: &CushionKey| {
            texture_for(&ctx, k, true, || {
                builds.set(builds.get() + 1);
                Some(input())
            })
        };
        assert_eq!(ask(&k1), None, "nothing to show before the first job finishes");
        wait_rendered(&ctx);
        let first = ask(&k1);
        assert!(first.is_some(), "uploaded after the job");
        assert_eq!(ask(&k1), first);
        assert_eq!(jobs_started(&ctx), 1, "same key, no new job");
        // A new layout generation of the same tree keeps the old picture on screen while it renders.
        let k2 = key(1, 2);
        assert_eq!(ask(&k2), first);
        wait_rendered(&ctx);
        let second = ask(&k2);
        assert!(second.is_some());
        assert_eq!(jobs_started(&ctx), 2);
        // Another tree revision has no stand-in.
        assert_eq!(ask(&key(2, 3)), None);
        assert_eq!(builds.get(), 3);
        wait_rendered(&ctx);
        release(&ctx);
        assert_eq!(ask(&key(2, 3)), None, "released: rendered again");
        assert_eq!(jobs_started(&ctx), 4);
    }

    #[test]
    fn no_job_starts_while_the_layout_is_outstanding() {
        let ctx = egui::Context::default();
        let k = key(1, 1);
        assert_eq!(texture_for(&ctx, &k, false, || Some(input())), None);
        assert_eq!(jobs_started(&ctx), 0);
        assert_eq!(texture_for(&ctx, &k, true, || Some(input())), None);
        assert_eq!(jobs_started(&ctx), 1);
    }

    #[test]
    fn a_failed_build_is_not_retried_every_frame() {
        let ctx = egui::Context::default();
        let k = key(1, 1);
        let mut calls = 0;
        for _ in 0..5 {
            assert_eq!(
                texture_for(&ctx, &k, true, || {
                    calls += 1;
                    None
                }),
                None
            );
        }
        assert_eq!(calls, 1);
        assert_eq!(jobs_started(&ctx), 0);
    }
}
