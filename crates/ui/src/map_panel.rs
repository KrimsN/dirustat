//! The treemap panel: paints `Session::map_layout`, hover highlight, tooltip and click selection.
//!
//! The layout lives in panel-local coordinates (origin at the panel's top-left, whole-pixel size),
//! so it is cached by the session and only recomputed on a tree, size or panel-size change.

use crate::cushion_view;
use crate::state::UiState;
use crate::theme::{Rgba, ThemeKind, Tokens};
use dirustat_engine::{MapTile, Session, extension_of};
use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use dirustat_treemap::Rect;
use egui::{Color32, Pos2, Sense, Stroke, StrokeKind};
use std::sync::Arc;

/// Tiles thinner than this (in points) are not painted.
const MIN_TILE_PX: f32 = 1.0;
/// Directory frames and leaf outlines are drawn only for tiles at least this large.
const MIN_OUTLINE_PX: f32 = 5.0;
/// Distance between the diagonal stripes of the free-space band.
const FREE_STRIPE_PX: f32 = 10.0;
/// The free-space band gets a text label when it is at least this wide.
const FREE_LABEL_MIN_PX: f32 = 80.0;
/// Width of each of the two rings drawn around a search hit.
const HIT_RING_PX: f32 = 2.0;
/// Tiles smaller than this get a single ring.
const HIT_RING_MIN_PX: f32 = 10.0;

/// Panel-local layout bounds for a panel of `size` points: whole pixels, never negative or NaN.
pub fn bounds_for_size(width: f32, height: f32) -> Rect {
    let snap = |v: f32| if v.is_finite() && v > 0.0 { v.floor() } else { 0.0 };
    Rect { x: 0.0, y: 0.0, w: snap(width), h: snap(height) }
}

/// Maps a pointer position in screen space into layout bounds (panel-local) space.
/// Returns `None` when the pointer is outside the panel rectangle or non-finite.
pub fn pointer_to_bounds(panel_min: (f32, f32), panel_size: (f32, f32), pointer: (f32, f32)) -> Option<(f32, f32)> {
    let (x, y) = (pointer.0 - panel_min.0, pointer.1 - panel_min.1);
    let inside = x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < panel_size.0 && y < panel_size.1;
    inside.then_some((x, y))
}

/// Colour of a tile body as RGBA bytes. Files and links use the extension colour, directories
/// (collapsed or empty) the surface token. Hovered tiles are lightened.
pub fn tile_color(kind: NodeKind, name: &str, tokens: &Tokens, hovered: bool) -> Rgba {
    let base = match kind {
        NodeKind::Dir => tokens.surface,
        NodeKind::File | NodeKind::Link => {
            let [r, g, b] = crate::ext_colors::ext_rgb(&extension_of(name));
            [r, g, b, 255]
        }
    };
    if hovered { lighten(base) } else { base }
}

fn lighten(c: Rgba) -> Rgba {
    let up = |v: u8| v.saturating_add((255 - v) / 3 + 12);
    [up(c[0]), up(c[1]), up(c[2]), c[3]]
}

/// Screen rectangle of a layout tile given the panel origin.
fn to_screen(origin: Pos2, r: Rect) -> egui::Rect {
    egui::Rect::from_min_size(Pos2::new(origin.x + r.x, origin.y + r.y), egui::vec2(r.w, r.h))
}

/// Path of a node for the tooltip: the same join as `tree.path` / `Session::node_path` (platform
/// separator, normalized root), computed on the tree the map snapshot was built from. Empty for an
/// unknown node or a broken parent chain.
pub fn node_path(tree: &Tree, node: NodeId) -> String {
    dirustat_engine::tree_node_path(tree, node).unwrap_or_default()
}

/// Per-tile colours, cached across frames in egui's temp memory.
#[derive(Clone)]
struct ColorCache {
    key: MapKey,
    colors: Arc<Vec<Rgba>>,
}

/// What the cached per-tile data depends on. The theme is part of it: tile colours come from the
/// theme's tokens, so a theme switch must not keep showing the old ones (G-066).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MapKey {
    revision: u64,
    generation: u64,
    bounds: [u32; 2],
    allocated: bool,
    theme: ThemeKind,
}

fn cache_key(session: &Session, bounds: Rect, theme: ThemeKind) -> MapKey {
    MapKey {
        revision: session.revision(),
        generation: session.map_generation(),
        bounds: [bounds.w.to_bits(), bounds.h.to_bits()],
        allocated: session.map_size_mode() == SizeMode::Allocated,
        theme,
    }
}

