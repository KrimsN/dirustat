//! L4: egui/eframe shell. Thin: reads state from `Session`, sends actions as commands.
//! Colours and radii come only from `theme::Tokens`, never hard-coded. Nothing below L4 knows about egui.

mod app;
pub mod cleanups_ui;
pub mod commands;
pub mod context_menu;
pub mod contrast;
pub mod cushion_view;
pub mod delete_dialog;
pub mod demo;
pub mod drive_picker;
pub mod export_png;
pub mod ext_colors;
pub mod ext_panel;
pub mod fonts;
pub mod journal_ui;
pub mod look_prefs;
pub mod map_panel;
pub mod palette;
pub mod prefs;
pub mod screenshot;
pub mod search_panel;
pub mod state;
pub mod text_fit;
pub mod theme;
pub mod toast;
pub mod tree_panel;

pub use app::DirustatApp;
pub use commands::{ExportKind, UI_COMMANDS, UiCommand, UiEffect};
pub use screenshot::{Region, SaveOptions, ScreenshotStatus};
pub use state::{PanelVisibility, ScanStatus, UiState};
pub use theme::{ThemeKind, Tokens};

use dirustat_control::PROTOCOL_VERSION;
use dirustat_engine::Session;

/// Window title: the product name is spelled `diRUSTat`.
pub const WINDOW_TITLE: &str = "diRUSTat";

/// Banner line for the applications.
pub fn banner(session: &Session) -> String {
    format!("{WINDOW_TITLE} (control protocol v{PROTOCOL_VERSION}, {} nodes)", session.tree().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mentions_product_name() {
        assert!(banner(&Session::new()).starts_with("diRUSTat"));
    }
}
