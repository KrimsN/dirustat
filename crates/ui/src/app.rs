//! The application shell: menu bar, top bar, status bar and the central panels.

use crate::state::{ScanStatus, UiState};
use crate::theme::{ThemeKind, Tokens};
use crate::{ext_panel, map_panel, tree_panel};
use dirustat_engine::Session;
use dirustat_model::SizeMode;
use dirustat_scan::{Progress, ScanHandle, ScanOptions};
use std::path::Path;
use std::time::Duration;

/// How often the UI repaints while a scan runs.
const SCAN_REPAINT: Duration = Duration::from_millis(50);

/// The egui application. The scan runs on its own thread; the UI only polls its counters.
#[derive(Debug)]
pub struct DirustatApp {
    session: Session,
    state: UiState,
    tokens: Tokens,
    scan: Option<ScanHandle>,
    applied_theme: Option<ThemeKind>,
}

impl DirustatApp {
    pub fn new() -> Self {
        Self::with_session(Session::new())
    }

    pub fn with_session(session: Session) -> Self {
        Self { session, state: UiState::default(), tokens: Tokens::default(), scan: None, applied_theme: None }
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Starts a scan of the path in the text field. Errors become the scan status, never a panic.
    pub fn start_scan(&mut self) {
        if self.scan.is_some() {
            return;
        }
        let text = self.state.path_text.trim();
        if text.is_empty() {
            self.state.scan = ScanStatus::Failed { message: "enter a folder path".to_owned() };
            return;
        }
        match dirustat_scan::start(Path::new(text), ScanOptions::default()) {
            Ok(handle) => {
                self.state.scan = running_status(handle.progress());
                self.scan = Some(handle);
            }
            Err(err) => self.state.scan = ScanStatus::Failed { message: err.to_string() },
        }
    }

    /// Asks a running scan to stop; it finishes on a later frame as `Cancelled`.
    pub fn cancel_scan(&mut self) {
        if let Some(handle) = &self.scan {
            handle.cancel();
        }
    }

    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Polls the running scan without blocking: `join` is only called once the thread has finished.
    fn poll_scan(&mut self, ctx: &egui::Context) {
        let Some(handle) = self.scan.take() else {
            return;
        };
        if !handle.is_finished() {
            self.state.scan = running_status(handle.progress());
            self.scan = Some(handle);
            ctx.request_repaint_after(SCAN_REPAINT);
            return;
        }
        let errors = handle.progress().errors;
        self.state.scan = match self.session.finish_scan(handle) {
            Ok(true) => ScanStatus::Done { errors },
            Ok(false) => ScanStatus::Cancelled,
            Err(err) => ScanStatus::Failed { message: err.to_string() },
        };
        ctx.request_repaint();
    }

    /// Draws one frame into `ui`. Separate from `eframe::App::ui` so tests can run it headless.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.applied_theme != Some(self.state.theme) {
            self.tokens = Tokens::for_kind(self.state.theme);
            self.tokens.apply(ui.ctx());
            self.applied_theme = Some(self.state.theme);
        }
        self.poll_scan(&ui.ctx().clone());
        self.menu_bar(ui);
        self.top_bar(ui);
        if self.state.panels.status_bar {
            self.status_bar(ui);
        }
        self.central(ui);
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        ui.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    ui.weak("(empty)");
                });
                ui.menu_button("View", |ui| {
                    ui.checkbox(&mut self.state.panels.tree, "Directory tree");
                    ui.checkbox(&mut self.state.panels.extensions, "File types");
                    ui.checkbox(&mut self.state.panels.map, "Treemap");
                    ui.checkbox(&mut self.state.panels.status_bar, "Status bar");
                    ui.separator();
                    ui.radio_value(&mut self.state.theme, ThemeKind::Dark, "Dark theme");
                    ui.radio_value(&mut self.state.theme, ThemeKind::Light, "Light theme");
                });
                ui.menu_button("Cleanup", |ui| {
                    ui.weak("(empty)");
                });
                ui.menu_button("Help", |ui| {
                    ui.weak("diRUSTat");
                });
            });
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let scanning = self.is_scanning();
        let mut start = false;
        let mut cancel = false;
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Path:");
                let width = (ui.available_width() - 120.0).max(80.0);
                let edit = ui.add_enabled(
                    !scanning,
                    egui::TextEdit::singleline(&mut self.state.path_text)
                        .desired_width(width)
                        .hint_text("Folder to scan"),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if scanning {
                    cancel = ui.button("Stop").clicked();
                } else {
                    start = ui.button("Scan").clicked() || enter;
                }
            });
        });
        if start {
            self.start_scan();
        }
        if cancel {
            self.cancel_scan();
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let tokens = &self.tokens;
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let tree = self.session.tree();
                let total = tree.get(tree.root()).map_or(0, |n| n.total_size(SizeMode::default()));
                ui.label(format!("{} nodes", tree.len()));
                ui.separator();
                ui.label(human_size(total));
                ui.separator();
                match &self.state.scan {
                    ScanStatus::Idle => {
                        ui.colored_label(Tokens::color(tokens.text_muted), "Idle");
                    }
                    ScanStatus::Running { dirs, files, bytes, errors } => {
                        ui.spinner();
                        ui.label(format!("Scanning: {dirs} folders, {files} files, {}", human_size(*bytes)));
                        if *errors > 0 {
                            ui.colored_label(Tokens::color(tokens.warning), format!("{errors} errors"));
                        }
                    }
                    ScanStatus::Done { errors } => {
                        ui.colored_label(Tokens::color(tokens.success), "Scan complete");
                        if *errors > 0 {
                            ui.colored_label(Tokens::color(tokens.warning), format!("{errors} errors"));
                        }
                    }
                    ScanStatus::Cancelled => {
                        ui.colored_label(Tokens::color(tokens.warning), "Scan cancelled");
                    }
                    ScanStatus::Failed { message } => {
                        ui.colored_label(Tokens::color(tokens.error), message);
                    }
                }
            });
        });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let panels = self.state.panels;
        if panels.tree {
            egui::Panel::left("tree_panel").default_size(320.0).show(ui, |ui| {
                tree_panel::show(ui, &self.session, &mut self.state);
            });
        }
        if panels.extensions {
            egui::Panel::right("ext_panel").default_size(220.0).show(ui, |ui| {
                ext_panel::show(ui, &self.session, &mut self.state);
            });
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if panels.map {
                map_panel::show(ui, &self.session, &mut self.state);
            }
        });
    }
}