/// The cushion texture for the current layout (see [`cushion_view`]); `None` while there is none yet.
/// `may_start` is false while a layout is outstanding: the tiles are about to change, so no job is
/// started for them.
fn cushion_texture(
    ctx: &egui::Context,
    tokens: &Tokens,
    (key, bounds): (MapKey, Rect),
    tiles: &[MapTile],
    tree: &Tree,
    colors: &[Rgba],
    (treemap, may_start): (&dirustat_engine::TreemapSettings, bool),
) -> Option<egui::TextureId> {
    let max_side = ctx.input(|i| i.max_texture_side);
    let (size, scale) = cushion_view::image_geometry(bounds.w, bounds.h, ctx.pixels_per_point(), max_side)?;
    let config = treemap.look.config();
    let cushion_key = cushion_view::CushionKey {
        revision: key.revision,
        generation: key.generation,
        bounds: key.bounds,
        allocated: key.allocated,
        theme: tokens.kind,
        scale_bits: scale.to_bits(),
        look_bits: cushion_view::look_bits(&config),
        grid: treemap.grid,
    };
    cushion_view::texture_for(ctx, &cushion_key, may_start && !tiles.is_empty(), || {
        let tiles = tiles
            .iter()
            .zip(colors)
            .map(|(t, c)| cushion_view::CushionTile {
                rect: t.rect,
                color: [c[0], c[1], c[2]],
                node: t.node.index(),
                parent: tree.get(t.node).and_then(|n| n.parent()).map(NodeId::index),
                leaf: t.leaf,
                remainder: t.remainder,
            })
            .collect();
        Some(cushion_view::CushionInput {
            size,
            scale,
            background: cushion_view::background_rgb(tokens),
            tiles,
            config,
            grid: treemap.grid,
        })
    })
}

