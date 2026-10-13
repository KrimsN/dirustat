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
                let resp = ui
                    .horizontal(|ui| {
                        ui.selectable_label(selected, egui::RichText::new(&e.label).color(color)).on_hover_text(&e.id)
                    })
                    .inner;
                if let Some(sc) = &e.shortcut {
                    ui.painter().text(
                        resp.rect.right_center(),
                        egui::Align2::RIGHT_CENTER,
                        sc,
                        egui::FontId::proportional(tokens.font_small),
                        Tokens::color(tokens.text_muted),
                    );
                }
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
