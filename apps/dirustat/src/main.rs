//! Desktop application: opens the native eframe/wgpu window.
//!
//! Flags are parsed by `dirustat_engine::LaunchArgs` (see `--help`): `--scan <path>`,
//! `--size-mode`, `--demo` (synthetic tree, never a real disk), `--screenshot <file.png>`,
//! `--exit-after-screenshot`, `--size WxH` and `--control <port>` (JSON control channel and MCP
//! bridge, `docs/control-protocol.md`).

mod bridge;

use bridge::UiEndpoint;
use dirustat_control::{ControlServer, SharedSession, default_key_dir, lock_session};
use dirustat_engine::{LAUNCH_USAGE, LaunchArgs, Session, Settings, settings_path};
use dirustat_ui::{DirustatApp, ScreenshotStatus, WINDOW_TITLE};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Frames to render before the screenshot, so panels and the treemap layout are settled.
const WARMUP_FRAMES: u32 = 8;

/// Wraps the app to serve the control bridge and take the requested screenshot after a few frames.
struct Shell {
    app: DirustatApp,
    endpoint: Option<UiEndpoint>,
    screenshot: Option<String>,
    exit_after: bool,
    frames: u32,
    ticket: Option<u64>,
    failed: Arc<AtomicBool>,
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, ui, frame);
        if let Some(endpoint) = self.endpoint.as_mut() {
            endpoint.pump(&mut self.app, ui.ctx());
        }
        self.frames = self.frames.saturating_add(1);
        if self.ticket.is_none()
            && self.frames >= WARMUP_FRAMES
            && !self.app.is_scanning()
            && let Some(path) = self.screenshot.take()
        {
            match self.app.request_screenshot(&path, None) {
                Ok(t) => self.ticket = Some(t),
                Err(message) => self.finish(ui, Some(message)),
            }
        } else if self.ticket.is_none() && self.screenshot.is_some() {
            ui.ctx().request_repaint();
        }
        if let Some(status) = self.ticket.and_then(|t| self.app.screenshot_status(t)) {
            match status {
                ScreenshotStatus::Pending => {}
                ScreenshotStatus::Done { .. } => {
                    self.ticket = None;
                    self.finish(ui, None);
                }
                ScreenshotStatus::Failed { message } => {
                    self.ticket = None;
                    self.finish(ui, Some(message));
                }
            }
        }
    }
}

impl Shell {
    #[allow(clippy::print_stderr)]
    fn finish(&mut self, ui: &eframe::egui::Ui, error: Option<String>) {
        if let Some(message) = error {
            eprintln!("screenshot failed: {message}");
            self.failed.store(true, Ordering::SeqCst);
        }
        if self.exit_after || self.failed.load(Ordering::SeqCst) {
            ui.ctx().send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
    }
}

/// Builds the session from the launch arguments: demo data or saved settings, then `--size-mode`.
#[allow(clippy::print_stderr)]
fn build_session(args: &LaunchArgs) -> Result<Session, String> {
    let mut session = if args.demo {
        dirustat_ui::demo::demo_session().map_err(|m| format!("cannot build demo data: {m}"))?
    } else {
        let mut session = Session::new();
        if let Some(path) = settings_path() {
            let loaded = Settings::load_from(&path);
            if let Some(warning) = loaded.warning {
                eprintln!("warning: {warning}");
            }
            session.apply_settings(loaded.settings);
        }
        session
    };
    if let Some(mode) = args.size_mode {
        session.set_map_size_mode(mode);
    }
    Ok(session)
}

/// Saves the settings on exit; a failure is only a warning. Demo runs never call this.
#[allow(clippy::print_stderr)]
fn save_settings(session: &SharedSession) {
    let Some(path) = settings_path() else { return };
    let settings = lock_session(session).settings().clone();
    if let Err(e) = settings.save_to(&path) {
        eprintln!("warning: {e}");
    }
}

#[allow(clippy::print_stderr, clippy::print_stdout)]
fn main() -> eframe::Result {
    let args = match LaunchArgs::parse(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{LAUNCH_USAGE}");
            std::process::exit(2);
        }
    };
    if args.help {
        println!("{LAUNCH_USAGE}");
        return Ok(());
    }
    let session = match build_session(&args) {
        Ok(s) => s,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    let mut app = DirustatApp::with_session(session);
    let shared = app.shared_session();
    if let Some(path) = &args.scan {
        app.state_mut().path_text = path.to_string_lossy().into_owned();
        app.start_scan();
    }
    let mut endpoint = None;
    let mut server = None;
    if let Some(port) = args.control {
        let Some(key_dir) = default_key_dir() else {
            eprintln!("cannot start the control channel: no per-user config directory");
            std::process::exit(1);
        };
        let (ui_bridge, ui_endpoint) = bridge::channel();
        match ControlServer::start(port, &key_dir, shared.clone(), Arc::new(ui_bridge)) {
            Ok(s) => {
                println!("control: 127.0.0.1:{} key file {}", s.port(), s.key_path().display());
                server = Some(s);
                endpoint = Some(ui_endpoint);
            }
            Err(e) => {
                eprintln!("cannot start the control channel on port {port}: {e}");
                std::process::exit(1);
            }
        }
    }
    let (w, h) = args.size.unwrap_or((1200, 800));
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size([f32::from(w), f32::from(h)]),
        ..eframe::NativeOptions::default()
    };
    let failed = Arc::new(AtomicBool::new(false));
    let shell = Shell {
        app,
        endpoint,
        screenshot: args.screenshot.map(|p| p.to_string_lossy().into_owned()),
        exit_after: args.exit_after_screenshot,
        frames: 0,
        ticket: None,
        failed: failed.clone(),
    };
    let result = eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(move |cc| {
            if let Some(endpoint) = &shell.endpoint {
                endpoint.attach(&cc.egui_ctx);
            }
            Ok(Box::new(shell))
        }),
    );
    if !args.demo {
        save_settings(&shared);
    }
    if let Some(server) = server {
        server.shutdown();
    }
    if failed.load(Ordering::SeqCst) {
        std::process::exit(1);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn parse(v: &[&str]) -> Result<LaunchArgs, dirustat_engine::LaunchError> {
        LaunchArgs::parse(v.iter().map(OsString::from))
    }

    #[test]
    fn launch_flags_cover_the_app_options() {
        let a = parse(&["--demo", "--screenshot", "a.png", "--exit-after-screenshot", "--size", "800x600"]).unwrap();
        assert!(a.demo && a.exit_after_screenshot && a.size == Some((800, 600)));
        assert!(parse(&["--scan", "x", "--control", "0", "--size-mode", "logical"]).is_ok());
        assert!(parse(&["--bogus"]).is_err());
    }

    #[test]
    fn demo_session_honours_size_mode_and_has_a_neutral_root() {
        let args = parse(&["--demo", "--size-mode", "logical"]).unwrap();
        let session = build_session(&args).unwrap();
        assert_eq!(session.map_size_mode(), dirustat_model::SizeMode::Logical);
        let root = session.tree().root();
        assert_eq!(session.tree().get(root).map(|n| n.name()), Some("demo"));
    }
}
