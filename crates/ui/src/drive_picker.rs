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

/// Draws one drive row with its fill bar; returns true when it was clicked.
fn drive_row(ui: &mut egui::Ui, drive: &DriveEntry, tokens: &Tokens) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ROW_W, ROW_H), egui::Sense::click());
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

/// The start screen shown while there is nothing to display.
pub fn show_start_screen(ui: &mut egui::Ui, drives: &[DriveEntry], tokens: &Tokens) -> Option<String> {
    let mut picked = None;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.12);
        ui.heading("Choose a drive to scan");
        ui.colored_label(Tokens::color(tokens.text_muted), "or type a folder path above and press Scan");
        ui.add_space(tokens.spacing * 2.0);
        picked = show_list(ui, drives, tokens);
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

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
