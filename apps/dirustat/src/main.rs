//! Desktop application: opens the native eframe/wgpu window.
//!
//! Flags for agents and documentation (`docs/development.md`):
//! `--demo` loads a synthetic tree (never a real disk), `--screenshot <file.png>` saves the window
//! once it has rendered, `--exit-after-screenshot` closes the app afterwards,
//! `--size <W>x<H>` sets the window size.

use dirustat_ui::{DirustatApp, ScreenshotStatus, WINDOW_TITLE};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Frames to render before the screenshot, so panels and the treemap layout are settled.
const WARMUP_FRAMES: u32 = 8;

#[derive(Debug, Default)]
struct Args {
    demo: bool,
    screenshot: Option<String>,
    exit_after_screenshot: bool,
    size: Option<[f32; 2]>,
}

fn parse_args<I: Iterator<Item = String>>(mut it: I) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--demo" => args.demo = true,
            "--exit-after-screenshot" => args.exit_after_screenshot = true,
            "--screenshot" => args.screenshot = Some(it.next().ok_or("--screenshot needs a file name")?),
            "--size" => {
                let v = it.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                let dim = |t: &str| t.parse::<u16>().ok().filter(|n| *n >= 200).map(f32::from);
                args.size = Some([dim(w).ok_or("bad width")?, dim(h).ok_or("bad height")?]);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// Wraps the app to take the requested screenshot after a few frames.
struct Shell {
    app: DirustatApp,
    screenshot: Option<String>,
    exit_after: bool,
    frames: u32,
    ticket: Option<u64>,
    failed: Arc<AtomicBool>,
}

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(&mut self.app, ui, frame);
        self.frames = self.frames.saturating_add(1);
        if self.ticket.is_none()
            && self.frames >= WARMUP_FRAMES
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

#[allow(clippy::print_stderr)]
fn main() -> eframe::Result {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(message) => {
            eprintln!(
                "{message}\nusage: dirustat [--demo] [--screenshot FILE.png] [--exit-after-screenshot] [--size WxH]"
            );
            std::process::exit(2);
        }
    };
    let app = if args.demo {
        match dirustat_ui::demo::demo_session() {
            Ok(session) => DirustatApp::with_session(session),
            Err(message) => {
                eprintln!("cannot build demo data: {message}");
                std::process::exit(1);
            }
        }
    } else {
        DirustatApp::new()
    };
    let size = args.size.unwrap_or([1200.0, 800.0]);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title(WINDOW_TITLE).with_inner_size(size),
        ..eframe::NativeOptions::default()
    };
    let failed = Arc::new(AtomicBool::new(false));
    let shell = Shell {
        app,
        screenshot: args.screenshot,
        exit_after: args.exit_after_screenshot,
        frames: 0,
        ticket: None,
        failed: failed.clone(),
    };
    let result = eframe::run_native(WINDOW_TITLE, options, Box::new(move |_cc| Ok(Box::new(shell))));
    if failed.load(Ordering::SeqCst) {
        std::process::exit(1);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Result<Args, String> {
        parse_args(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn flags_parse_and_hostile_values_are_errors() {
        let a = parse(&["--demo", "--screenshot", "a.png", "--exit-after-screenshot", "--size", "800x600"]);
        assert!(a.as_ref().is_ok_and(|a| a.demo && a.exit_after_screenshot && a.size == Some([800.0, 600.0])));
        for bad in [&["--bogus"][..], &["--screenshot"], &["--size", "10x10"], &["--size", "axb"], &["--size", "5"]] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&[]).is_ok());
    }
}
