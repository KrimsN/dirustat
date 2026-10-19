//! Drive picker: the volumes from `list.drives` with fill bars, shown on the start screen and in
//! the "Drives" menu of the top bar. Clicking a drive starts a scan of its root.

use crate::app::human_size;
use crate::theme::Tokens;
use crate::tree_panel::percent_tenths;
use dirustat_engine::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Used share above which the bar turns to the warning colour, in tenths of a percent.
const WARN_TENTHS: u32 = 850;
/// Used share above which the bar turns to the error colour, in tenths of a percent.
const FULL_TENTHS: u32 = 950;
const ROW_W: f32 = 320.0;
/// Narrowest drive row; below this the list scrolls sideways instead of overprinting.
const ROW_MIN_W: f32 = 220.0;
const ROW_H: f32 = 44.0;

/// One volume as shown by the picker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveEntry {
    pub path: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

impl DriveEntry {
    /// Used share of the volume in tenths of a percent (0 when the size is unknown).
    pub fn used_tenths(&self) -> u32 {
        percent_tenths(self.total.saturating_sub(self.free), self.total)
    }

    /// Title of the row: the root plus the label when there is one.
    pub fn title(&self) -> String {
        if self.label.is_empty() { self.path.clone() } else { format!("{}  {}", self.path, self.label) }
    }
}

/// Reads the volumes through the `list.drives` command. A failure means "no drives".
pub fn load(session: &mut Session) -> Vec<DriveEntry> {
    let Ok(value) = session.execute("list.drives", &json!({})) else { return Vec::new() };
    value
        .get("drives")
        .cloned()
        .and_then(|v: Value| serde_json::from_value::<Vec<DriveEntry>>(v).ok())
        .unwrap_or_default()
}

/// Width of a drive row for `available` points: [`ROW_W`] when there is room, narrower otherwise
/// (small windows, high zoom), never below [`ROW_MIN_W`].
pub fn row_width(available: f32) -> f32 {
    if available.is_finite() { available.clamp(ROW_MIN_W, ROW_W) } else { ROW_W }
}

/// Draws one drive row with its fill bar; returns true when it was clicked.
fn drive_row(ui: &mut egui::Ui, drive: &DriveEntry, tokens: &Tokens) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(row_width(ui.available_width()), ROW_H), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(rect, f32::from(tokens.radius_small), Tokens::color(tokens.surface));
    }
    let pad = tokens.spacing;
    painter.text(
        rect.left_top() + egui::vec2(pad, pad),
        egui::Align2::LEFT_TOP,
        drive.title(),
        egui::FontId::proportional(tokens.font_body),
        Tokens::color(tokens.text),
    );
    let tenths = drive.used_tenths();
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + pad, rect.bottom() - pad - 8.0),
        egui::pos2(rect.right() - pad, rect.bottom() - pad),
    );
    painter.rect_filled(track, f32::from(tokens.radius_small), Tokens::color(tokens.background));
    let fill_w = track.width() * f32::from(u16::try_from(tenths).unwrap_or(1000)) / 1000.0;
    if fill_w > 0.5 {
        let color = if tenths >= FULL_TENTHS {
            tokens.error
        } else if tenths >= WARN_TENTHS {
            tokens.warning
        } else {
            tokens.accent
        };
        let fill = egui::Rect::from_min_size(track.min, egui::vec2(fill_w, track.height()));
        painter.rect_filled(fill, f32::from(tokens.radius_small), Tokens::color(color));
    }
    painter.text(
        rect.right_top() + egui::vec2(-pad, pad),
        egui::Align2::RIGHT_TOP,
        format!("{} free of {}", human_size(drive.free), human_size(drive.total)),
        egui::FontId::proportional(tokens.font_small),
        Tokens::color(tokens.text_muted),
    );
    resp.on_hover_text(format!("Scan {}", drive.path)).clicked()
}

/// Draws the drive list; returns the root of the clicked drive.
pub fn show_list(ui: &mut egui::Ui, drives: &[DriveEntry], tokens: &Tokens) -> Option<String> {
    if drives.is_empty() {
        ui.colored_label(Tokens::color(tokens.text_muted), "No drives found");
        return None;
    }
    let mut picked = None;
    for drive in drives {
        if drive_row(ui, drive, tokens) {
            picked = Some(drive.path.clone());
        }
    }
    picked
}

/// What the user chose on the start screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartAction {
    /// Scan the root of this drive.
    Drive(String),
    /// Open the native folder dialog.
    Browse,
}

/// The start screen shown while there is nothing to display.
pub fn show_start_screen(ui: &mut egui::Ui, drives: &[DriveEntry], tokens: &Tokens) -> Option<StartAction> {
    let mut picked = None;
    // Scrolls when the window is small, the search bar is open or the zoom is high (QA-134).
    egui::ScrollArea::vertical().id_salt("start_screen").auto_shrink([false, false]).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.12);
            ui.heading("Choose a drive to scan");
            ui.colored_label(Tokens::color(tokens.text_muted), "or pick any folder");
            ui.add_space(tokens.spacing);
            if ui.button("Browse for a folder…").on_hover_text("Choose the folder to scan in a dialog").clicked() {
                picked = Some(StartAction::Browse);
            }
            ui.add_space(tokens.spacing * 2.0);
            if let Some(root) = show_list(ui, drives, tokens) {
                picked = Some(StartAction::Drive(root));
            }
        });
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_shrink_with_the_window_but_not_below_the_minimum() {
        assert_eq!(row_width(1000.0), ROW_W);
        assert_eq!(row_width(250.0), 250.0);
        assert_eq!(row_width(10.0), ROW_MIN_W);
        assert_eq!(row_width(f32::NAN), ROW_W);
    }

    #[test]
    fn many_drives_in_a_tiny_panel_build_inside_a_scroll_area() {
        let drives: Vec<DriveEntry> = (0..12)
            .map(|i| DriveEntry { path: format!("{i}:\\"), label: "Data".into(), total: 1000, free: 400 })
            .collect();
        let ctx = egui::Context::default();
        for tokens in [Tokens::dark(), Tokens::light()] {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 210.0))),
                ..egui::RawInput::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                assert!(show_start_screen(ui, &drives, &tokens).is_none());
            });
            out.drop_without_applying_deltas();
        }
    }

    #[test]
    fn used_share_and_title() {
        let d = DriveEntry { path: "C:\\".into(), label: "System".into(), total: 200, free: 50 };
        assert_eq!(d.used_tenths(), 750);
        assert_eq!(d.title(), "C:\\  System");
        let unknown = DriveEntry { path: "/".into(), label: String::new(), total: 0, free: 0 };
        assert_eq!(unknown.used_tenths(), 0);
        assert_eq!(unknown.title(), "/");
        let odd = DriveEntry { path: "X:\\".into(), label: String::new(), total: 10, free: 99 };
        assert_eq!(odd.used_tenths(), 0, "free above total never underflows");
    }
}
