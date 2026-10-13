//! L4: egui/eframe shell. Thin: reads state from `Session`, sends actions as commands.
//! Colours and radii come only from `theme::Tokens`, never hard-coded. Nothing below L4 knows about egui.

mod app;
pub mod commands;
pub mod demo;
pub mod ext_panel;
pub mod map_panel;
pub mod palette;
pub mod screenshot;
pub mod state;
pub mod theme;
pub mod tree_panel;

pub use app::DirustatApp;
pub use commands::{UI_COMMANDS, UiCommand, UiEffect};
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
