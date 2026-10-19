//! The command palette (Ctrl+Shift+P): a searchable list of engine and shell commands.

use crate::state::UiState;
use crate::theme::Tokens;

/// One selectable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub id: String,
    pub label: String,
    pub menu: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
}

/// Maximum rows shown at once.
const MAX_ROWS: usize = 12;

/// Indices of the entries matching every whitespace-separated term of `query`
/// (case-insensitive, over label, menu path and id). Enabled entries come first, order is kept.
pub fn filter(entries: &[PaletteEntry], query: &str) -> Vec<usize> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let hay = format!("{} {} {}", e.label, e.menu, e.id).to_lowercase();
            terms.iter().all(|t| hay.contains(t.as_str()))
        })
        .map(|(i, _)| i)
        .collect();
    hits.sort_by_key(|i| entries.get(*i).is_none_or(|e| !e.enabled));
    hits
}

/// Draws the palette while `state.palette_open`; returns the id of the command to run.
pub fn show(ctx: &egui::Context, state: &mut UiState, tokens: &Tokens, entries: &[PaletteEntry]) -> Option<String> {
    if !state.palette_open {
        return None;
    }
    let hits = filter(entries, &state.palette_query);
    let last = hits.len().saturating_sub(1);
    state.palette_index = state.palette_index.min(last);
    let (down, up, enter, escape) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    if down {
        state.palette_index = state.palette_index.saturating_add(1).min(last);
    }
    if up {
        state.palette_index = state.palette_index.saturating_sub(1);
    }
    let mut chosen = None;
    let mut clicked = None;
    egui::Window::new("Command palette")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
        .fixed_size([460.0, 0.0])
        .show(ctx, |ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut state.palette_query)
                    .desired_width(f32::INFINITY)
                    .hint_text("Type a command"),
            );
            edit.request_focus();
            if edit.changed() {
                state.palette_index = 0;
            }
            ui.separator();
            for (row, idx) in hits.iter().take(MAX_ROWS).enumerate() {
                let Some(e) = entries.get(*idx) else { continue };
                let selected = row == state.palette_index;
                let color = Tokens::color(if e.enabled { tokens.text } else { tokens.text_muted });
                let resp = draw_row(ui, e, selected, color, tokens).on_hover_text(&e.id);
                if resp.clicked() {
                    clicked = Some(*idx);
                }
            }
            if hits.is_empty() {
                ui.colored_label(Tokens::color(tokens.text_muted), "No matching command");
            }
        });
    if enter {
        clicked = hits.get(state.palette_index).copied();
    }
    if let Some(e) = clicked.and_then(|i| entries.get(i))
        && e.enabled
    {
        chosen = Some(e.id.clone());
    }
    if escape || chosen.is_some() {
        state.palette_open = false;
        state.palette_query.clear();
        state.palette_index = 0;
    }
    chosen
}

/// Horizontal room kept between a label and the shortcut text of its row.
const SHORTCUT_GAP: f32 = 12.0;

/// One row: the label at the left, the shortcut right-aligned in its own column. The label is
/// clipped before the shortcut instead of being drawn under it (QA-132).
fn draw_row(
    ui: &mut egui::Ui,
    e: &PaletteEntry,
    selected: bool,
    color: egui::Color32,
    tokens: &Tokens,
) -> egui::Response {
    let row_h = tokens.font_body + tokens.spacing * 1.5;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter_at(rect);
    let radius = f32::from(tokens.radius_small);
    if selected {
        painter.rect_filled(rect, radius, Tokens::color(tokens.accent).gamma_multiply(0.35));
    } else if resp.hovered() {
        painter.rect_filled(rect, radius, Tokens::color(tokens.surface));
    }
    let pad = tokens.spacing;
    let mut label_right = rect.right() - pad;
    if let Some(shortcut) = &e.shortcut {
        let galley = painter.layout_no_wrap(
            shortcut.clone(),
            egui::FontId::proportional(tokens.font_small),
            Tokens::color(tokens.text_muted),
        );
        let x = rect.right() - pad - galley.size().x;
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            Tokens::color(tokens.text_muted),
        );
        label_right = x - SHORTCUT_GAP;
    }
    let clip = egui::Rect::from_min_max(rect.min, egui::pos2(label_right.max(rect.left()), rect.bottom()));
    painter.with_clip_rect(clip).text(
        egui::pos2(rect.left() + pad, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &e.label,
        egui::FontId::proportional(tokens.font_body),
        color,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, label: &str, enabled: bool) -> PaletteEntry {
        PaletteEntry { id: id.into(), label: label.into(), menu: "View/x".into(), shortcut: None, enabled }
    }

    #[test]
    fn filter_matches_terms_case_insensitively_and_puts_enabled_first() {
        let e = vec![entry("scan.cancel", "Cancel scan", false), entry("scan.start", "Scan folder", true)];
        assert_eq!(filter(&e, ""), vec![1, 0]);
        assert_eq!(filter(&e, "SCAN"), vec![1, 0]);
        assert_eq!(filter(&e, "cancel scan"), vec![0]);
        assert_eq!(filter(&e, "scan.start"), vec![1]);
        assert!(filter(&e, "zzz").is_empty());
        assert!(filter(&[], "x").is_empty());
    }

    fn collect_text_rects(shape: &egui::epaint::Shape, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::epaint::Shape::Text(t) => out.push(t.visual_bounding_rect()),
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect_text_rects(s, out)),
            _ => {}
        }
    }

    #[test]
    fn rows_with_shortcuts_draw_in_both_themes() {
        let ctx = egui::Context::default();
        let entries = vec![
            PaletteEntry {
                id: "file.delete".into(),
                label: "Delete (recycle bin)".into(),
                menu: "Edit/Delete".into(),
                shortcut: Some("Delete".into()),
                enabled: true,
            },
            PaletteEntry {
                id: "file.delete_permanent".into(),
                label: "Delete permanently".into(),
                menu: "Edit/Delete permanently".into(),
                shortcut: Some("Shift+Delete".into()),
                enabled: false,
            },
        ];
        for tokens in [Tokens::dark(), Tokens::light()] {
            let mut state = UiState { palette_open: true, palette_query: "del".into(), ..UiState::default() };
            let mut rects = Vec::new();
            // The first frames size the window and load the fonts; the last one has the final layout.
            for _ in 0..4 {
                rects.clear();
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    ..egui::RawInput::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    let _ = show(ui.ctx(), &mut state, &tokens, &entries);
                });
                for clipped in &out.shapes {
                    collect_text_rects(&clipped.shape, &mut rects);
                }
                out.drop_without_applying_deltas();
            }
            assert!(rects.len() >= 4, "{} text shapes", rects.len());
            for (i, a) in rects.iter().enumerate() {
                for b in rects.iter().skip(i + 1) {
                    assert!(!a.intersects(*b), "text {a:?} is drawn over text {b:?}");
                }
            }
        }
    }

    #[test]
    fn closed_palette_returns_nothing_and_open_one_survives_a_frame() {
        let ctx = egui::Context::default();
        let tokens = Tokens::default();
        let entries = vec![entry("scan.start", "Scan folder", true)];
        let mut state = UiState::default();
        assert!(show(&ctx, &mut state, &tokens, &entries).is_none());
        state.palette_open = true;
        state.palette_index = 99;
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            assert!(show(ui.ctx(), &mut state, &tokens, &entries).is_none());
        });
        out.drop_without_applying_deltas();
        assert_eq!(state.palette_index, 0);
    }
}
