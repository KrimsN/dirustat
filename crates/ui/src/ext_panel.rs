//! File-type (extension) list: swatch, extension, file count, size and a percent bar.
//!
//! The statistics come from `Session::extension_stats` and are cached in egui temp memory keyed
//! by `Session::revision()` and the size mode, so a frame never re-walks the tree. Rows are
//! virtualized with `ScrollArea::show_rows`. Clicking a row stores the extension in
//! `UiState::selected_ext` (clicking it again clears it).

use crate::app::human_size;
use crate::state::{ExtSort, ExtSortColumn, UiState};
use crate::theme::Tokens;
use dirustat_engine::{ExtStat, Session, ext_color};
use dirustat_model::SizeMode;
use egui::{Align2, FontId, Id, Rect, Sense, Ui, pos2, vec2};
use std::sync::{Arc, Mutex, PoisonError};

const COUNT_W: f32 = 48.0;
const SIZE_W: f32 = 68.0;
const PERCENT_W: f32 = 64.0;
const SWATCH: f32 = 10.0;

/// Label shown for files without an extension.
const NO_EXT_LABEL: &str = "(no extension)";

/// Cached, already sorted rows plus the key they were built for.
#[derive(Debug, Default)]
struct ExtCache {
    key: Option<(u64, SizeMode)>,
    sort: Option<ExtSort>,
    rows: Vec<ExtStat>,
}

type SharedCache = Arc<Mutex<ExtCache>>;

/// Sorts `rows` in place by `sort`; ties always fall back to extension ascending, so the order
/// is total and deterministic. The percent column orders like the size column (same quantity).
pub(crate) fn sort_stats(rows: &mut [ExtStat], sort: ExtSort) {
    rows.sort_by(|a, b| {
        let ord = match sort.column {
            ExtSortColumn::Extension => a.ext.cmp(&b.ext),
            ExtSortColumn::Files => a.file_count.cmp(&b.file_count),
            ExtSortColumn::Size | ExtSortColumn::Percent => a.size.cmp(&b.size),
        };
        let ord = if sort.descending { ord.reverse() } else { ord };
        ord.then_with(|| a.ext.cmp(&b.ext))
    });
}

/// Order after a click on a column header: same column flips direction, a new column starts
/// descending (ascending for the extension name).
pub(crate) fn next_sort(current: ExtSort, clicked: ExtSortColumn) -> ExtSort {
    if current.column == clicked {
        ExtSort { column: clicked, descending: !current.descending }
    } else {
        ExtSort { column: clicked, descending: clicked != ExtSortColumn::Extension }
    }
}

/// New selection after clicking `clicked`: selecting the selected extension clears it.
pub(crate) fn toggle_selection(current: Option<&str>, clicked: &str) -> Option<String> {
    if current == Some(clicked) { None } else { Some(clicked.to_owned()) }
}

/// Text for the extension column.
pub(crate) fn ext_label(ext: &str) -> String {
    if ext.is_empty() { NO_EXT_LABEL.to_owned() } else { format!(".{ext}") }
}

/// Percent text with one decimal; non-finite input renders as 0.
pub(crate) fn format_percent(percent: f64) -> String {
    let p = if percent.is_finite() { percent.clamp(0.0, 100.0) } else { 0.0 };
    format!("{p:.1}%")
}

/// Bar fill fraction in `0.0..=1.0`.
pub(crate) fn bar_fraction(percent: f64) -> f32 {
    if percent.is_finite() {
        // Narrowing is intended: the value is already within 0..=1.
        #[allow(clippy::cast_possible_truncation)]
        let f = (percent / 100.0).clamp(0.0, 1.0) as f32;
        f
    } else {
        0.0
    }
}

fn header_label(base: &str, column: ExtSortColumn, sort: ExtSort) -> String {
    if sort.column == column {
        format!("{base} {}", if sort.descending { "\u{25BC}" } else { "\u{25B2}" })
    } else {
        base.to_owned()
    }
}

