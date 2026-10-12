//! L4: egui/eframe shell. Thin: reads state from `Session`, sends actions as commands.
//! Colours and radii come only from `theme::Tokens`, never hard-coded. Nothing below L4 knows about egui.
//!
//! Status: skeleton (milestone M1 brings in egui/eframe/wgpu).

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
