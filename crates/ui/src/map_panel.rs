//! The treemap panel: paints `Session::map_layout`, hover highlight, tooltip and click selection.
//!
//! The layout lives in panel-local coordinates (origin at the panel's top-left, whole-pixel size),
//! so it is cached by the session and only recomputed on a tree, size or panel-size change.

use crate::cushion_view;
use crate::state::UiState;
use crate::theme::{Rgba, Tokens};
use dirustat_engine::{MapTile, Session, extension_of};
use dirustat_model::{NodeId, NodeKind, SizeMode, Tree};
use dirustat_treemap::Rect;
use egui::{Color32, Pos2, Sense, Stroke, StrokeKind};
use std::sync::Arc;

/// Tiles thinner than this (in points) are not painted.
const MIN_TILE_PX: f32 = 1.0;
/// Directory frames and leaf outlines are drawn only for tiles at least this large.
const MIN_OUTLINE_PX: f32 = 5.0;
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
    key: (u64, u64, [u32; 2], bool),
    colors: Arc<Vec<Rgba>>,
}

fn cache_key(session: &Session, bounds: Rect) -> (u64, u64, [u32; 2], bool) {
    (
        session.revision(),
        session.map_generation(),
        [bounds.w.to_bits(), bounds.h.to_bits()],
        session.map_size_mode() == SizeMode::Allocated,
    )
}

/// The cushion texture for the current layout (see [`cushion_view`]); `None` while there is none yet.
/// `may_start` is false while a layout is outstanding: the tiles are about to change, so no job is
/// started for them.
fn cushion_texture(
    ctx: &egui::Context,
    tokens: &Tokens,
    (key, bounds): ((u64, u64, [u32; 2], bool), Rect),
    tiles: &[MapTile],
    tree: &Tree,
    colors: &[Rgba],
    may_start: bool,
) -> Option<egui::TextureId> {
    let (revision, generation, bounds_bits, allocated) = key;
    let max_side = ctx.input(|i| i.max_texture_side);
    let (size, scale) = cushion_view::image_geometry(bounds.w, bounds.h, ctx.pixels_per_point(), max_side)?;
    let cushion_key = cushion_view::CushionKey {
        revision,
        generation,
        bounds: bounds_bits,
        allocated,
        theme: tokens.kind,
        scale_bits: scale.to_bits(),
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
            config: dirustat_treemap::CushionConfig::default(),
        })
    })
}

/// Draws the panel. Clicking selects the tile under the pointer through the `select.node` command.
pub fn show(ui: &mut egui::Ui, session: &mut Session, state: &mut UiState) {
    let tokens = Tokens::for_kind(state.theme);
    let avail = ui.available_size();
    let bounds = bounds_for_size(avail.x, avail.y);
    if bounds.w < MIN_TILE_PX || bounds.h < MIN_TILE_PX {
        return;
    }
    let (response, painter) = ui.allocate_painter(egui::vec2(bounds.w, bounds.h), Sense::click());
    let origin = response.rect.min;
    // The layout is computed on a background thread; until it arrives the previous one is shown.
    let layout_pending = session.map_poll(bounds);
    let key = cache_key(session, bounds);
    let cache_id = ui.id().with("map_colors");
    let cached: Option<ColorCache> = ui.data(|d| d.get_temp::<ColorCache>(cache_id));
    let selected = session.selection();
    let mode = session.map_size_mode();
    let cushion_on = session.settings().treemap.cushion;

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
        if let Some((k, v)) = ui.data(|d| d.get_temp::<(((u64, u64, [u32; 2], bool), String), Arc<Vec<bool>>)>(id))
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
    let cushion = if cushion_on {
        cushion_texture(ui.ctx(), &tokens, (key, bounds), tiles, tree, &colors, !layout_pending)
    } else {
        cushion_view::release(ui.ctx());
        None
    };
    if let Some(texture) = cushion {
        let uv = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        painter.image(texture, response.rect, uv, Color32::WHITE);
    }
    let frame_stroke = Stroke::new(1.0, Tokens::color(tokens.border));
    let leaf_stroke = Stroke::new(1.0, Color32::from_black_alpha(70));
    let mut selected_rect = None;
    let mut hovered_rect = None;
    for (tile, color) in tiles.iter().zip(colors.iter()) {
        let r = tile.rect;
        if Some(tile.node) == selected {
            selected_rect = Some(to_screen(origin, r));
        }
        if Some(tile.node) == hovered_node {
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