/// Column x ranges `(ext_right, count_right, size_right)` inside a row of the given rect.
fn column_edges(rect: Rect) -> (f32, f32, f32) {
    let size_right = (rect.right() - PERCENT_W).max(rect.left());
    let count_right = (size_right - SIZE_W).max(rect.left());
    let ext_right = (count_right - COUNT_W).max(rect.left());
    (ext_right, count_right, size_right)
}

fn rgba(c: [u8; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(c[0], c[1], c[2])
}

/// Draws the panel.
pub fn show(ui: &mut Ui, session: &Session, state: &mut UiState) {
    let tokens = Tokens::for_kind(state.theme);
    let mode = session.map_size_mode();
    let cache_id = Id::new("dirustat_ext_cache");
    let cache: SharedCache = ui.data_mut(|d| d.get_temp::<SharedCache>(cache_id)).unwrap_or_default();
    {
        let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
        refresh_cache(&mut c, session, mode, state.ext_sort);
    }
    ui.data_mut(|d| d.insert_temp(cache_id, Arc::clone(&cache)));

    let c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    ui.strong("File types");
    if c.rows.is_empty() {
        ui.weak("No files scanned yet");
        return;
    }
    if let Some(column) = header_row(ui, &tokens, state.ext_sort) {
        state.ext_sort = next_sort(state.ext_sort, column);
    }

    let row_h = tokens.font_small + tokens.spacing;
    let mut clicked: Option<String> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, c.rows.len(), |ui, range| {
        for row in c.rows.get(range).unwrap_or_default() {
            let selected = state.selected_ext.as_deref() == Some(row.ext.as_str());
            if draw_row(ui, &tokens, row, selected, row_h) {
                clicked = Some(row.ext.clone());
            }
        }
    });
    if let Some(ext) = clicked {
        state.selected_ext = toggle_selection(state.selected_ext.as_deref(), &ext);
    }
}

fn refresh_cache(c: &mut ExtCache, session: &Session, mode: SizeMode, sort: ExtSort) {
    let key = Some((session.revision(), mode));
    if c.key != key {
        c.rows = session.extension_stats(mode);
        c.key = key;
        c.sort = None;
    }
    if c.sort != Some(sort) {
        sort_stats(&mut c.rows, sort);
        c.sort = Some(sort);
    }
}

/// Draws the header; returns the column clicked this frame.
fn header_row(ui: &mut Ui, tokens: &Tokens, sort: ExtSort) -> Option<ExtSortColumn> {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), tokens.font_small + tokens.spacing), Sense::hover());
    let (ext_right, count_right, size_right) = column_edges(rect);
    let cells = [
        (
            ExtSortColumn::Extension,
            "Type",
            Rect::from_x_y_ranges(rect.left()..=ext_right, rect.y_range()),
            Align2::LEFT_CENTER,
        ),
        (
            ExtSortColumn::Files,
            "Files",
            Rect::from_x_y_ranges(ext_right..=count_right, rect.y_range()),
            Align2::RIGHT_CENTER,
        ),
        (
            ExtSortColumn::Size,
            "Size",
            Rect::from_x_y_ranges(count_right..=size_right, rect.y_range()),
            Align2::RIGHT_CENTER,
        ),
        (
            ExtSortColumn::Percent,
            "%",
            Rect::from_x_y_ranges(size_right..=rect.right(), rect.y_range()),
            Align2::RIGHT_CENTER,
        ),
    ];
    let font = FontId::proportional(tokens.font_small);
    let mut hit = None;
    for (column, base, cell, align) in cells {
        let resp = ui.interact(cell, ui.id().with(("ext_header", base)), Sense::click());
        let colour = if sort.column == column { tokens.accent } else { tokens.text_muted };
        let anchor = if align == Align2::LEFT_CENTER { cell.left_center() } else { cell.right_center() };
        ui.painter_at(cell).text(anchor, align, header_label(base, column, sort), font.clone(), Tokens::color(colour));
        if resp.clicked() {
            hit = Some(column);
        }
    }
    hit
}

