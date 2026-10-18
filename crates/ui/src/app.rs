//! The application shell: menu bar, top bar, status bar and the central panels.
//!
//! Every action goes through a command: menus are built from `Session::commands()` plus the
//! shell-only `UI_COMMANDS`, buttons and shortcuts dispatch by id (`DirustatApp::run_command`).

use crate::commands::{self, UI_COMMANDS, UiEffect};
use crate::palette::{self, PaletteEntry};
use crate::screenshot::{Region, SaveOptions, ScreenshotStatus, Screenshots};
use crate::state::{ScanStatus, UiState};
use crate::theme::{ThemeKind, Tokens};
use crate::{ext_panel, map_panel, tree_panel};
use dirustat_control::{SharedSession, lock_session, share};
use dirustat_engine::{CommandInfo, FastPathStatus, Session};
use dirustat_model::SizeMode;
use serde_json::{Value, json};
use std::sync::MutexGuard;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

/// How often the UI repaints while a scan runs.
const SCAN_REPAINT: Duration = Duration::from_millis(50);

/// Top-level menus in display order; other groups follow in registration order.
const MENU_ORDER: [&str; 5] = ["File", "Edit", "View", "Cleanup", "Help"];

/// The egui application. The scan runs inside the session; the UI only polls it.
#[derive(Debug)]
pub struct DirustatApp {
    session: SharedSession,
    state: UiState,
    tokens: Tokens,
    applied_theme: Option<ThemeKind>,
    applied_zoom: Option<u16>,
    shots: Screenshots,
    quit_requested: bool,
    /// Result of the fast-path probe running on a helper thread (opening a volume can block).
    probe: Option<Receiver<(String, FastPathStatus)>>,
    /// The file types panel is enabled but collapsed because the window is too narrow.
    ext_collapsed: bool,
    /// The path field takes keyboard focus on the next frame (Scan folder without a path).
    focus_path: bool,
    /// Pixel area of the treemap panel in the last frame (for the "too small" selection hint).
    map_area_px: f32,
}

/// Share of the window width the treemap always keeps, and its absolute floor in points.
const MAP_MIN_FRACTION: f32 = 0.40;
const MAP_MIN_W: f32 = 200.0;
/// Of the width left over by the treemap, the tree panel may take this share and the file types
/// panel the rest.
const TREE_SHARE: f32 = 0.6;
const TREE_MIN_W: f32 = 190.0;
const EXT_MIN_W: f32 = 240.0;

/// Width limits of the side panels, derived from the window width (in points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SideLayout {
    /// False when the file types panel would not get its minimum width; it is collapsed then.
    pub show_ext: bool,
    pub tree_max: f32,
    pub ext_max: f32,
}

impl SideLayout {
    /// The treemap keeps at least 40 % of the window (and 200 points); the side panels share the
    /// rest, and the file types panel collapses first when there is not enough room.
    pub(crate) fn for_width(total: f32, map_visible: bool) -> Self {
        let total = if total.is_finite() { total.max(0.0) } else { 0.0 };
        let budget = if map_visible { (total - (total * MAP_MIN_FRACTION).max(MAP_MIN_W)).max(0.0) } else { total };
        let ext_max = budget * (1.0 - TREE_SHARE);
        Self { show_ext: ext_max >= EXT_MIN_W, tree_max: budget * TREE_SHARE, ext_max }
    }
}

/// Status text when a scan is requested without a path.
const EMPTY_PATH_MESSAGE: &str = "Type or paste a folder path first";

/// Drops the internal `invalid params: ` prefix of an engine error.
fn strip_prefix(message: &str) -> String {
    message.strip_prefix("invalid params: ").unwrap_or(message).to_owned()
}

/// Turns a `scan.start` failure into text a user can act on (no internal prefixes, no localized
/// OS error text).
fn friendly_scan_error(path: &str, message: &str) -> String {
    let m = strip_prefix(message);
    if m == EMPTY_PATH_MESSAGE || m == "path is empty" {
        EMPTY_PATH_MESSAGE.to_owned()
    } else if m.contains("not a directory") {
        format!("Not a folder: {path}")
    } else if m.contains("cannot read scan root") {
        if m.contains("os error 2") || m.contains("os error 3") {
            format!("Folder not found: {path}")
        } else {
            format!("Cannot open folder: {path}")
        }
    } else {
        m
    }
}

/// A menu item: label, shortcut and state of one command.
struct MenuItem {
    id: String,
    label: String,
    shortcut: Option<&'static str>,
    enabled: bool,
    checked: bool,
}

impl DirustatApp {
    pub fn new() -> Self {
        Self::with_session(Session::new())
    }