/// Draws the panel. Clicking selects the tile under the pointer through the `select.node` command.
pub fn show(ui: &mut egui::Ui, session: &mut Session, state: &mut UiState) {
    let tokens = Tokens::for_kind(state.theme);
    let avail = ui.available_size();
    let full = bounds_for_size(avail.x, avail.y);
    if full.w < MIN_TILE_PX || full.h < MIN_TILE_PX {
        return;
    }
    let (response, painter) = ui.allocate_painter(egui::vec2(full.w, full.h), Sense::click());
    let origin = response.rect.min;
    // With the free-space band on, the tree gets the left part of the panel; everything below (layout,
    // pointer mapping, the cushion image) works on that part, the band is painted separately.
    let split = session.free_space_split(full);
    let bounds = split.tree;
    if split.pending {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    // The layout is computed on a background thread; until it arrives the previous one is shown.
    let layout_pending = session.map_poll(bounds);
    let key = cache_key(session, bounds, state.theme);
    let cache_id = ui.id().with("map_colors");
    let cached: Option<ColorCache> = ui.data(|d| d.get_temp::<ColorCache>(cache_id));
    let selected = session.selection();
    let mode = session.map_size_mode();
    let treemap_settings = session.settings().treemap;

    let pointer =
        response.hover_pos().and_then(|p| pointer_to_bounds((origin.x, origin.y), (bounds.w, bounds.h), (p.x, p.y)));
    let click = if response.clicked() {
        response
            .interact_pointer_pos()
            .and_then(|p| pointer_to_bounds((origin.x, origin.y), (bounds.w, bounds.h), (p.x, p.y)))
    } else {
        None
    };

    let (tree, layout) = session.map_ready_view();
    let tiles = layout.tiles();
    let hovered_tile: Option<MapTile> = pointer.and_then(|(x, y)| layout.tile_at(x, y)).copied();
    let hovered_node = hovered_tile.map(|t| t.node);

    let colors = match cached {
        Some(c) if c.key == key && c.colors.len() == tiles.len() => c.colors,
        _ => {
            let colors: Arc<Vec<Rgba>> = Arc::new(
                tiles
                    .iter()
                    .map(|t| match tree.get(t.node) {
                        Some(n) => tile_color(n.kind(), n.name(), &tokens, false),
                        None => tokens.surface,
                    })
                    .collect(),
            );
            ui.data_mut(|d| d.insert_temp(cache_id, ColorCache { key, colors: Arc::clone(&colors) }));
            colors
        }
    };

    // Leaves whose extension is selected in the extension panel (cached per revision/layout/extension).
    let ext_hits: Option<Arc<Vec<bool>>> = state.selected_ext.as_deref().map(|want| {
        let id = ui.id().with("map_ext_hits");
        let hit_key = (key, want.to_owned());
        if let Some((k, v)) = ui.data(|d| d.get_temp::<((MapKey, String), Arc<Vec<bool>>)>(id))
            && k == hit_key
        {
            return v;
        }
        let v: Arc<Vec<bool>> = Arc::new(
            tiles
                .iter()
                .map(|t| t.leaf && tree.get(t.node).is_some_and(|n| dirustat_engine::extension_of(n.name()) == want))
                .collect(),
        );
        ui.data_mut(|d| d.insert_temp(id, (hit_key, Arc::clone(&v))));
        v
    });

    painter.rect_filled(response.rect, 0.0, Tokens::color(tokens.background));
    // Cushion shading is one cached texture rendered off the UI thread; flat tiles are drawn until the
    // first picture arrives and whenever the setting is off.
    let cushion = if treemap_settings.cushion {
        cushion_texture(ui.ctx(), &tokens, (key, bounds), tiles, tree, &colors, (&treemap_settings, !layout_pending))
    } else {
        cushion_view::release(ui.ctx());
        None
    };
    if let Some(texture) = cushion {
        let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture, to_screen(origin, bounds), uv, Color32::WHITE);
    }
    let frame_stroke = Stroke::new(1.0, Tokens::color(tokens.border));
    // With the grid on, the flat tiles get a clearly visible outline instead of the faint default.
    let leaf_stroke = Stroke::new(1.0, Color32::from_black_alpha(if treemap_settings.grid { 170 } else { 70 }));
    let mut selected_rect = None;
    let mut hovered_rect = None;
    for (tile, color) in tiles.iter().zip(colors.iter()) {
        let r = tile.rect;
        // A remainder tile carries its parent's id but is only a sliver of undrawable children; the
        // outline belongs to the folder's own tile (G-072, QA-250).
        if Some(tile.node) == selected && !tile.remainder {
            selected_rect = Some(to_screen(origin, r));
        }
        if Some(tile.node) == hovered_node && !tile.remainder {
            hovered_rect = Some((to_screen(origin, r), tile.leaf));
        }
        if r.w < MIN_TILE_PX || r.h < MIN_TILE_PX {
            continue;
        }
        if cushion.is_some() {
            // The texture carries the fills and the hierarchy (the cushion ridges show directory
            // nesting), so there are deliberately no leaf outlines or directory frames in this mode.
            continue;
        }
        let screen = to_screen(origin, r);
        if tile.leaf {
            painter.rect_filled(screen, 0.0, Tokens::color(*color));
            if r.w >= MIN_OUTLINE_PX && r.h >= MIN_OUTLINE_PX {
                painter.rect_stroke(screen, 0.0, leaf_stroke, StrokeKind::Inside);
            }
        } else if r.w >= MIN_OUTLINE_PX && r.h >= MIN_OUTLINE_PX {
            painter.rect_stroke(screen, 0.0, frame_stroke, StrokeKind::Inside);
        }
    }
    if let Some(hits) = ext_hits.as_ref().filter(|h| h.len() == tiles.len()) {
        let accent = Tokens::color(tokens.accent);
        for (tile, _) in tiles.iter().zip(hits.iter()).filter(|(_, hit)| **hit) {
            if tile.rect.w >= MIN_TILE_PX && tile.rect.h >= MIN_TILE_PX {
                painter.rect_stroke(to_screen(origin, tile.rect), 0.0, Stroke::new(1.0, accent), StrokeKind::Inside);
            }
        }
    }
    if !state.search.hit_set().is_empty() {
        // Two rings in the theme's extreme colours, ordered by contrast with the tile, so a hit
        // stands out on every tile colour (QA-131).
        for (tile, color) in
            tiles.iter().zip(colors.iter()).filter(|(t, _)| state.search.hit_set().contains(&t.node.index()))
        {
            if tile.rect.w < MIN_TILE_PX || tile.rect.h < MIN_TILE_PX {
                continue;
            }
            let (inner, outer) = crate::contrast::outline_pair(&tokens, *color);
            let screen = to_screen(origin, tile.rect);
            if tile.rect.w >= HIT_RING_MIN_PX && tile.rect.h >= HIT_RING_MIN_PX {
                painter.rect_stroke(screen, 0.0, Stroke::new(HIT_RING_PX, inner), StrokeKind::Inside);
                painter.rect_stroke(
                    screen.shrink(HIT_RING_PX),
                    0.0,
                    Stroke::new(HIT_RING_PX, outer),
                    StrokeKind::Inside,
                );
            } else {
                painter.rect_stroke(screen, 0.0, Stroke::new(HIT_RING_PX, inner), StrokeKind::Inside);
            }
        }
    }
    if let Some((screen, leaf)) = hovered_rect {
        let fill = Tokens::color(tokens.text);
        if leaf {
            painter.rect_filled(screen, 0.0, fill.gamma_multiply(0.25));
        }
        painter.rect_stroke(screen, 0.0, Stroke::new(1.0, fill), StrokeKind::Inside);
    }
    if let Some(screen) = selected_rect {
        painter.rect_stroke(screen, 0.0, Stroke::new(2.0, Tokens::color(tokens.accent)), StrokeKind::Inside);
    }
    if let Some((band, numbers)) = split.band {
        paint_free_band(&painter, &tokens, to_screen(origin, band), numbers);
        let over_band = response.hover_pos().is_some_and(|p| to_screen(origin, band).contains(p));
        if over_band {
            response.clone().on_hover_ui_at_pointer(|ui| {
                ui.strong("Free space");
                ui.label(format!(
                    "{} free of {} on the volume",
                    crate::app::human_size(numbers.free),
                    crate::app::human_size(numbers.total)
                ));
                ui.weak(format!("Scanned: {}", crate::app::human_size(numbers.used)));
            });
        }
    }
    if layout_pending {
        painter.text(
            response.rect.left_top() + egui::vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            "Laying out...",
            egui::FontId::proportional(12.0),
            Tokens::color(tokens.text_muted),
        );
        // Poll again soon; the layout thread does not wake the UI by itself.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
    }

    let tooltip = hovered_tile.and_then(|t| {
        let n = tree.get(t.node)?;
        Some((n.name().to_owned(), n.total_size(mode), node_path(tree, t.node)))
    });
    if let Some((name, size, path)) = tooltip {
        response.clone().on_hover_ui_at_pointer(|ui| {
            ui.strong(name);
            ui.label(crate::app::human_size(size));
            ui.weak(path);
        });
    }
    if let Some((x, y)) = click {
        // Selection goes through the command registry; a click on empty space clears it.
        match session.tile_at(x, y) {
            Some(node) => {
                let _ = session.execute("select.node", &serde_json::json!({ "id": node.index() }));
            }
            None => {
                let _ = session.execute("select.clear", &serde_json::json!({}));
            }
        }
    }
    context_menu(ui, &response, session, state, (origin, bounds));
}

