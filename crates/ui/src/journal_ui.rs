//! The Journal window (`ui.journal`, Tools > Journal): the destructive actions of this session from
//! `file.journal`, newest first, in pages. The host mirrors the journal to a JSONL file
//! (`Session::set_journal_file`); this window reads the engine's in-memory copy.

use crate::app::human_size;
use crate::theme::Tokens;
use crate::tree_panel::format_date;
use dirustat_engine::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Rows per page.
pub const PAGE_SIZE: usize = 25;

/// One journal row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRow {
    /// Seconds since the Unix epoch.
    pub time: u64,
    pub op: String,
    pub path: String,
    pub destination: Option<String>,
    pub size: u64,
    pub mode: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// State of the window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalState {
    pub open: bool,
    /// Index of the first row shown (a multiple of [`PAGE_SIZE`]).
    pub offset: usize,
    pub total: usize,
    pub rows: Vec<JournalRow>,
    pub error: Option<String>,
    /// The page was read from the session (cleared after an action or a page change).
    #[serde(skip)]
    pub loaded: bool,
}

impl JournalState {
    /// Index of the last page.
    pub fn last_offset(&self) -> usize {
        self.total.saturating_sub(1) / PAGE_SIZE * PAGE_SIZE
    }

    /// Moves one page back (`-1`) or forward (`1`) and marks the page for reload.
    pub fn turn(&mut self, forward: bool) {
        self.offset = if forward {
            self.offset.saturating_add(PAGE_SIZE).min(self.last_offset())
        } else {
            self.offset.saturating_sub(PAGE_SIZE)
        };
        self.loaded = false;
    }
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) of a journal timestamp.
pub fn format_time(secs: u64) -> String {
    let Ok(secs) = i64::try_from(secs) else { return "-".to_owned() };
    let rest = secs.rem_euclid(86_400);
    format!("{} {:02}:{:02}:{:02}", format_date(Some(secs)), rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Reads the current page with `file.journal`.
pub fn load(session: &mut Session, state: &mut JournalState) {
    let params = json!({ "limit": PAGE_SIZE, "offset": state.offset });
    match session.execute("file.journal", &params) {
        Ok(v) => {
            state.total = v.get("total").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
            // A page past the end (entries were never removed, but be safe) goes back to the last one.
            if state.offset > state.last_offset() {
                state.offset = state.last_offset();
                state.loaded = false;
                return;
            }
            state.rows = v
                .get("entries")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|e| JournalRow {
                            time: e.get("time").and_then(Value::as_u64).unwrap_or(0),
                            op: e.get("op").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            path: e.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            destination: e.get("destination").and_then(Value::as_str).map(str::to_owned),
                            size: e.get("size").and_then(Value::as_u64).unwrap_or(0),
                            mode: e.get("mode").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            ok: e.get("ok").and_then(Value::as_bool).unwrap_or(false),
                            error: e.get("error").and_then(Value::as_str).map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default();
            state.error = None;
        }
        Err(e) => state.error = Some(e.to_string()),
    }
    state.loaded = true;
}

/// What the user did in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalAction {
    Refresh,
    Previous,
    Next,
}

/// Draws the window.
pub fn show(ctx: &egui::Context, state: &mut JournalState, tokens: &Tokens) -> Option<JournalAction> {
    let mut action = None;
    let mut open = state.open;
    let muted = Tokens::color(tokens.text_muted);
    egui::Window::new("Journal").open(&mut open).default_width(760.0).default_height(460.0).show(ctx, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new("Destructive actions of this session, newest first. The same entries are appended to journal.jsonl in the settings folder.")
                    .color(muted),
            )
            .wrap(),
        );
        if let Some(error) = &state.error {
            ui.colored_label(Tokens::color(tokens.error), error);
        }
        ui.horizontal(|ui| {
            if ui.button("Refresh").clicked() {
                action = Some(JournalAction::Refresh);
            }
            if ui.add_enabled(state.offset > 0, egui::Button::new("Previous")).clicked() {
                action = Some(JournalAction::Previous);
            }
            let last = state.offset.saturating_add(PAGE_SIZE).min(state.total);
            if ui.add_enabled(last < state.total, egui::Button::new("Next")).clicked() {
                action = Some(JournalAction::Next);
            }
            let first = if state.total == 0 { 0 } else { state.offset.saturating_add(1) };
            ui.colored_label(muted, format!("{first}-{last} of {}", state.total));
        });
        ui.separator();
        if state.rows.is_empty() {
            ui.colored_label(muted, "No destructive actions yet");
            return;
        }
        egui::ScrollArea::vertical().id_salt("journal_rows").auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("journal_grid").striped(true).num_columns(6).spacing([12.0, 4.0]).show(ui, |ui| {
                for h in ["Time (UTC)", "Operation", "Mode", "Result", "Size", "Path"] {
                    ui.label(egui::RichText::new(h).strong());
                }
                ui.end_row();
                for row in &state.rows {
                    ui.label(format_time(row.time));
                    ui.label(&row.op);
                    ui.label(&row.mode);
                    let (text, color) = if row.ok { ("ok", tokens.success) } else { ("failed", tokens.error) };
                    let result = ui.colored_label(Tokens::color(color), text);
                    if let Some(error) = &row.error {
                        result.on_hover_text(error);
                    }
                    ui.label(human_size(row.size));
                    let path = match &row.destination {
                        Some(dest) => format!("{} -> {dest}", row.path),
                        None => row.path.clone(),
                    };
                    crate::text_fit::path_label(ui, &path, tokens.font_small, None);
                    ui.end_row();
                }
            });
        });
    });
    state.open = open;
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats_in_utc() {
        assert_eq!(format_time(0), "1970-01-01 00:00:00");
        assert_eq!(format_time(86_400 + 3_661), "1970-01-02 01:01:01");
        assert_eq!(format_time(u64::MAX), "-");
    }

    #[test]
    fn paging_is_clamped() {
        let mut s = JournalState { total: 60, ..JournalState::default() };
        assert_eq!(s.last_offset(), 50);
        s.turn(false);
        assert_eq!(s.offset, 0);
        s.turn(true);
        s.turn(true);
        s.turn(true);
        assert_eq!(s.offset, 50);
        s.turn(false);
        assert_eq!(s.offset, 25);
        assert_eq!(JournalState::default().last_offset(), 0);
    }
}