/// Draws one row; returns true when it was clicked.
fn draw_row(ui: &mut Ui, tokens: &Tokens, row: &ExtStat, selected: bool, row_h: f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
    let painter = ui.painter_at(rect);
    let radius = egui::CornerRadius::same(tokens.radius_small);
    if selected {
        let [r, g, b, _] = tokens.accent;
        painter.rect_filled(rect, radius, egui::Color32::from_rgba_unmultiplied(r, g, b, 70));
    } else if resp.hovered() {
        painter.rect_filled(rect, radius, Tokens::color(tokens.surface));
    }
    let (ext_right, count_right, size_right) = column_edges(rect);
    let font = FontId::proportional(tokens.font_small);
    let text = Tokens::color(tokens.text);
    let muted = Tokens::color(tokens.text_muted);
    let mid_y = rect.center().y;

    let swatch = Rect::from_center_size(pos2(rect.left() + SWATCH, mid_y), vec2(SWATCH, SWATCH));
    painter.rect_filled(swatch, egui::CornerRadius::same(2), rgba(ext_color(&row.ext)));
    let ext_painter = ui.painter_at(Rect::from_x_y_ranges(
        (swatch.right() + 4.0)..=ext_right.max(swatch.right() + 4.0),
        rect.y_range(),
    ));
    ext_painter.text(pos2(swatch.right() + 6.0, mid_y), Align2::LEFT_CENTER, ext_label(&row.ext), font.clone(), text);
    painter.text(pos2(count_right, mid_y), Align2::RIGHT_CENTER, row.file_count.to_string(), font.clone(), muted);
    painter.text(pos2(size_right, mid_y), Align2::RIGHT_CENTER, human_size(row.size), font.clone(), text);

    let bar = Rect::from_x_y_ranges((size_right + 6.0).min(rect.right())..=rect.right(), rect.y_range())
        .shrink2(vec2(0.0, 3.0));
    painter.rect_filled(bar, radius, Tokens::color(tokens.border));
    let fill = Rect::from_min_size(bar.min, vec2(bar.width() * bar_fraction(row.percent), bar.height()));
    painter.rect_filled(fill, radius, rgba(ext_color(&row.ext)));
    painter.text(bar.right_center() - vec2(2.0, 0.0), Align2::RIGHT_CENTER, format_percent(row.percent), font, text);
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(ext: &str, count: u64, size: u64) -> ExtStat {
        ExtStat { ext: ext.into(), file_count: count, size, percent: 0.0 }
    }

    fn exts(rows: &[ExtStat]) -> Vec<&str> {
        rows.iter().map(|r| r.ext.as_str()).collect()
    }

    #[test]
    fn sorts_by_each_column_both_ways_with_stable_ties() {
        let mut rows = vec![stat("b", 2, 10), stat("a", 2, 10), stat("c", 9, 5)];
        let sort = |column, descending| ExtSort { column, descending };
        sort_stats(&mut rows, sort(ExtSortColumn::Size, true));
        assert_eq!(exts(&rows), ["a", "b", "c"]);
        sort_stats(&mut rows, sort(ExtSortColumn::Size, false));
        assert_eq!(exts(&rows), ["c", "a", "b"]);
        sort_stats(&mut rows, sort(ExtSortColumn::Files, true));
        assert_eq!(exts(&rows), ["c", "a", "b"]);
        sort_stats(&mut rows, sort(ExtSortColumn::Extension, false));
        assert_eq!(exts(&rows), ["a", "b", "c"]);
        sort_stats(&mut rows, sort(ExtSortColumn::Extension, true));
        assert_eq!(exts(&rows), ["c", "b", "a"]);
        sort_stats(&mut rows, sort(ExtSortColumn::Percent, true));
        assert_eq!(exts(&rows), ["a", "b", "c"]);
    }

    #[test]
    fn header_click_flips_or_switches() {
        let d = ExtSort::default();
        assert_eq!((d.column, d.descending), (ExtSortColumn::Size, true));
        assert!(!next_sort(d, ExtSortColumn::Size).descending);
        assert_eq!(
            next_sort(d, ExtSortColumn::Extension),
            ExtSort { column: ExtSortColumn::Extension, descending: false }
        );
        assert!(next_sort(d, ExtSortColumn::Files).descending);
    }

    #[test]
    fn selection_toggles() {
        assert_eq!(toggle_selection(None, "rs"), Some("rs".into()));
        assert_eq!(toggle_selection(Some("rs"), "rs"), None);
        assert_eq!(toggle_selection(Some("rs"), "txt"), Some("txt".into()));
        assert_eq!(toggle_selection(None, ""), Some(String::new()));
    }

    #[test]
    fn formatting_handles_edges() {
        assert_eq!(ext_label(""), NO_EXT_LABEL);
        assert_eq!(ext_label("rs"), ".rs");
        assert_eq!(format_percent(12.345), "12.3%");
        assert_eq!(format_percent(f64::NAN), "0.0%");
        assert_eq!(format_percent(f64::INFINITY), "0.0%");
        assert_eq!(format_percent(-5.0), "0.0%");
        assert_eq!(format_percent(250.0), "100.0%");
        assert!((bar_fraction(50.0) - 0.5).abs() < 1e-6);
        assert_eq!(bar_fraction(f64::NAN), 0.0);
        assert_eq!(bar_fraction(1e9), 1.0);
    }

    #[test]
    fn column_edges_never_invert_in_narrow_rects() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 10.0));
        let (a, b, c) = column_edges(rect);
        assert!(a >= rect.left() && a <= b && b <= c && c <= rect.right());
    }

    #[test]
    fn cache_recomputes_only_on_key_change() {
        let session = Session::new();
        let mut c = ExtCache::default();
        refresh_cache(&mut c, &session, SizeMode::Logical, ExtSort::default());
        c.rows.push(stat("sentinel", 1, 1));
        refresh_cache(&mut c, &session, SizeMode::Logical, ExtSort::default());
        assert!(c.rows.iter().any(|r| r.ext == "sentinel"), "same key must not recompute");
        refresh_cache(&mut c, &session, SizeMode::Allocated, ExtSort::default());
        assert!(c.rows.is_empty(), "mode change recomputes");
    }

    fn frame(session: &Session, state: &mut UiState, ctx: &egui::Context, input: egui::RawInput) -> usize {
        let out = ctx.run_ui(input, |ui| show(ui, session, state));
        let n = out.shapes.len();
        out.drop_without_applying_deltas();
        n
    }

    #[test]
    fn panel_follows_session_size_mode() {
        let ctx = egui::Context::default();
        let mut state = UiState::default();
        let mut session = Session::new();
        let Ok(dir) = tempfile::tempdir() else { return };
        assert!(std::fs::write(dir.path().join("a.bin"), vec![1u8; 10]).is_ok());
        let Ok(handle) = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()) else { return };
        assert!(session.finish_scan(handle).is_ok());
        session.set_map_size_mode(SizeMode::Logical);
        let _ = frame(&session, &mut state, &ctx, egui::RawInput::default());
        let cache = ctx.data_mut(|d| d.get_temp::<SharedCache>(Id::new("dirustat_ext_cache")));
        let Some(cache) = cache else { return };
        let c = cache.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(c.key.map(|k| k.1), Some(SizeMode::Logical));
        assert_eq!(c.rows.iter().map(|r| r.size).sum::<u64>(), 10);
    }

    #[test]
    fn headless_frames_empty_and_scanned() {
        let ctx = egui::Context::default();
        let mut state = UiState::default();
        let mut session = Session::new();
        assert!(frame(&session, &mut state, &ctx, egui::RawInput::default()) > 0);

        let Ok(dir) = tempfile::tempdir() else { return };
        for (name, len) in [("a.bin", 4096usize), ("b.txt", 10), ("noext", 0)] {
            assert!(std::fs::write(dir.path().join(name), vec![0u8; len]).is_ok());
        }
        let Ok(handle) = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()) else { return };
        assert!(session.finish_scan(handle).is_ok());
        for theme in [crate::ThemeKind::Dark, crate::ThemeKind::Light] {
            state.theme = theme;
            assert!(frame(&session, &mut state, &ctx, egui::RawInput::default()) > 0);
        }
        // Several frames with the same session reuse the cache; selection state stays valid.
        state.selected_ext = Some("bin".into());
        let _ = frame(&session, &mut state, &ctx, egui::RawInput::default());
        assert_eq!(state.selected_ext.as_deref(), Some("bin"));
    }
}