/// Paints the free-space band: a muted panel colour with diagonal stripes (so it cannot be mistaken
/// for a file tile) and, when it is wide enough, a label.
fn paint_free_band(painter: &egui::Painter, tokens: &Tokens, band: egui::Rect, numbers: dirustat_engine::FreeSpace) {
    let clip = painter.with_clip_rect(band);
    clip.rect_filled(band, 0.0, Tokens::color(tokens.panel));
    let stripe = Stroke::new(1.0, Tokens::color(tokens.border));
    let mut x = band.left() - band.height();
    while x < band.right() {
        clip.line_segment([Pos2::new(x, band.bottom()), Pos2::new(x + band.height(), band.top())], stripe);
        x += FREE_STRIPE_PX;
    }
    clip.rect_stroke(band, 0.0, Stroke::new(1.0, Tokens::color(tokens.border)), StrokeKind::Inside);
    if band.width() >= FREE_LABEL_MIN_PX {
        let font = egui::FontId::proportional(12.0);
        let color = Tokens::color(tokens.text);
        let backdrop = Tokens::color(tokens.panel);
        let text = format!(
            "Free space
{}",
            crate::app::human_size(numbers.free)
        );
        let galley = painter.layout_no_wrap(text, font, color);
        let at = band.center() - galley.size() / 2.0;
        clip.rect_filled(egui::Rect::from_min_size(at, galley.size()).expand(3.0), 2.0, backdrop);
        clip.galley(at, galley, color);
    }
}

