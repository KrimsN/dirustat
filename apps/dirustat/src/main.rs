//! Desktop application: opens the native eframe/wgpu window.

use dirustat_ui::{DirustatApp, WINDOW_TITLE};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title(WINDOW_TITLE).with_inner_size([1200.0, 800.0]),
        ..eframe::NativeOptions::default()
    };
    eframe::run_native(WINDOW_TITLE, options, Box::new(|_cc| Ok(Box::new(DirustatApp::new()))))
}