    pub fn with_session(session: Session) -> Self {
        Self::with_shared_session(share(session))
    }

    /// Builds the app around a session that is also shared with the control server.
    pub fn with_shared_session(session: SharedSession) -> Self {
        Self {
            session,
            state: UiState::default(),
            tokens: Tokens::default(),
            applied_theme: None,
            applied_zoom: None,
            shots: Screenshots::default(),
            quit_requested: false,
            probe: None,
            ext_collapsed: false,
            focus_path: false,
            map_area_px: 0.0,
        }
    }

    pub fn state(&self) -> &UiState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

    /// Locks the session (poison-tolerant). Do not keep the guard across a frame.
    pub fn session(&self) -> MutexGuard<'_, Session> {
        lock_session(&self.session)
    }

    /// A handle to the shared session, for the control server.
    pub fn shared_session(&self) -> SharedSession {
        SharedSession::clone(&self.session)
    }

    /// True once `ui.quit` ran; the shell closes the window on the next frame.
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }

    /// Serialized UI state for `ui.state`.
    pub fn ui_state_json(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }

    /// Strict, atomic patch for `ui.set_state`; hostile input yields an error, never a panic.
    pub fn apply_ui_patch(&mut self, patch: &Value) -> Result<(), String> {
        self.state.apply_patch(patch)
    }

    /// Queues a trusted window screenshot (the app's own `--screenshot` flag: overwrites and
    /// creates directories); poll with [`Self::screenshot_status`].
    pub fn request_screenshot(&mut self, path: &str, region: Option<Region>) -> Result<u64, String> {
        self.shots.request(path, region)
    }

    /// Queues a screenshot with explicit write rules (`ui.screenshot` from the control channel).
    pub fn request_screenshot_with(
        &mut self,
        path: &str,
        region: Option<Region>,
        options: SaveOptions,
    ) -> Result<u64, String> {
        self.shots.request_with(path, region, options)
    }

    pub fn screenshot_status(&self, ticket: u64) -> Option<ScreenshotStatus> {
        self.shots.status(ticket)
    }

    /// Runs a command by id: shell commands change `UiState`, everything else goes to
    /// `Session::execute`. Errors are also shown in the status bar.
    pub fn run_command(&mut self, id: &str) -> Result<Value, String> {
        let result = self.dispatch(id).map_err(|m| {
            if id == "scan.start" { friendly_scan_error(self.state.path_text.trim(), &m) } else { strip_prefix(&m) }
        });
        match &result {
            Ok(_) => self.state.notice = None,
            Err(message) => {
                if id == "scan.start" {
                    self.state.scan = ScanStatus::Failed { message: message.clone() };
                } else {
                    self.state.notice = Some(message.clone());
                }
            }
        }
        result
    }

    fn dispatch(&mut self, id: &str) -> Result<Value, String> {
        if id == "scan.start" && self.state.path_text.trim().is_empty() {
            self.focus_path = true;
            return Err(EMPTY_PATH_MESSAGE.to_owned());
        }
        if let Some(effect) = commands::run(&mut self.state, id) {
            if effect == UiEffect::Quit {
                self.quit_requested = true;
            }
            return Ok(json!({ "ok": true }));
        }
        let mut session = lock_session(&self.session);
        let infos = session.commands();
        let params = self.default_params(&session, id, &infos).ok_or_else(|| format!("{id} needs parameters"))?;
        let value = session.execute(id, &params).map_err(|e| e.to_string())?;
        let prefer_fast = session.settings().prefer_fast_path;
        drop(session);
        if id == "scan.start" {
            self.state.scan = ScanStatus::Running { dirs: 0, files: 0, bytes: 0, errors: 0 };
            self.refresh_elevation_hint(prefer_fast);
        }
        Ok(value)
    }

    /// Probes the fast path once per scan start (never per frame) and sets the status-bar hint.
    ///
    /// Opening a raw volume can block, so the probe runs on a short helper thread and its result is
    /// picked up by [`Self::poll_probe`] on a later frame. The hint is cleared at once and set only if the
    /// probe says administrator rights are missing.
    fn refresh_elevation_hint(&mut self, prefer_fast: bool) {
        self.state.set_elevation_hint(None, None);
        self.probe = None;
        let Some(volume) = dirustat_engine::volume_root_of(self.state.path_text.trim()).filter(|_| prefer_fast) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("fast-path-probe".to_owned()).spawn(move || {
            let status = dirustat_engine::fast_path_status(&volume);
            // The receiver is gone when a newer probe replaced this one; nothing to report then.
            let _ = tx.send((volume, status));
        });
        if spawned.is_ok() {
            self.probe = Some(rx);
        }
    }

    /// Applies a finished probe, if any, without blocking.
    fn poll_probe(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.probe else { return };
        match rx.try_recv() {
            Ok((volume, status)) => {
                self.state.set_elevation_hint(Some(&volume), Some(&status));
                self.probe = None;
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(SCAN_REPAINT),
            Err(TryRecvError::Disconnected) => self.probe = None,
        }
    }

    /// Parameters for a command invoked without a form (menu, shortcut, palette, button).
    fn default_params(&self, session: &Session, id: &str, infos: &[CommandInfo]) -> Option<Value> {
        match id {
            "scan.start" => Some(json!({ "path": self.state.path_text.trim() })),
            "view.size_mode" => {
                let next = if session.map_size_mode() == SizeMode::Allocated { "logical" } else { "allocated" };
                Some(json!({ "mode": next }))
            }
            _ => {
                let info = infos.iter().find(|i| i.id == id)?;
                info.params_doc.trim_start().starts_with("none").then(|| json!({}))
            }
        }
    }

    /// Whether a command can run without a parameter form.
    fn runnable_without_form(info: &CommandInfo) -> bool {
        matches!(info.id, "scan.start" | "view.size_mode") || info.params_doc.trim_start().starts_with("none")
    }

    /// Starts a scan of the path in the text field. Errors become the scan status, never a panic.
    pub fn start_scan(&mut self) {
        let _ = self.run_command("scan.start");
    }

    /// Asks a running scan to stop; it finishes on a later frame as `Cancelled`.
    pub fn cancel_scan(&mut self) {
        let _ = self.run_command("scan.cancel");
    }

    pub fn is_scanning(&self) -> bool {
        lock_session(&self.session).is_scanning()
    }

    /// Follows the session's scan without blocking: progress while it runs, status when it ends.
    fn poll_scan(&mut self, ctx: &egui::Context) {
        self.poll_probe(ctx);
        let mut session = lock_session(&self.session);
        if session.is_scanning() {
            let errors = Self::progress_into_status(&mut self.state, &session);
            match session.execute("session.poll", &json!({})) {
                Ok(v) if v.get("scanning").and_then(Value::as_bool) == Some(false) => {
                    let replaced = v.get("replaced").and_then(Value::as_bool) == Some(true);
                    self.state.scan = if replaced { ScanStatus::Done { errors } } else { ScanStatus::Cancelled };
                    ctx.request_repaint();
                }
                Ok(_) => ctx.request_repaint_after(SCAN_REPAINT),
                Err(err) => {
                    let message = friendly_scan_error(self.state.path_text.trim(), &err.to_string());
                    self.state.scan = ScanStatus::Failed { message };
                    ctx.request_repaint();
                }
            }
        } else if let ScanStatus::Running { errors, .. } = self.state.scan {
            // Finished elsewhere (for example by a control client polling the session).
            self.state.scan = ScanStatus::Done { errors };
        }
    }

    /// Copies the session's scan progress into the status; returns the error count.
    fn progress_into_status(ui_state: &mut UiState, session: &Session) -> u64 {
        let state = session.state_json();
        let get = |k: &str| state.get("progress").and_then(|p| p.get(k)).and_then(Value::as_u64).unwrap_or(0);
        let (dirs, files, bytes, errors) = (get("dirs"), get("files"), get("bytes"), get("errors"));
        ui_state.scan = ScanStatus::Running { dirs, files, bytes, errors };
        // A scan started through the control channel: show its path in the (disabled) path box.
        if let Some(root) = state.get("scan_root").and_then(Value::as_str)
            && ui_state.path_text != root
        {
            root.clone_into(&mut ui_state.path_text);
        }
        errors
    }

    /// Draws one frame into `ui`. Separate from `eframe::App::ui` so tests can run it headless.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.applied_theme != Some(self.state.theme) {
            self.tokens = Tokens::for_kind(self.state.theme);
            self.tokens.apply(&ctx);
            self.applied_theme = Some(self.state.theme);
        }
        if self.applied_zoom != Some(self.state.zoom_percent) {
            ctx.set_zoom_factor(f32::from(self.state.zoom_percent) / 100.0);
            self.applied_zoom = Some(self.state.zoom_percent);
        }
        self.poll_scan(&ctx);
        self.shots.pump(&ctx);
        let infos = lock_session(&self.session).commands();
        self.handle_shortcuts(&ctx, &infos);
        if let Some(id) = self.menu_bar(ui, &infos) {
            let _ = self.run_command(&id);
        }
        self.top_bar(ui);
        if self.state.panels.status_bar {
            self.status_bar(ui);
        }
        self.central(ui);
        self.elevation_help_window(&ctx);
        let entries = Self::palette_entries(&infos);
        if let Some(id) = palette::show(&ctx, &mut self.state, &self.tokens, &entries) {
            let _ = self.run_command(&id);
        }
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Non-modal explanation opened by `ui.relaunch_elevated_help`. It performs no elevation.
    fn elevation_help_window(&mut self, ctx: &egui::Context) {
        if !self.state.elevation_help_open {
            return;
        }
        let mut open = true;
        let muted = Tokens::color(self.tokens.text_muted);
        egui::Window::new("Administrator rights").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(crate::state::ELEVATION_HINT);
            ui.add_space(self.tokens.spacing);
            ui.label("To use the fast scan, close diRUSTat and start it again with \"Run as administrator\".");
            ui.colored_label(muted, "diRUSTat never elevates by itself; scans without it use the portable walk.");
        });
        self.state.elevation_help_open = open;
    }

    /// Consumes keyboard shortcuts of enabled commands; plain keys are ignored while typing.
    fn handle_shortcuts(&mut self, ctx: &egui::Context, infos: &[CommandInfo]) {
        let typing = ctx.egui_wants_keyboard_input();
        let mut hits: Vec<(String, egui::KeyboardShortcut)> = Vec::new();
        for i in infos.iter().filter(|i| i.enabled && Self::runnable_without_form(i)) {
            if let Some(sc) = i.shortcut.and_then(commands::parse_shortcut) {
                hits.push((i.id.to_owned(), sc));
            }
        }
        for c in UI_COMMANDS {
            if let Some(sc) = c.shortcut.and_then(commands::parse_shortcut) {
                hits.push((c.id.to_owned(), sc));
            }
        }
        let weight = |m: &egui::Modifiers| usize::from(m.command) + usize::from(m.shift) + usize::from(m.alt);
        hits.sort_by_key(|(_, sc)| std::cmp::Reverse(weight(&sc.modifiers)));
        for (id, sc) in hits {
            if typing && weight(&sc.modifiers) == 0 {
                continue;
            }
            if ctx.input_mut(|i| i.consume_shortcut(&sc)) {
                let _ = self.run_command(&id);
            }
        }
    }

    fn palette_entries(infos: &[CommandInfo]) -> Vec<PaletteEntry> {
        let engine = infos.iter().filter(|i| Self::runnable_without_form(i)).map(|i| PaletteEntry {
            id: i.id.to_owned(),
            label: i.label.to_owned(),
            menu: i.menu.to_owned(),
            shortcut: i.shortcut.map(str::to_owned),
            enabled: i.enabled,
        });
        let shell = UI_COMMANDS.iter().map(|c| PaletteEntry {
            id: c.id.to_owned(),
            label: c.label.to_owned(),
            menu: c.menu.to_owned(),
            shortcut: c.shortcut.map(str::to_owned),
            enabled: true,
        });
        engine.chain(shell).collect()
    }

    /// Menu items grouped by top-level menu, engine commands first, shell commands after.
    fn menu_groups(&self, infos: &[CommandInfo]) -> Vec<(String, Vec<MenuItem>)> {
        let mut groups: Vec<(String, Vec<MenuItem>)> =
            MENU_ORDER.iter().map(|n| ((*n).to_owned(), Vec::new())).collect();
        let mut add = |menu: &str, item: MenuItem| {
            let Some((group, _)) = menu.split_once('/') else { return };
            let at = match groups.iter().position(|(g, _)| g == group) {
                Some(at) => at,
                None => {
                    groups.push((group.to_owned(), Vec::new()));
                    groups.len().saturating_sub(1)
                }
            };
            if let Some((_, items)) = groups.get_mut(at) {
                items.push(item);
            }
        };
        for i in infos.iter().filter(|i| !i.menu.is_empty() && Self::runnable_without_form(i)) {
            let label = i.menu.split_once('/').map_or(i.label, |(_, l)| l).to_owned();
            let item =
                MenuItem { id: i.id.to_owned(), label, shortcut: i.shortcut, enabled: i.enabled, checked: false };
            add(i.menu, item);
        }
        for c in UI_COMMANDS.iter().filter(|c| !c.menu.is_empty()) {
            let label = c.menu.split_once('/').map_or(c.label, |(_, l)| l).to_owned();
            let checked = c.checked.is_some_and(|f| f(&self.state));
            add(c.menu, MenuItem { id: c.id.to_owned(), label, shortcut: c.shortcut, enabled: true, checked });
        }
        groups.retain(|(_, items)| !items.is_empty());
        groups
    }

    /// Draws the menu bar; returns the id of the clicked command.
    fn menu_bar(&mut self, ui: &mut egui::Ui, infos: &[CommandInfo]) -> Option<String> {
        let groups = self.menu_groups(infos);
        let mut clicked = None;
        egui::Panel::top("menu_bar").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                for (name, items) in &groups {
                    ui.menu_button(name, |ui| {
                        for item in items {
                            // A bullet marks a checked item (the filled-button style reads as a stray bar).
                            let label =
                                if item.checked { format!("\u{2022} {}", item.label) } else { item.label.clone() };
                            let mut button = egui::Button::new(label);
                            if let Some(sc) = item.shortcut {
                                button = button.shortcut_text(sc);
                            }
                            if ui.add_enabled(item.enabled, button).clicked() {
                                clicked = Some(item.id.clone());
                                ui.close();
                            }
                        }
                    });
                }
            });
        });
        clicked
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let (scanning, mode) = {
            let session = lock_session(&self.session);
            (session.is_scanning(), session.map_size_mode())
        };
        let mut run: Option<&str> = None;
        let size_label = match mode {
            SizeMode::Allocated => "Size: allocated",
            SizeMode::Logical => "Size: logical",
        };
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Path:");
                let width = (ui.available_width() - 260.0).max(80.0);
                let edit = ui.add_enabled(
                    !scanning,
                    egui::TextEdit::singleline(&mut self.state.path_text)
                        .desired_width(width)
                        .hint_text("Folder to scan"),
                );
                if std::mem::take(&mut self.focus_path) {
                    edit.request_focus();
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if scanning {
                    if ui.button("Stop").clicked() {
                        run = Some("scan.cancel");
                    }
                } else if ui.button("Scan").clicked() || enter {
                    run = Some("scan.start");
                }
                if ui.button(size_label).on_hover_text("Toggle logical / allocated sizes").clicked() {
                    run = Some("view.size_mode");
                }
            });
        });
        if let Some(id) = run {
            let _ = self.run_command(id);
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let tokens = &self.tokens;
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let session = lock_session(&self.session);
                let tree = session.tree();
                let mode = session.map_size_mode();
                let total = tree.get(tree.root()).map_or(0, |n| n.total_size(mode));
                let running = matches!(self.state.scan, ScanStatus::Running { .. });
                if running && tree.len() > 1 {
                    let muted = Tokens::color(tokens.text_muted);
                    ui.colored_label(muted, format!("previous scan: {} nodes", tree.len()));
                    ui.separator();
                    ui.colored_label(muted, human_size(total));
                } else {
                    ui.label(format!("{} nodes", tree.len()));
                    ui.separator();
                    ui.label(human_size(total));
                }
                ui.separator();
                match &self.state.scan {
                    ScanStatus::Idle => {
                        ui.colored_label(Tokens::color(tokens.text_muted), "Idle");
                    }
                    ScanStatus::Running { dirs, files, bytes, errors } => {
                        busy_indicator(ui, Tokens::color(tokens.accent));
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
                if let Some(notice) = &self.state.notice {
                    ui.separator();
                    ui.colored_label(Tokens::color(tokens.error), notice);
                }
                let selected_total = session.selection().and_then(|id| tree.get(id)).map(|n| n.total_size(mode));
                if self.state.panels.map
                    && selected_total.is_some_and(|t| selection_too_small(t, total, self.map_area_px))
                {
                    ui.separator();
                    ui.colored_label(Tokens::color(tokens.text_muted), "Selection is too small to show in the treemap");
                }
                if self.ext_collapsed {
                    ui.separator();
                    ui.colored_label(Tokens::color(tokens.text_muted), "File types hidden: window too narrow");
                }
                if self.state.elevation_hint.is_some() {
                    ui.separator();
                    ui.colored_label(Tokens::color(tokens.warning), crate::state::ELEVATION_HINT)
                        .on_hover_text("Help > Administrator rights for fast scan explains how to restart");
                }
            });
        });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        let panels = self.state.panels;
        let mut session = lock_session(&self.session);
        let area = ui.available_rect_before_wrap();
        let layout = SideLayout::for_width(ui.available_width(), panels.map);
        self.ext_collapsed = panels.extensions && !layout.show_ext;
        if panels.tree {
            let tree_max = layout.tree_max;
            egui::Panel::left("tree_panel").default_size(440.0).size_range(TREE_MIN_W.min(tree_max)..=tree_max).show(
                ui,
                |ui| {
                    tree_panel::show(ui, &mut session, &mut self.state, &self.tokens);
                },
            );
        }
        if panels.extensions && layout.show_ext {
            egui::Panel::right("ext_panel").default_size(300.0).size_range(EXT_MIN_W..=layout.ext_max).show(ui, |ui| {
                ext_panel::show(ui, &session, &mut self.state);
            });
        }
        let mut map_area = 0.0;
        egui::CentralPanel::default().show(ui, |ui| {
            if panels.map {
                let size = ui.available_size();
                map_area = size.x.max(0.0) * size.y.max(0.0);
                map_panel::show(ui, &mut session, &mut self.state);
            }
        });
        self.map_area_px = map_area;
        if matches!(self.state.scan, ScanStatus::Running { .. }) && session.tree().len() > 1 {
            stale_overlay(ui, area, &self.tokens);
        }
    }
}