impl Default for DirustatApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for DirustatApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

fn running_status(p: Progress) -> ScanStatus {
    ScanStatus::Running { dirs: p.dirs, files: p.files, bytes: p.bytes, errors: p.errors }
}

/// Formats a byte count with binary units, using integer math for the fraction.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut whole = bytes;
    let mut rem = 0u64;
    let mut unit = 0usize;
    while whole >= 1024 && unit < UNITS.len().saturating_sub(1) {
        rem = whole % 1024;
        whole /= 1024;
        unit = unit.saturating_add(1);
    }
    let name = UNITS.get(unit).copied().unwrap_or("B");
    if unit == 0 {
        format!("{whole} {name}")
    } else {
        let tenth = rem.saturating_mul(10) / 1024;
        format!("{whole}.{tenth} {name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Runs one headless frame and returns the number of emitted shapes.
    fn frame(app: &mut DirustatApp, ctx: &egui::Context) -> usize {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| app.show(ui));
        let shapes = out.shapes.len();
        out.drop_without_applying_deltas();
        shapes
    }

    #[test]
    fn headless_frame_builds_in_both_themes() {
        let ctx = egui::Context::default();
        let mut app = DirustatApp::new();
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            app.state_mut().theme = theme;
            assert!(frame(&mut app, &ctx) > 0);
        }
    }

    #[test]
    fn hidden_panels_still_build() {
        let ctx = egui::Context::default();
        let mut app = DirustatApp::new();
        app.state_mut().panels =
            crate::PanelVisibility { tree: false, map: false, extensions: false, status_bar: false };
        let _ = frame(&mut app, &ctx);
    }

    #[test]
    fn empty_and_bad_paths_become_failed_status() {
        let mut app = DirustatApp::new();
        app.start_scan();
        assert!(matches!(app.state().scan, ScanStatus::Failed { .. }));
        app.state_mut().path_text = "/definitely/not/here/\u{0}".to_owned();
        app.start_scan();
        assert!(matches!(app.state().scan, ScanStatus::Failed { .. }) || app.is_scanning());
    }

    #[test]
    fn scan_completes_through_frames_without_blocking() {
        let dir = tempfile::tempdir();
        let Ok(dir) = dir else { return };
        assert!(std::fs::write(dir.path().join("a.bin"), vec![0u8; 4096]).is_ok());
        let ctx = egui::Context::default();
        let mut app = DirustatApp::new();
        app.state_mut().path_text = dir.path().to_string_lossy().into_owned();
        app.start_scan();
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.is_scanning() && Instant::now() < deadline {
            let _ = frame(&mut app, &ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = frame(&mut app, &ctx);
        assert_eq!(app.state().scan, ScanStatus::Done { errors: 0 });
        assert!(app.session().tree().len() >= 2);
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert!(human_size(u64::MAX).ends_with("PiB"));
    }
}
