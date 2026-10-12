//! Placeholder for the left directory tree. Filled in by a later task; extend the `show` signature as needed.

use crate::state::UiState;
use dirustat_engine::Session;

/// Draws the panel. Currently a placeholder.
pub fn show(ui: &mut egui::Ui, session: &Session, state: &mut UiState) {
    let _ = (session, state);
    ui.weak("Directory tree (coming soon)");
}