/// Smallest tile area (2 x 2 px) the treemap draws.
const MIN_TILE_AREA_PX: f32 = 4.0;

/// Whether a node of `size` bytes in a tree of `root_total` bytes gets a tile smaller than the
/// treemap's drawing threshold in a panel of `area_px` pixels (an estimate from the size share).
fn selection_too_small(size: u64, root_total: u64, area_px: f32) -> bool {
    if root_total == 0 || !area_px.is_finite() || area_px <= 0.0 {
        return false;
    }
    // Precision loss is irrelevant for an estimate.
    #[allow(clippy::cast_precision_loss)]
    let share = size as f64 / root_total as f64;
    #[allow(clippy::cast_possible_truncation)]
    let px = (share * f64::from(area_px)) as f32;
    px < MIN_TILE_AREA_PX
}

/// Dims the panels that still show the previous scan while a new scan runs, and says so.
fn stale_overlay(ui: &egui::Ui, area: egui::Rect, tokens: &Tokens) {
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("stale_scan_overlay"));
    let painter = ui.ctx().layer_painter(layer).with_clip_rect(area);
    let [r, g, b, _] = tokens.background;
    painter.rect_filled(area, 0.0, egui::Color32::from_rgba_unmultiplied(r, g, b, 150));
    painter.text(
        area.center_top() + egui::vec2(0.0, 28.0),
        egui::Align2::CENTER_CENTER,
        "Previous scan - new scan in progress",
        egui::FontId::proportional(tokens.font_body),
        Tokens::color(tokens.text),
    );
}