/// Right click: selects the tile under the pointer and offers the node context menu for it.
fn context_menu(
    ui: &mut egui::Ui,
    response: &egui::Response,
    session: &mut Session,
    state: &mut UiState,
    (origin, bounds): (Pos2, Rect),
) {
    let slot = ui.id().with("map_context_node");
    if response.secondary_clicked() {
        let node = response
            .interact_pointer_pos()
            .and_then(|p| pointer_to_bounds((origin.x, origin.y), (bounds.w, bounds.h), (p.x, p.y)))
            .and_then(|(x, y)| session.tile_at(x, y));
        ui.data_mut(|d| d.insert_temp(slot, node));
        if let Some(n) = node {
            let _ = session.execute("select.node", &serde_json::json!({ "id": n.index() }));
        }
    }
    let mut chosen = None;
    let block = crate::delete_dialog::block_reason(session);
    let root = session.tree().root();
    response.context_menu(|ui| match ui.data(|d| d.get_temp::<Option<NodeId>>(slot)).flatten() {
        Some(node) => {
            let block = block.or_else(|| (node == root).then_some(crate::delete_dialog::ROOT_REASON));
            chosen = crate::context_menu::menu_ui(ui, block).map(|a| (node, a));
        }
        None => {
            ui.weak("No item here");
        }
    });
    if let Some((node, action)) = chosen {
        crate::context_menu::perform(ui.ctx(), session, state, node, action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::NodeKind;

    #[test]
    fn bounds_snap_to_whole_pixels_and_reject_garbage() {
        let b = bounds_for_size(800.7, 600.2);
        assert_eq!((b.x, b.y, b.w, b.h), (0.0, 0.0, 800.0, 600.0));
        for bad in [f32::NAN, f32::INFINITY, -4.0] {
            let b = bounds_for_size(bad, 10.0);
            assert_eq!(b.w, 0.0);
        }
    }

    #[test]
    fn pointer_mapping_is_relative_and_half_open() {
        assert_eq!(pointer_to_bounds((100.0, 50.0), (200.0, 100.0), (110.0, 60.0)), Some((10.0, 10.0)));
        assert_eq!(pointer_to_bounds((100.0, 50.0), (200.0, 100.0), (100.0, 50.0)), Some((0.0, 0.0)));
        assert_eq!(pointer_to_bounds((100.0, 50.0), (200.0, 100.0), (300.0, 60.0)), None);
        assert_eq!(pointer_to_bounds((100.0, 50.0), (200.0, 100.0), (99.0, 60.0)), None);
        assert_eq!(pointer_to_bounds((0.0, 0.0), (10.0, 10.0), (f32::NAN, 1.0)), None);
    }

    #[test]
    fn tile_colors_by_extension_dir_and_hover() {
        let t = Tokens::dark();
        let a = tile_color(NodeKind::File, "A.TXT", &t, false);
        assert_eq!(a, tile_color(NodeKind::File, "b.txt", &t, false), "same extension, same colour");
        assert_eq!(tile_color(NodeKind::Dir, "x.txt", &t, false), t.surface);
        let hot = tile_color(NodeKind::File, "a.txt", &t, true);
        assert!(hot[0] >= a[0] && hot[1] >= a[1] && hot[2] >= a[2] && hot != a);
        assert_eq!(tile_color(NodeKind::File, "Makefile", &t, false)[..3], dirustat_engine::NO_EXT_COLOR);
    }

    #[test]
    fn node_path_joins_names_from_root() {
        let mut tree = Tree::new("root");
        let root = tree.root();
        let d = tree.add_child(root, "d", NodeKind::Dir, 0).unwrap();
        let f = tree.add_child(d, "f.txt", NodeKind::File, 1).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(node_path(&tree, f), format!("root{sep}d{sep}f.txt"));
        assert_eq!(node_path(&tree, root), "root");
    }

    #[test]
    fn tooltip_path_matches_tree_path() {
        // Trailing separator on the root and a Windows-style root must not double the separator.
        let (mut s, _dir) = scanned_session();
        let nodes: Vec<NodeId> = (0..s.tree().len()).filter_map(|i| s.tree().node_at(i)).collect();
        assert!(nodes.len() > 3);
        for id in nodes {
            let expected = s.execute("tree.path", &serde_json::json!({ "node": id.index() })).unwrap();
            assert_eq!(node_path(s.tree(), id), expected["path"].as_str().unwrap());
        }
        let mut tree = Tree::new("C:\\");
        let root = tree.root();
        let f = tree.add_child(root, "f.txt", NodeKind::File, 1).unwrap();
        assert_eq!(node_path(&tree, f), "C:\\f.txt");
    }

    fn scanned_session() -> (Session, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![1u8; 6000]).unwrap();
        std::fs::write(dir.path().join("sub").join("b.txt"), vec![1u8; 3000]).unwrap();
        std::fs::write(dir.path().join("sub").join("c.log"), vec![1u8; 1000]).unwrap();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        let mut s = Session::new();
        assert_eq!(s.finish_scan(handle), Ok(true));
        (s, dir)
    }

    fn run_frame(ctx: &egui::Context, s: &mut Session, st: &mut UiState, events: Vec<egui::Event>) -> usize {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            events,
            ..egui::RawInput::default()
        };
        let out = ctx.run_ui(input, |ui| show(ui, s, st));
        let n = out.shapes.len();
        out.drop_without_applying_deltas();
        n
    }

    /// Runs one frame and returns the colour cache the panel left in egui's memory.
    fn frame_cache(ctx: &egui::Context, s: &mut Session, st: &mut UiState) -> Option<ColorCache> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            ..egui::RawInput::default()
        };
        let mut cache = None;
        let out = ctx.run_ui(input, |ui| {
            show(ui, s, st);
            cache = ui.data(|d| d.get_temp::<ColorCache>(ui.id().with("map_colors")));
        });
        out.drop_without_applying_deltas();
        cache
    }

    fn wait_for_layout(s: &mut Session) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while s.map_poll(bounds_for_size(400.0, 300.0)) {
            assert!(std::time::Instant::now() < deadline, "background layout never finished");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn a_theme_switch_rebuilds_the_tile_colours() {
        // G-066: the cache key had no theme, so colours stayed stale after a switch.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("empty")).unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![1u8; 6000]).unwrap();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        let mut s = Session::new();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let mut st = UiState::default();
        let ctx = egui::Context::default();
        let _ = frame_cache(&ctx, &mut s, &mut st);
        wait_for_layout(&mut s);
        let dark = frame_cache(&ctx, &mut s, &mut st).expect("cache after the layout arrived");
        assert_eq!(dark.key.theme, ThemeKind::Dark);
        let dark_tokens = Tokens::for_kind(ThemeKind::Dark);
        let light_tokens = Tokens::for_kind(ThemeKind::Light);
        assert_ne!(dark_tokens.surface, light_tokens.surface);
        let tree = s.tree().clone();
        let tiles: Vec<MapTile> = s.map_ready_view().1.tiles().to_vec();
        let expected = |tokens: &Tokens| -> Vec<Rgba> {
            tiles
                .iter()
                .map(|t| tree.get(t.node).map_or(tokens.surface, |n| tile_color(n.kind(), n.name(), tokens, false)))
                .collect()
        };
        assert_eq!(*dark.colors, expected(&dark_tokens));
        st.theme = ThemeKind::Light;
        let light = frame_cache(&ctx, &mut s, &mut st).expect("cache");
        assert_eq!(light.key.theme, ThemeKind::Light, "the key follows the theme");
        assert_ne!(light.key, dark.key);
        assert_eq!(*light.colors, expected(&light_tokens), "colours are rebuilt with the light tokens");
        let has_dir_tile = tiles.iter().any(|t| tree.get(t.node).is_some_and(|n| n.kind() == NodeKind::Dir));
        if has_dir_tile {
            assert_ne!(*dark.colors, *light.colors, "a collapsed directory uses the theme's surface colour");
        }
    }

    #[test]
    fn headless_frame_paints_tiles_and_click_selects() {
        let (mut s, _dir) = scanned_session();
        let mut st = UiState::default();
        let ctx = egui::Context::default();
        // The first frame only starts the background layout; frames follow until it has arrived.
        run_frame(&ctx, &mut s, &mut st, vec![]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while s.map_poll(bounds_for_size(400.0, 300.0)) {
            assert!(std::time::Instant::now() < deadline, "background layout never finished");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let painted = run_frame(&ctx, &mut s, &mut st, vec![]);
        assert!(painted >= 4, "painted {painted} shapes");
        assert!(s.map_state_view().tile_count >= 4);
        let first = s.map_layout(bounds_for_size(400.0, 300.0)).tiles().as_ptr();
        // Move then click in the panel (hover first so egui registers the pointer).
        let pos = Pos2::new(20.0, 20.0);
        let click = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        run_frame(&ctx, &mut s, &mut st, vec![egui::Event::PointerMoved(pos)]);
        run_frame(&ctx, &mut s, &mut st, vec![click(true)]);
        run_frame(&ctx, &mut s, &mut st, vec![click(false)]);
        assert!(s.selection().is_some(), "click selected a tile");
        assert_eq!(s.map_layout(bounds_for_size(400.0, 300.0)).tiles().as_ptr(), first, "layout cached");
    }

    /// Number of painted textured meshes that are not the font atlas, i.e. cushion images.
    fn image_shapes(ctx: &egui::Context, s: &mut Session, st: &mut UiState) -> usize {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
            ..egui::RawInput::default()
        };
        let out = ctx.run_ui(input, |ui| show(ui, s, st));
        let n = out
            .shapes
            .iter()
            .filter(|c| matches!(&c.shape, egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default()))
            .count();
        out.drop_without_applying_deltas();
        n
    }

    fn settled(ctx: &egui::Context, s: &mut Session, st: &mut UiState) {
        run_frame(ctx, s, st, vec![]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while s.map_poll(bounds_for_size(400.0, 300.0)) {
            assert!(std::time::Instant::now() < deadline, "background layout never finished");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn cushion_is_on_by_default_cached_and_switches_to_flat() {
        let (mut s, _dir) = scanned_session();
        let (mut st, ctx) = (UiState::default(), egui::Context::default());
        settled(&ctx, &mut s, &mut st);
        assert!(s.settings().treemap.cushion);
        // Frames go on until the background render has delivered its picture.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while image_shapes(&ctx, &mut s, &mut st) == 0 {
            assert!(std::time::Instant::now() < deadline, "cushion image never arrived");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let jobs = cushion_view::jobs_started(&ctx);
        assert!(jobs >= 1);
        for _ in 0..5 {
            assert_eq!(image_shapes(&ctx, &mut s, &mut st), 1);
        }
        assert_eq!(cushion_view::jobs_started(&ctx), jobs, "no re-render while nothing changed");
        // Off: flat tiles, no image, and the texture is released.
        s.execute("treemap.cushion", &serde_json::json!({ "enabled": false })).unwrap();
        let flat = run_frame(&ctx, &mut s, &mut st, vec![]);
        assert_eq!(image_shapes(&ctx, &mut s, &mut st), 0);
        assert!(flat >= 4, "flat fallback paints the tiles, painted {flat}");
        assert_eq!(cushion_view::jobs_started(&ctx), jobs);
        // Back on: one new picture.
        s.execute("treemap.cushion", &serde_json::json!({})).unwrap();
        while image_shapes(&ctx, &mut s, &mut st) == 0 {
            assert!(std::time::Instant::now() < deadline, "cushion image never came back");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(cushion_view::jobs_started(&ctx), jobs + 1);
    }

    #[test]
    fn a_new_look_or_grid_renders_one_new_picture_and_an_unchanged_one_does_not() {
        let (mut s, _dir) = scanned_session();
        let (mut st, ctx) = (UiState::default(), egui::Context::default());
        settled(&ctx, &mut s, &mut st);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let wait_image = |ctx: &egui::Context, s: &mut Session, st: &mut UiState, jobs: u64| {
            // The old picture stays on screen (1 image shape) while the new one renders.
            while cushion_view::jobs_started(ctx) < jobs || cushion_view::is_rendering(ctx) {
                assert_eq!(image_shapes(ctx, s, st), 1, "a stand-in picture keeps the treemap visible");
                assert!(std::time::Instant::now() < deadline, "render never finished");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert_eq!(image_shapes(ctx, s, st), 1);
        };
        while image_shapes(&ctx, &mut s, &mut st) == 0 {
            assert!(std::time::Instant::now() < deadline, "cushion image never arrived");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let jobs = cushion_view::jobs_started(&ctx);
        s.execute("treemap.look", &serde_json::json!({ "height": 0.9 })).unwrap();
        assert_eq!(image_shapes(&ctx, &mut s, &mut st), 1);
        wait_image(&ctx, &mut s, &mut st, jobs + 1);
        assert_eq!(cushion_view::jobs_started(&ctx), jobs + 1, "a new look renders once");
        // The same value again changes nothing.
        s.execute("treemap.look", &serde_json::json!({ "height": 0.9 })).unwrap();
        for _ in 0..3 {
            assert_eq!(image_shapes(&ctx, &mut s, &mut st), 1);
        }
        assert_eq!(cushion_view::jobs_started(&ctx), jobs + 1, "unchanged look: cached");
        s.execute("treemap.grid", &serde_json::json!({ "enabled": true })).unwrap();
        assert_eq!(image_shapes(&ctx, &mut s, &mut st), 1);
        wait_image(&ctx, &mut s, &mut st, jobs + 2);
        assert_eq!(cushion_view::jobs_started(&ctx), jobs + 2, "the grid is part of the picture");
        // Resetting the settings restores the default look (one more picture, never a blank map).
        s.execute("settings.reset", &serde_json::json!({})).unwrap();
        assert_eq!(image_shapes(&ctx, &mut s, &mut st), 1);
    }

    #[test]
    fn grid_makes_the_flat_outline_stronger() {
        let (mut s, _dir) = scanned_session();
        let (mut st, ctx) = (UiState::default(), egui::Context::default());
        settled(&ctx, &mut s, &mut st);
        s.execute("treemap.cushion", &serde_json::json!({ "enabled": false })).unwrap();
        let alpha_of_leaf_strokes = |ctx: &egui::Context, s: &mut Session, st: &mut UiState| -> Vec<u8> {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 300.0))),
                ..egui::RawInput::default()
            };
            let out = ctx.run_ui(input, |ui| show(ui, s, st));
            let mut alphas: Vec<u8> = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::epaint::Shape::Rect(r) if r.stroke.width > 0.0 && r.fill == Color32::TRANSPARENT => {
                        Some(r.stroke.color.a())
                    }
                    _ => None,
                })
                .collect();
            alphas.sort_unstable();
            out.drop_without_applying_deltas();
            alphas
        };
        let plain = alpha_of_leaf_strokes(&ctx, &mut s, &mut st);
        s.execute("treemap.grid", &serde_json::json!({ "enabled": true })).unwrap();
        let grid = alpha_of_leaf_strokes(&ctx, &mut s, &mut st);
        assert!(plain.contains(&70) && !plain.contains(&170), "{plain:?}");
        assert!(grid.contains(&170) && !grid.contains(&70), "{grid:?}");
    }

    /// A folder with two big files and many tiny ones (its tiny children merge into a remainder
    /// tile that carries the folder's id), next to a plain folder.
    fn remainder_session() -> (Session, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big");
        std::fs::create_dir(&big).unwrap();
        for f in 0..2 {
            std::fs::write(big.join(format!("f{f}.bin")), vec![1u8; 1_000_000]).unwrap();
        }
        for f in 0..300 {
            std::fs::write(big.join(format!("t{f}.txt")), [1u8]).unwrap();
        }
        std::fs::create_dir(dir.path().join("other")).unwrap();
        std::fs::write(dir.path().join("other").join("o.bin"), vec![1u8; 1_500_000]).unwrap();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        let mut s = Session::new();
        assert_eq!(s.finish_scan(handle), Ok(true));
        // Allocated sizes would give each tiny file a whole cluster.
        s.set_map_size_mode(SizeMode::Logical);
        (s, dir)
    }

    /// Screen rects of the 2 px accent selection outline in one frame at `size`.
    fn selection_outlines(ctx: &egui::Context, s: &mut Session, st: &mut UiState, size: (f32, f32)) -> Vec<egui::Rect> {
        let accent = Tokens::color(Tokens::for_kind(st.theme).accent);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(size.0, size.1))),
            ..egui::RawInput::default()
        };
        let out = ctx.run_ui(input, |ui| show(ui, s, st));
        let rects = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.stroke.width == 2.0 && r.stroke.color == accent => Some(r.rect),
                _ => None,
            })
            .collect();
        out.drop_without_applying_deltas();
        rects
    }

    /// G-072 (QA-250): the outline of a folder was drawn around its remainder sliver (a tile with
    /// the folder's id, painted later) instead of around the folder, in some window sizes only.
    #[test]
    fn a_selected_folder_is_outlined_around_its_own_tile_at_every_size() {
        for size in [(760.0, 560.0), (1000.0, 300.0), (1920.0, 1080.0)] {
            let (mut s, _dir) = remainder_session();
            let (mut st, ctx) = (UiState::default(), egui::Context::default());
            let bounds = bounds_for_size(size.0, size.1);
            let _ = selection_outlines(&ctx, &mut s, &mut st, size);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while s.map_poll(bounds) {
                assert!(std::time::Instant::now() < deadline, "background layout never finished");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let big = s
                .tree()
                .children(s.tree().root())
                .find(|&c| s.tree().get(c).is_some_and(|n| n.name() == "big"))
                .unwrap();
            assert!(s.select_node(big));
            let tiles = s.map_ready_view().1.tiles().to_vec();
            let own = tiles.iter().find(|t| t.node == big && !t.remainder).unwrap().rect;
            assert!(
                tiles.iter().any(|t| t.node == big && t.remainder),
                "{size:?}: the fixture must produce a remainder tile for the folder"
            );
            let outlines = selection_outlines(&ctx, &mut s, &mut st, size);
            assert_eq!(outlines.len(), 1, "{size:?}: exactly one selection outline");
            let r = outlines[0];
            assert!(r.width() > 20.0 && r.height() > 20.0, "{size:?}: outline {r:?} is a sliver");
            // The outline is the folder's tile (panel-local rect shifted by the panel origin).
            assert!(
                (r.width() - own.w).abs() < 0.01 && (r.height() - own.h).abs() < 0.01,
                "{size:?}: {r:?} vs {own:?}"
            );
        }
    }

    #[test]
    fn empty_session_and_tiny_panel_do_not_panic() {
        let mut s = Session::new();
        let mut st = UiState::default();
        let ctx = egui::Context::default();
        run_frame(&ctx, &mut s, &mut st, vec![]);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(0.5, 0.5))),
            ..egui::RawInput::default()
        };
        let out = ctx.run_ui(input, |ui| show(ui, &mut s, &mut st));
        out.drop_without_applying_deltas();
    }
}