/// A small rotating arc drawn from line segments (the UI font has no spinner glyph).
fn busy_indicator(ui: &mut egui::Ui, color: egui::Color32) {
    let size = 14.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let start = t * 6.0;
    let points: Vec<egui::Pos2> = (0..=12)
        .map(|i| {
            let a = start + (i as f32) * 0.35;
            rect.center() + egui::vec2(a.cos(), a.sin()) * (size * 0.4)
        })
        .collect();
    ui.painter().add(egui::Shape::line(points, egui::Stroke::new(2.0, color)));
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
    fn control_started_scan_fills_the_path_box_and_dims_stale_views() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let Some(app) = demo_app() else { return };
        let mut app = app;
        let path = dir.path().to_string_lossy().into_owned();
        assert!(app.session().execute("scan.start", &json!({ "path": path })).is_ok());
        let ctx = egui::Context::default();
        app.poll_scan(&ctx);
        assert_eq!(app.state().path_text, path);
        assert!(matches!(app.state().scan, ScanStatus::Running { .. }));
        // The previous (demo) tree is still shown: frames with the overlay build.
        assert!(frame(&mut app, &ctx) > 0);
    }

    #[test]
    fn tiny_selection_is_flagged() {
        assert!(selection_too_small(295, 5_000_000_000, 600_000.0));
        assert!(!selection_too_small(2_500_000, 5_000_000, 600_000.0));
        assert!(!selection_too_small(1, 0, 600_000.0));
        assert!(!selection_too_small(1, 10, f32::NAN));
        assert!(!selection_too_small(1, 10, 0.0));
    }

    #[test]
    fn scan_errors_are_actionable() {
        let mut app = DirustatApp::new();
        app.start_scan();
        assert_eq!(app.state().scan, ScanStatus::Failed { message: EMPTY_PATH_MESSAGE.to_owned() });
        assert!(app.focus_path, "an empty path moves focus to the path field");
        let ctx = egui::Context::default();
        let settle = |app: &mut DirustatApp| {
            let deadline = Instant::now() + Duration::from_secs(20);
            while matches!(app.state().scan, ScanStatus::Running { .. }) && Instant::now() < deadline {
                app.poll_scan(&ctx);
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        app.state_mut().path_text = "E:/nonexistent_zzz_dirustat".to_owned();
        app.start_scan();
        settle(&mut app);
        assert_eq!(
            app.state().scan,
            ScanStatus::Failed { message: "Folder not found: E:/nonexistent_zzz_dirustat".to_owned() }
        );
        let Ok(dir) = tempfile::tempdir() else { return };
        let file = dir.path().join("f.txt");
        assert!(std::fs::write(&file, b"x").is_ok());
        app.state_mut().path_text = file.to_string_lossy().into_owned();
        app.start_scan();
        settle(&mut app);
        assert!(matches!(&app.state().scan, ScanStatus::Failed { message } if message.starts_with("Not a folder: ")));
        assert_eq!(strip_prefix("invalid params: x"), "x");
        assert_eq!(friendly_scan_error("p", "invalid params: path is empty"), EMPTY_PATH_MESSAGE);
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

    fn demo_app() -> Option<DirustatApp> {
        Some(DirustatApp::with_session(crate::demo::demo_session().ok()?))
    }

    #[test]
    fn menus_come_from_the_command_registry() {
        let app = DirustatApp::new();
        let infos = app.session().commands();
        let groups = app.menu_groups(&infos);
        let names: Vec<&str> = groups.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.starts_with(&["File"]) && names.contains(&"View"), "{names:?}");
        let all: Vec<&MenuItem> = groups.iter().flat_map(|(_, i)| i).collect();
        let scan = all.iter().find(|i| i.id == "scan.start");
        assert!(scan.is_some_and(|i| i.enabled && i.shortcut == Some("Ctrl+O")));
        assert!(all.iter().any(|i| i.id == "scan.cancel" && !i.enabled));
        assert!(all.iter().any(|i| i.id == "ui.quit"));
        assert!(all.iter().any(|i| i.id == "view.size_mode"));
        let entries = DirustatApp::palette_entries(&infos);
        assert!(entries.iter().any(|e| e.id == "ui.command_palette"));
        assert!(entries.iter().all(|e| e.id != "select.node"));
    }

    #[test]
    fn commands_dispatch_by_id() {
        let mut app = DirustatApp::new();
        assert!(app.run_command("ui.toggle_map").is_ok());
        assert!(!app.state().panels.map);
        assert!(app.run_command("view.size_mode").is_ok());
        assert_eq!(app.session().map_size_mode(), SizeMode::Logical);
        assert!(app.run_command("select.node").is_err());
        assert!(app.state().notice.is_some());
        assert!(app.run_command("no.such").is_err());
        assert!(app.run_command("scan.cancel").is_err());
        assert!(app.run_command("ui.quit").is_ok());
        assert!(app.quit_requested());
    }

    #[test]
    fn demo_frames_palette_and_screenshot_path() {
        let Some(mut app) = demo_app() else { return };
        assert!(app.session().tree().len() > 10);
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let input = || egui::RawInput { screen_rect: Some(screen), ..egui::RawInput::default() };
        assert!(app.run_command("ui.command_palette").is_ok());
        for _ in 0..4 {
            let out = ctx.run_ui(input(), |ui| app.show(ui));
            assert!(!out.shapes.is_empty());
            out.drop_without_applying_deltas();
        }
        assert!(app.state().palette_open);
        let Ok(dir) = tempfile::tempdir() else { return };
        let path = dir.path().join("shot.png");
        let ticket = app.request_screenshot(&path.to_string_lossy(), None);
        assert!(ticket.is_ok());
        let ticket = ticket.unwrap_or_default();
        let image = std::sync::Arc::new(egui::ColorImage::filled([64, 48], egui::Color32::WHITE));
        let mut shot = input();
        shot.events.push(egui::Event::Screenshot {
            viewport_id: egui::ViewportId::ROOT,
            user_data: egui::UserData::new(ticket),
            image,
        });
        let out = ctx.run_ui(shot, |ui| app.show(ui));
        out.drop_without_applying_deltas();
        assert!(matches!(app.screenshot_status(ticket), Some(ScreenshotStatus::Done { width: 64, .. })));
        assert!(path.is_file());
    }

    #[test]
    fn ui_state_json_and_patch() {
        let mut app = DirustatApp::new();
        assert_eq!(app.ui_state_json()["panels"]["tree"], true);
        assert!(app.apply_ui_patch(&json!({"panels": {"tree": false}})).is_ok());
        assert_eq!(app.ui_state_json()["panels"]["tree"], false);
        assert!(app.apply_ui_patch(&json!({"bogus": 1})).is_err());
    }

    #[test]
    fn shortcuts_run_commands() {
        let mut app = DirustatApp::new();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Num0,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        app.state_mut().zoom_percent = 150;
        let out = ctx.run_ui(input, |ui| app.show(ui));
        out.drop_without_applying_deltas();
        assert_eq!(app.state().zoom_percent, 100);
    }

    #[test]
    fn elevation_hint_is_set_only_for_denied_volume_roots() {
        let denied = |reason: &'static str| dirustat_engine::FastPathStatus {
            volume: "C:".into(),
            available: false,
            reason,
            needs_admin: reason == "access_denied",
        };
        let mut s = UiState::default();
        s.set_elevation_hint(Some("C:"), Some(&denied("access_denied")));
        assert_eq!(s.elevation_hint.as_deref(), Some("C:"));
        s.set_elevation_hint(Some("C:"), Some(&denied("not_ntfs")));
        assert_eq!(s.elevation_hint, None);
        s.set_elevation_hint(Some("C:"), Some(&denied("access_denied")));
        s.set_elevation_hint(None, Some(&denied("access_denied")));
        assert_eq!(s.elevation_hint, None);
        s.set_elevation_hint(Some("C:"), None);
        assert_eq!(s.elevation_hint, None);
    }

    #[test]
    fn non_root_scans_clear_the_hint_and_frames_show_it() {
        let mut app = DirustatApp::new();
        app.state_mut().elevation_hint = Some("C:".to_owned());
        app.state_mut().path_text = "definitely/not/a/volume/root".to_owned();
        app.refresh_elevation_hint(true);
        assert_eq!(app.state().elevation_hint, None);
        // Preference off: no probe, no hint, even for a volume root.
        app.state_mut().path_text = "C:\\".to_owned();
        app.refresh_elevation_hint(false);
        assert_eq!(app.state().elevation_hint, None);
        // A frame with the hint and the help window open builds in both themes.
        let ctx = egui::Context::default();
        app.state_mut().elevation_hint = Some("C:".to_owned());
        assert!(app.run_command("ui.relaunch_elevated_help").is_ok());
        assert!(app.state().elevation_help_open);
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            app.state_mut().theme = theme;
            assert!(frame(&mut app, &ctx) > 0);
        }
        let json = app.ui_state_json();
        assert_eq!(json["elevation_hint"], "C:");
        assert_eq!(json["elevation_help_open"], true);
        assert!(app.apply_ui_patch(&json!({"elevation_hint": "D:"})).is_err());
    }

    #[test]
    fn volume_probe_runs_off_the_ui_thread_and_is_polled() {
        let mut app = DirustatApp::new();
        let ctx = egui::Context::default();
        app.state_mut().elevation_hint = Some("stale".to_owned());
        app.state_mut().path_text = "C:\\".to_owned();
        app.refresh_elevation_hint(true);
        // The stale hint is cleared at once and a probe is pending (no blocking `Volume::open` here).
        assert_eq!(app.state().elevation_hint, None);
        assert!(app.probe.is_some());
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while app.probe.is_some() {
            app.poll_probe(&ctx);
            assert!(std::time::Instant::now() < deadline, "probe never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Whatever the machine says, the hint is either unset or names the probed volume.
        assert!(matches!(app.state().elevation_hint.as_deref(), None | Some("C:")));
        // A newer refresh for a non-volume path drops a pending probe.
        app.refresh_elevation_hint(true);
        app.state_mut().path_text = "folder".to_owned();
        app.refresh_elevation_hint(true);
        assert!(app.probe.is_none());
    }

    #[test]
    fn side_panels_leave_the_treemap_a_minimum_share() {
        for total in [0.0, 100.0, 330.0, 667.0, 700.0, 1000.0, 1200.0, 2560.0, f32::NAN, f32::INFINITY] {
            let l = SideLayout::for_width(total, true);
            let t = if total.is_finite() { total } else { 0.0 };
            let ext = if l.show_ext { l.ext_max } else { 0.0 };
            let map = t - l.tree_max - ext;
            assert!(map >= (t * 0.4).min(t) - 0.01, "treemap starved at {total}: {map}");
        }
        assert!(!SideLayout::for_width(667.0, true).show_ext, "zoom 150 at 1000 px collapses the file types");
        assert!(SideLayout::for_width(1200.0, true).show_ext);
        assert!(SideLayout::for_width(1200.0, false).tree_max >= 1200.0 * 0.6 - 0.01);
    }

    #[test]
    fn narrow_window_collapses_file_types_and_says_so() {
        let Some(mut app) = demo_app() else { return };
        let ctx = egui::Context::default();
        let size = |w: f32, h: f32| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
            ..egui::RawInput::default()
        };
        for (w, collapsed) in [(1400.0, false), (500.0, true), (1400.0, false)] {
            for _ in 0..3 {
                let out = ctx.run_ui(size(w, 700.0), |ui| app.show(ui));
                out.drop_without_applying_deltas();
            }
            assert_eq!(app.ext_collapsed, collapsed, "width {w}");
        }
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert!(human_size(u64::MAX).ends_with("PiB"));
    }
}
