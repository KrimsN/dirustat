//! The application shell: menu bar, top bar, status bar and the central panels.
//!
//! Every action goes through a command: menus are built from `Session::commands()` plus the
//! shell-only `UI_COMMANDS`, buttons and shortcuts dispatch by id (`DirustatApp::run_command`).

use crate::commands::{self, Needs, UI_COMMANDS, UiEffect};
use crate::delete_dialog::{self, DeleteMode, DialogEvent, DialogRequest};
use crate::palette::{self, PaletteEntry};
use crate::screenshot::{Region, SaveOptions, ScreenshotStatus, Screenshots};
use crate::state::{ScanStatus, UiState};
use crate::theme::{ThemeKind, Tokens};
use crate::{cleanups_ui, drive_picker, ext_panel, journal_ui, map_panel, prefs, search_panel, toast, tree_panel};
use dirustat_control::{SharedSession, lock_session, share};
use dirustat_engine::{CommandError, CommandInfo, FastPathStatus, Session, execute_unlocked};
use dirustat_model::SizeMode;
use dirustat_platform::{PlatformError, PlatformServices};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

/// How often the UI repaints while a scan runs.
const SCAN_REPAINT: Duration = Duration::from_millis(50);

/// Top-level menus in display order; other groups follow in registration order.
const MENU_ORDER: [&str; 6] = ["File", "Edit", "View", "Tools", "Cleanup", "Help"];

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
    /// Text for the clipboard, handed to egui on the next frame (`ui.copy_path`).
    pending_copy: Option<String>,
    /// Native services used for the folder dialog (replaced by a fake in tests).
    picker: Arc<dyn PlatformServices>,
    /// Answer of the folder dialog running on a helper thread (the dialog blocks its thread).
    browse: Option<Receiver<Result<Option<PathBuf>, PlatformError>>>,
    /// The start screen asked for the folder dialog; started once the session lock is released.
    browse_requested: bool,
    /// Answer of the confirmed delete-dialog command running on a worker thread (G-062).
    command_job: Option<CommandJob>,
    /// Scan root and pending answer of `platform.free_space`, asked on a worker thread.
    free_job: Option<(String, CommandJob)>,
}

/// The answer of a command running on a worker thread.
type CommandJob = Receiver<Result<Value, CommandError>>;

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

/// Status text when a command needs a selected node and there is none.
const SELECT_FIRST_MESSAGE: &str = "Select a file or folder first";

/// Status text when the window is asked to close while a deletion runs (closing would cut it off
/// half way).
const WAIT_TO_QUIT_MESSAGE: &str = "A deletion is still running; close the window when it has finished";

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
    /// Tooltip of a disabled item that says why (for example a network scan root).
    disabled_reason: Option<&'static str>,
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
            pending_copy: None,
            picker: dirustat_platform::default_platform(),
            browse: None,
            browse_requested: false,
            command_job: None,
            free_job: None,
        }
    }

    /// Replaces the services behind the folder dialog (tests use a fake that never opens a window).
    pub fn set_folder_picker(&mut self, picker: Arc<dyn PlatformServices>) {
        self.picker = picker;
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
        let delete_mode = match id {
            "file.delete" => Some(DeleteMode::Recycle),
            "file.delete_permanent" => Some(DeleteMode::Permanent),
            _ => None,
        };
        if let Some(mode) = delete_mode {
            // Delete from the menu, a shortcut or the palette only opens the confirmation dialog
            // (after a read-only preview); the destructive command is sent by its confirm button.
            let mut session = lock_session(&self.session);
            delete_dialog::open_for_selection(&mut session, &mut self.state, mode)?;
            return Ok(json!({ "ok": true, "dialog": "open" }));
        }
        if let Some(effect) = commands::run(&mut self.state, id) {
            match effect {
                UiEffect::Quit if self.deletion_running() => return Err(WAIT_TO_QUIT_MESSAGE.to_owned()),
                UiEffect::Quit => self.quit_requested = true,
                UiEffect::CopyPath => self.copy_selected_path()?,
                UiEffect::BrowseFolder => self.start_browse()?,
                UiEffect::None => {}
            }
            return Ok(json!({ "ok": true }));
        }
        let mut session = lock_session(&self.session);
        let infos = session.commands();
        let params = self.default_params(&session, id, &infos).ok_or_else(|| {
            if commands::engine_menu(id).is_some_and(|e| e.needs == Needs::Selection) {
                SELECT_FIRST_MESSAGE.to_owned()
            } else {
                format!("{id} needs parameters")
            }
        })?;
        let value = session.execute(id, &params).map_err(|e| e.to_string())?;
        let prefer_fast = session.settings().prefer_fast_path;
        drop(session);
        if id == "scan.start" {
            self.state.scan = ScanStatus::Running { dirs: 0, files: 0, bytes: 0, errors: 0 };
            self.state.free_space = None;
            self.state.free_key = None;
            self.refresh_elevation_hint(prefer_fast);
        } else if id == "search.clear" {
            self.state.search.reset();
        }
        Ok(value)
    }

    /// Opens the native folder dialog on a helper thread; [`Self::poll_browse`] picks up the answer.
    /// Only the interactive entry points (menu, shortcut, button, palette) reach this; the control
    /// channel has no such command, so a programmatic call never opens a dialog.
    fn start_browse(&mut self) -> Result<(), String> {
        if self.browse.is_some() {
            return Ok(());
        }
        if lock_session(&self.session).is_scanning() {
            return Err("Stop the running scan first".to_owned());
        }
        let (tx, rx) = mpsc::channel();
        let picker = Arc::clone(&self.picker);
        std::thread::Builder::new()
            .name("folder-dialog".to_owned())
            .spawn(move || {
                // The receiver is gone when the window closed meanwhile; nothing to report then.
                let _ = tx.send(picker.pick_folder());
            })
            .map_err(|_| "Cannot open the folder dialog".to_owned())?;
        self.browse = Some(rx);
        Ok(())
    }

    /// Applies a finished folder dialog, if any, without blocking: a chosen folder is scanned.
    fn poll_browse(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.browse else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(SCAN_REPAINT);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(PlatformError::Failed {
                op: "pick_folder",
                message: "the dialog thread ended without an answer".to_owned(),
            }),
        };
        self.browse = None;
        ctx.request_repaint();
        match answer {
            Ok(Some(folder)) => {
                self.state.path_text = folder.to_string_lossy().into_owned();
                if lock_session(&self.session).is_scanning() {
                    self.state.notice = Some("A scan is running; the folder is in the path field".to_owned());
                } else {
                    self.start_scan();
                }
            }
            Ok(None) => {}
            Err(PlatformError::Unsupported(_)) => {
                self.state.notice = Some("The folder dialog is not available here; type the path".to_owned());
            }
            Err(err) => self.state.notice = Some(format!("Cannot open the folder dialog: {err}")),
        }
    }

    /// Queues the path of the selected node for the clipboard.
    fn copy_selected_path(&mut self) -> Result<(), String> {
        let session = lock_session(&self.session);
        let path = session.selection().and_then(|n| session.node_path(n)).ok_or(SELECT_FIRST_MESSAGE)?;
        drop(session);
        self.pending_copy = Some(path);
        Ok(())
    }

    /// Replaces the `scan.exclude` setting through `settings.set`; the error is shown in the editor.
    fn set_excludes(&mut self, list: &[String]) -> Result<(), String> {
        let mut session = lock_session(&self.session);
        session
            .execute("settings.set", &json!({ "key": "scan.exclude", "value": list }))
            .map(|_| ())
            .map_err(|e| strip_prefix(&e.to_string()))
    }

    /// Applies one action of the exclusions editor.
    pub fn apply_prefs_action(&mut self, action: prefs::PrefsAction) {
        let current = lock_session(&self.session).settings().exclude.clone();
        let result = match action {
            prefs::PrefsAction::Add(text) => {
                prefs::with_pattern(&current, &text).and_then(|next| self.set_excludes(&next)).map(|()| {
                    self.state.exclude_input.clear();
                })
            }
            prefs::PrefsAction::Remove(i) => self.set_excludes(&prefs::without_pattern(&current, i)),
        };
        self.state.prefs_error = result.err();
    }

    /// Reloads the drive list when it is stale and asks for the free space of the scanned volume
    /// on a worker thread (a slow or hung drive must not stall the frame); [`Self::poll_free_space`]
    /// picks up the answer.
    fn refresh_volumes(&mut self) {
        let mut session = lock_session(&self.session);
        if !self.state.drives_loaded {
            self.state.drives = drive_picker::load(&mut session);
            self.state.drives_loaded = true;
        }
        if !matches!(self.state.scan, ScanStatus::Done { .. }) {
            return;
        }
        let key = (self.state.path_text.trim().to_owned(), session.revision());
        drop(session);
        if self.state.free_key.as_ref() == Some(&key) {
            return;
        }
        self.state.free_space = None;
        let job = self.spawn_command("platform.free_space", json!({ "path": key.0 }));
        self.free_job = Some((key.0.clone(), job));
        self.state.free_key = Some(key);
        // The fill bars of the drive list changed with the scan.
        self.state.drives_loaded = false;
    }

    /// Applies a finished free-space query, if any, without blocking.
    fn poll_free_space(&mut self, ctx: &egui::Context) {
        let Some((path, rx)) = &self.free_job else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer.ok(),
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(SCAN_REPAINT);
                return;
            }
            Err(TryRecvError::Disconnected) => None,
        };
        self.state.free_space = answer.and_then(|v| {
            Some(crate::state::FreeSpace {
                path: path.clone(),
                total: v.get("total")?.as_u64()?,
                free: v.get("free")?.as_u64()?,
            })
        });
        self.free_job = None;
        ctx.request_repaint();
    }

    /// Runs `id` on a worker thread through `execute_unlocked`: the session is locked only for the
    /// command's checks and bookkeeping, never for its slow part. The answer arrives on the returned
    /// channel. If no thread can be started the command runs here instead.
    fn spawn_command(&self, id: &'static str, params: Value) -> CommandJob {
        let (tx, rx) = mpsc::channel();
        let fallback = tx.clone();
        let shared = SharedSession::clone(&self.session);
        let worker_params = params.clone();
        let spawned = std::thread::Builder::new().name("command-worker".to_owned()).spawn(move || {
            // The receiver is gone when the window closed or a newer request replaced this one.
            let _ = tx.send(execute_unlocked(|| lock_session(&shared), id, &worker_params));
        });
        if spawned.is_err() {
            let _ = fallback.send(execute_unlocked(|| lock_session(&self.session), id, &params));
        }
        rx
    }

    /// Sends the confirmed command of the delete dialog to a worker thread. UI-only: the confirm
    /// button is the only caller.
    fn confirm_delete(&mut self) {
        if self.command_job.is_some() {
            return;
        }
        if let Some((command, params)) = delete_dialog::begin_confirm(&mut self.state) {
            self.command_job = Some(self.spawn_command(command, params));
        }
    }

    /// Hands a finished delete-dialog command to the dialog, if any, without blocking.
    fn poll_command_job(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.command_job else { return };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(SCAN_REPAINT);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(CommandError::Platform(
                "the command stopped without an answer; scan again to see what is left".to_owned(),
            )),
        };
        self.command_job = None;
        delete_dialog::finish_confirm(&mut self.state, result);
        ctx.request_repaint();
    }

    /// True while a confirmed delete or cleanup is running on its worker thread.
    pub fn command_running(&self) -> bool {
        self.command_job.is_some()
    }

    /// True while a deletion runs, started here or by a control client: ending the process now
    /// would cut it off half way.
    fn deletion_running(&self) -> bool {
        self.command_running() || lock_session(&self.session).removal_in_progress()
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
            "treemap.cushion" => Some(json!({})),
            "platform.reveal" | "platform.properties" => session.selection().map(|n| json!({ "id": n.index() })),
            _ => {
                let info = infos.iter().find(|i| i.id == id)?;
                info.params_doc.trim_start().starts_with("none").then(|| json!({}))
            }
        }
    }

    /// Menu path of an engine command: its own, or the one the shell assigns.
    fn menu_of(info: &CommandInfo) -> &'static str {
        if info.menu.is_empty() { commands::engine_menu(info.id).map_or("", |e| e.menu) } else { info.menu }
    }

    /// Shortcut of an engine command: its own, or the one the shell assigns.
    fn shortcut_of(info: &CommandInfo) -> Option<&'static str> {
        info.shortcut.or_else(|| commands::engine_menu(info.id).and_then(|e| e.shortcut))
    }

    /// Whether a command can run without a parameter form.
    fn runnable_without_form(info: &CommandInfo) -> bool {
        matches!(
            info.id,
            "scan.start"
                | "view.size_mode"
                | "treemap.cushion"
                | "platform.reveal"
                | "platform.properties"
                | "file.delete"
                | "file.delete_permanent"
        ) || info.params_doc.trim_start().starts_with("none")
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
        self.poll_browse(&ctx);
        self.poll_command_job(&ctx);
        self.poll_free_space(&ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.deletion_running() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.state.notice = Some(WAIT_TO_QUIT_MESSAGE.to_owned());
        }
        if let Some(wait) = self.state.tick_notice(std::time::Instant::now()) {
            ctx.request_repaint_after(wait);
        }
        self.shots.pump(&ctx);
        self.refresh_volumes();
        if let Some(text) = self.pending_copy.take() {
            ctx.copy_text(text);
        }
        let infos = lock_session(&self.session).commands();
        self.handle_dialog_request();
        self.handle_cleanup_request();
        self.handle_shortcuts(&ctx, &infos);
        if let Some(id) = self.menu_bar(ui, &infos) {
            let _ = self.run_command(&id);
        }
        self.top_bar(ui);
        self.search_bar(ui, &ctx);
        if self.state.panels.status_bar {
            self.status_bar(ui);
        }
        self.search_results(ui);
        if let Some(path) = self.central(ui) {
            self.state.path_text = path;
            self.start_scan();
        }
        if std::mem::take(&mut self.browse_requested) {
            let _ = self.run_command("ui.browse_folder");
        }
        self.prefs_window(&ctx);
        self.elevation_help_window(&ctx);
        self.cleanups_window(&ctx);
        self.journal_window(&ctx);
        self.delete_dialog_window(&ctx);
        toast::show(&ctx, &mut self.state, &self.tokens);
        let entries = Self::palette_entries(&infos);
        if let Some(id) = palette::show(&ctx, &mut self.state, &self.tokens, &entries) {
            let _ = self.run_command(&id);
        }
        if self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The search bar under the top bar; runs the search when it is due.
    fn search_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let wait = {
            let mut session = lock_session(&self.session);
            self.state.search.update(&mut session, std::time::Instant::now())
        };
        if let Some(wait) = wait {
            ctx.request_repaint_after(wait);
        }
        if !self.state.search.open {
            return;
        }
        let mut actions = search_panel::BarActions::default();
        egui::Panel::top("search_bar").show(ui, |ui| {
            actions = search_panel::show_bar(ui, &mut self.state.search, &self.tokens);
        });
        if actions.clear {
            let _ = self.run_command("search.clear");
        }
        if actions.close {
            self.state.search.open = false;
        }
    }

    /// The result list above the status bar; a click selects the hit in the tree and the treemap.
    fn search_results(&mut self, ui: &mut egui::Ui) {
        let search = &self.state.search;
        if !search.open || !search.searched {
            return;
        }
        let selected = lock_session(&self.session).selection().map(|n| n.index());
        let mut clicked = None;
        egui::Panel::bottom("search_results").resizable(true).default_size(150.0).size_range(60.0..=400.0).show(
            ui,
            |ui| {
                clicked = search_panel::show_results(ui, &self.state.search, selected, &self.tokens);
            },
        );
        if let Some(id) = clicked {
            let _ = lock_session(&self.session).execute("select.node", &json!({ "id": id }));
        }
    }

    /// The preferences window with the exclusions editor.
    fn prefs_window(&mut self, ctx: &egui::Context) {
        if !self.state.prefs_open {
            return;
        }
        let exclude = lock_session(&self.session).settings().exclude.clone();
        let mut open = true;
        let action = prefs::show(
            ctx,
            &mut open,
            &mut self.state.exclude_input,
            self.state.prefs_error.as_deref(),
            &exclude,
            &self.tokens,
        );
        self.state.prefs_open = open;
        if let Some(action) = action {
            self.apply_prefs_action(action);
        }
    }

    /// Executes a `delete_dialog` request of `ui.set_state`: the same path as the Delete key, so
    /// it can open the dialog (after the read-only preview) or close it, and never confirm it.
    fn handle_dialog_request(&mut self) {
        let Some(request) = self.state.dialog_request.take() else { return };
        let _ = match request {
            DialogRequest::Recycle => self.run_command("file.delete"),
            DialogRequest::Permanent => self.run_command("file.delete_permanent"),
            // A running dialog waits for its answer; closing it would lose the outcome.
            DialogRequest::Close if self.command_running() => {
                self.state.notice = Some("The dialog closes when the deletion has finished".to_owned());
                Ok(Value::Null)
            }
            DialogRequest::Close => {
                self.state.delete_dialog = None;
                Ok(Value::Null)
            }
        };
    }

    /// Executes a `cleanups_preview` request of `ui.set_state`: opens the window on that cleanup and
    /// runs the read-only preview (what the Select and Preview buttons do). Never runs a cleanup.
    fn handle_cleanup_request(&mut self) {
        let Some(id) = self.state.cleanup_request.take() else { return };
        self.state.cleanups.open = true;
        let mut session = lock_session(&self.session);
        if cleanups_ui::needs_reload(&session, &self.state.cleanups) {
            cleanups_ui::load(&mut session, &mut self.state);
        }
        self.state.cleanups.selected = Some(id.clone());
        // A refusal is shown inline in the window (QA-129), not as a toast.
        let _ = cleanups_ui::preview(&mut session, &mut self.state, &id);
    }

    /// Restarts the scan of the current scan root (after a stale-tree refusal).
    fn rescan(&mut self) {
        let root = {
            let session = lock_session(&self.session);
            session.node_path(session.tree().root())
        };
        if let Some(root) = root {
            self.state.path_text = root;
        }
        self.start_scan();
    }

    /// The modal delete confirmation. The destructive command leaves only from the confirm click.
    fn delete_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.state.delete_dialog.take() else { return };
        let event = delete_dialog::show(ctx, &mut dialog, &self.tokens);
        self.state.delete_dialog = Some(dialog);
        match event {
            DialogEvent::None => {}
            DialogEvent::Cancel => self.state.delete_dialog = None,
            DialogEvent::Confirm => {
                self.confirm_delete();
                ctx.request_repaint();
            }
            DialogEvent::Rescan => {
                self.state.delete_dialog = None;
                self.rescan();
            }
            DialogEvent::SwitchToPermanent => {
                let mut session = lock_session(&self.session);
                delete_dialog::switch_to_permanent(&mut session, &mut self.state);
            }
        }
    }

    /// Tools > Cleanups.
    fn cleanups_window(&mut self, ctx: &egui::Context) {
        if !self.state.cleanups.open {
            // Reopening reads the saved cleanups again (QA-130).
            self.state.cleanups.loaded = false;
            return;
        }
        let mut session = lock_session(&self.session);
        if cleanups_ui::needs_reload(&session, &self.state.cleanups) {
            cleanups_ui::load(&mut session, &mut self.state);
        }
        let block = delete_dialog::block_reason(&session);
        let revision = session.revision();
        let action = cleanups_ui::show(ctx, &mut self.state.cleanups, &self.tokens, block, revision);
        if let Some(action) = action {
            Self::apply_cleanups_action(&mut self.state, &mut session, action);
            ctx.request_repaint();
        }
    }

    fn apply_cleanups_action(state: &mut UiState, session: &mut Session, action: cleanups_ui::CleanupsAction) {
        use cleanups_ui::CleanupsAction as A;
        let result = match action {
            A::Reload => {
                state.cleanups.loaded = false;
                Ok(())
            }
            A::Select(id) => {
                state.cleanups.selected = Some(id);
                state.cleanups.preview = None;
                state.cleanups.confirm_remove = None;
                Ok(())
            }
            A::Preview(id) => {
                // The refusal is shown inline in the window.
                let _ = cleanups_ui::preview(session, state, &id);
                Ok(())
            }
            A::Run(id) => cleanups_ui::open_run_dialog(session, state, &id),
            A::DryRun(id) => cleanups_ui::dry_run(session, state, &id),
            A::New => {
                state.cleanups.form = Some(cleanups_ui::CleanupForm::default());
                Ok(())
            }
            A::Edit(id) => {
                state.cleanups.form =
                    state.cleanups.entries.iter().find(|e| e.id == id && !e.builtin).map(cleanups_ui::form_from_entry);
                Ok(())
            }
            A::AskRemove(id) => {
                state.cleanups.confirm_remove = Some(id);
                Ok(())
            }
            A::Remove(id) => {
                cleanups_ui::remove(session, state, &id);
                Ok(())
            }
            A::CancelRemove => {
                state.cleanups.confirm_remove = None;
                Ok(())
            }
            A::Save => {
                cleanups_ui::save_form(session, state);
                Ok(())
            }
            A::CancelEdit => {
                state.cleanups.form = None;
                Ok(())
            }
        };
        if let Err(message) = result {
            toast::push(state, toast::ToastKind::Error, message);
        }
    }

    /// Tools > Journal.
    fn journal_window(&mut self, ctx: &egui::Context) {
        if !self.state.journal.open {
            return;
        }
        if !self.state.journal.loaded {
            journal_ui::load(&mut lock_session(&self.session), &mut self.state.journal);
        }
        match journal_ui::show(ctx, &mut self.state.journal, &self.tokens) {
            Some(journal_ui::JournalAction::Refresh) => self.state.journal.loaded = false,
            Some(journal_ui::JournalAction::Previous) => self.state.journal.turn(false),
            Some(journal_ui::JournalAction::Next) => self.state.journal.turn(true),
            None => {}
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
        // A modal dialog owns the keyboard: no shortcut (Delete included) acts behind it.
        if self.state.delete_dialog.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let mut hits: Vec<(String, egui::KeyboardShortcut)> = Vec::new();
        for i in infos.iter().filter(|i| i.enabled && Self::runnable_without_form(i)) {
            if let Some(sc) = Self::shortcut_of(i).and_then(commands::parse_shortcut) {
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
            menu: Self::menu_of(i).to_owned(),
            shortcut: Self::shortcut_of(i).map(str::to_owned),
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
        let (has_selection, has_hits, delete_block, cushion_on) = {
            let session = lock_session(&self.session);
            let selection = session.selection();
            let block = match selection {
                Some(node) => delete_dialog::node_block_reason(&session, node),
                None => delete_dialog::block_reason(&session),
            };
            (selection.is_some(), !session.search_hits().is_empty(), block, session.settings().treemap.cushion)
        };
        for i in infos.iter().filter(|i| !Self::menu_of(i).is_empty() && Self::runnable_without_form(i)) {
            let menu = Self::menu_of(i);
            let label = match i.id {
                "file.delete" => "Move to Recycle Bin".to_owned(),
                "file.delete_permanent" => "Delete permanently...".to_owned(),
                _ => menu.split_once('/').map_or(i.label, |(_, l)| l).to_owned(),
            };
            let needs = commands::engine_menu(i.id).map_or(Needs::Nothing, |e| e.needs);
            let satisfied = match needs {
                Needs::Nothing => true,
                Needs::Selection => has_selection,
                Needs::SearchHits => has_hits,
                Needs::Deletable => has_selection && delete_block.is_none(),
            };
            let item = MenuItem {
                id: i.id.to_owned(),
                label,
                shortcut: Self::shortcut_of(i),
                enabled: i.enabled && satisfied,
                checked: i.id == "treemap.cushion" && cushion_on,
                disabled_reason: if needs == Needs::Deletable && has_selection { delete_block } else { None },
            };
            add(menu, item);
        }
        for c in UI_COMMANDS.iter().filter(|c| !c.menu.is_empty()) {
            let label = c.menu.split_once('/').map_or(c.label, |(_, l)| l).to_owned();
            let checked = c.checked.is_some_and(|f| f(&self.state));
            add(
                c.menu,
                MenuItem {
                    id: c.id.to_owned(),
                    label,
                    shortcut: c.shortcut,
                    enabled: true,
                    checked,
                    disabled_reason: None,
                },
            );
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
                            let mut response = ui.add_enabled(item.enabled, button);
                            if let Some(reason) = item.disabled_reason {
                                response = response.on_disabled_hover_text(reason);
                            }
                            if response.clicked() {
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
        let mut picked: Option<String> = None;
        let size_label = match mode {
            SizeMode::Allocated => "Size: allocated",
            SizeMode::Logical => "Size: logical",
        };
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Path:");
                let width = (ui.available_width() - 340.0).max(80.0);
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
                } else {
                    if ui.button("Browse…").on_hover_text("Choose the folder to scan in a dialog").clicked() {
                        run = Some("ui.browse_folder");
                    }
                    if ui.button("Scan").clicked() || enter {
                        run = Some("scan.start");
                    }
                }
                if ui.button(size_label).on_hover_text("Toggle logical / allocated sizes").clicked() {
                    run = Some("view.size_mode");
                }
                ui.add_enabled_ui(!scanning, |ui| {
                    ui.menu_button("Drives", |ui| {
                        if let Some(root) = drive_picker::show_list(ui, &self.state.drives, &self.tokens) {
                            picked = Some(root);
                            ui.close();
                        }
                    });
                });
            });
        });
        if let Some(root) = picked {
            self.state.path_text = root;
            run = Some("scan.start");
        }
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
                    ui.colored_label(muted, format!("previous scan: {}", node_count_text(tree.len())));
                    ui.separator();
                    ui.colored_label(muted, human_size(total));
                } else {
                    ui.label(node_count_text(tree.len()));
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
                if let Some(space) = &self.state.free_space {
                    ui.separator();
                    ui.label(format!("Free: {} of {}", human_size(space.free), human_size(space.total)))
                        .on_hover_text(format!("Volume of {}", space.path));
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

    /// Draws the panels; returns the root of a drive picked on the start screen.
    fn central(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let panels = self.state.panels;
        let mut picked = None;
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
        let mut browse = false;
        let nothing_to_show = session.tree().len() <= 1
            && matches!(self.state.scan, ScanStatus::Idle | ScanStatus::Cancelled | ScanStatus::Failed { .. });
        egui::CentralPanel::default().show(ui, |ui| {
            if nothing_to_show {
                match drive_picker::show_start_screen(ui, &self.state.drives, &self.tokens) {
                    Some(drive_picker::StartAction::Drive(root)) => picked = Some(root),
                    Some(drive_picker::StartAction::Browse) => browse = true,
                    None => {}
                }
            } else if panels.map {
                let size = ui.available_size();
                map_area = size.x.max(0.0) * size.y.max(0.0);
                map_panel::show(ui, &mut session, &mut self.state);
            }
        });
        self.map_area_px = map_area;
        self.browse_requested |= browse;
        if matches!(self.state.scan, ScanStatus::Running { .. }) && session.tree().len() > 1 {
            stale_overlay(ui, area, &self.tokens);
        }
        picked
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
    if unit == 0 {
        return format!("{whole} B");
    }
    // Round to the nearest tenth (47_998_000 B is 45.8 MiB, not 45.7); a carry may reach the next unit.
    let mut tenth = rem.saturating_mul(10).saturating_add(512) / 1024;
    if tenth >= 10 {
        whole = whole.saturating_add(1);
        tenth = 0;
        if whole >= 1024 && unit < UNITS.len().saturating_sub(1) {
            whole = 1;
            unit = unit.saturating_add(1);
        }
    }
    let name = UNITS.get(unit).copied().unwrap_or("B");
    format!("{whole}.{tenth} {name}")
}

/// "1 node" / "N nodes".
fn node_count_text(n: usize) -> String {
    if n == 1 { "1 node".to_owned() } else { format!("{n} nodes") }
}

#[cfg(test)]
mod flow_tests;

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
        // The scan root is normalized (QA-112), so the separator is platform-specific.
        assert!(matches!(
            &app.state().scan,
            ScanStatus::Failed { message }
                if message.starts_with("Folder not found: ") && message.ends_with("nonexistent_zzz_dirustat")
        ));
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
    fn cushion_menu_item_is_checked_and_toggles_the_setting() {
        let mut app = DirustatApp::new();
        let item_checked = |app: &DirustatApp| {
            let infos = app.session().commands();
            let groups = app.menu_groups(&infos);
            let found = groups.iter().flat_map(|(_, i)| i).find(|i| i.id == "treemap.cushion");
            found.map(|i| (i.checked, i.enabled))
        };
        assert_eq!(item_checked(&app), Some((true, true)), "on by default, listed in the View menu");
        let infos = app.session().commands();
        assert!(DirustatApp::palette_entries(&infos).iter().any(|e| e.id == "treemap.cushion"));
        assert!(app.run_command("treemap.cushion").is_ok());
        assert!(!app.session().settings().treemap.cushion);
        assert_eq!(item_checked(&app), Some((false, true)));
        assert!(app.run_command("treemap.cushion").is_ok());
        assert_eq!(item_checked(&app), Some((true, true)));
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

    fn app_with_fake() -> (DirustatApp, std::sync::Arc<dirustat_platform::FakePlatform>) {
        let fake = std::sync::Arc::new(dirustat_platform::FakePlatform::new());
        let mut session = Session::new();
        session.set_platform(fake.clone());
        (DirustatApp::with_session(session), fake)
    }

    /// Scans `dir` through frames until the scan is done.
    fn scan_dir(app: &mut DirustatApp, ctx: &egui::Context, dir: &std::path::Path) {
        app.state_mut().path_text = dir.to_string_lossy().into_owned();
        app.start_scan();
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.is_scanning() && Instant::now() < deadline {
            let _ = frame(app, ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = frame(app, ctx);
        let _ = frame(app, ctx);
    }

    fn fixture_dir() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().ok()?;
        std::fs::create_dir(dir.path().join("docs")).ok()?;
        std::fs::write(dir.path().join("docs").join("report.txt"), vec![0u8; 3000]).ok()?;
        std::fs::write(dir.path().join("big.bin"), vec![0u8; 9000]).ok()?;
        std::fs::write(dir.path().join("small.bin"), vec![0u8; 10]).ok()?;
        Some(dir)
    }

    /// Runs frames until the folder dialog thread has answered.
    fn wait_browse(app: &mut DirustatApp, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.browse.is_some() && Instant::now() < deadline {
            let _ = frame(app, ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn browse_command_scans_the_chosen_folder_without_typing_a_path() {
        let Some(dir) = fixture_dir() else { return };
        let (mut app, fake) = app_with_fake();
        app.set_folder_picker(fake.clone());
        fake.set_picked_folder(Some(dir.path().to_path_buf()));
        let ctx = egui::Context::default();
        assert!(app.state().path_text.is_empty());
        assert!(app.run_command("ui.browse_folder").is_ok());
        wait_browse(&mut app, &ctx);
        assert_eq!(app.state().path_text, dir.path().to_string_lossy());
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.is_scanning() && Instant::now() < deadline {
            let _ = frame(&mut app, &ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = frame(&mut app, &ctx);
        assert!(matches!(app.state().scan, ScanStatus::Done { .. }), "{:?}", app.state().scan);
        assert!(fake.calls().contains(&dirustat_platform::PlatformCall::PickFolder));
    }

    #[test]
    fn cancelled_folder_dialog_changes_nothing() {
        let (mut app, fake) = app_with_fake();
        app.set_folder_picker(fake.clone());
        let ctx = egui::Context::default();
        app.state_mut().path_text = "keep-me".to_owned();
        assert!(app.run_command("ui.browse_folder").is_ok());
        wait_browse(&mut app, &ctx);
        assert_eq!(app.state().path_text, "keep-me");
        assert!(matches!(app.state().scan, ScanStatus::Idle));
        assert!(app.state().notice.is_none());
    }

    #[test]
    fn failing_folder_dialog_is_reported_not_fatal() {
        let (mut app, fake) = app_with_fake();
        app.set_folder_picker(fake.clone());
        fake.set_failure(Some(dirustat_platform::PlatformError::Unsupported("pick_folder")));
        let ctx = egui::Context::default();
        assert!(app.run_command("ui.browse_folder").is_ok());
        wait_browse(&mut app, &ctx);
        assert!(app.state().notice.as_deref().is_some_and(|n| n.contains("type the path")));
        assert!(app.browse.is_none(), "the dialog slot is free again");
    }

    #[test]
    fn browse_is_refused_while_a_scan_runs() {
        let Some(dir) = fixture_dir() else { return };
        let (mut app, fake) = app_with_fake();
        app.set_folder_picker(fake.clone());
        let ctx = egui::Context::default();
        scan_dir(&mut app, &ctx, dir.path());
        app.session().execute("scan.start", &json!({ "path": dir.path().to_string_lossy() })).ok();
        if app.is_scanning() {
            assert!(app.run_command("ui.browse_folder").is_err());
            assert!(app.browse.is_none());
        }
    }

    #[test]
    fn drive_picker_lists_fake_drives_and_refreshes() {
        use dirustat_platform::DriveInfo;
        let (mut app, fake) = app_with_fake();
        fake.set_drives(vec![DriveInfo { path: "Z:\\".into(), label: "Fake".into(), total: 1000, free: 250 }]);
        let ctx = egui::Context::default();
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            app.state_mut().theme = theme;
            assert!(frame(&mut app, &ctx) > 0, "the start screen builds");
        }
        assert_eq!(app.state().drives.len(), 1);
        assert_eq!(app.state().drives.first().map(|d| d.used_tenths()), Some(750));
        assert_eq!(app.ui_state_json()["drives"][0]["path"], "Z:\\");
        fake.set_drives(vec![]);
        assert!(app.run_command("ui.refresh_drives").is_ok());
        let _ = frame(&mut app, &ctx);
        assert!(app.state().drives.is_empty());
    }

    #[test]
    fn picking_a_drive_scans_its_root_and_free_space_reaches_the_status_bar() {
        let Some(dir) = fixture_dir() else { return };
        let (mut app, fake) = app_with_fake();
        fake.set_space(dirustat_platform::SpaceInfo { total: 1000, free: 250 });
        let ctx = egui::Context::default();
        // A pick sets the path and runs the same scan.start command as the Scan button.
        scan_dir(&mut app, &ctx, dir.path());
        assert_eq!(app.state().scan, ScanStatus::Done { errors: 0 });
        let space = app.state().free_space.clone();
        assert_eq!(space.map(|s| (s.total, s.free)), Some((1000, 250)));
        assert!(fake.calls().contains(&dirustat_platform::PlatformCall::FreeSpace(dir.path().to_path_buf())));
        // The reading is cached per scan root and revision, not repeated every frame.
        let before = fake.calls().len();
        let _ = frame(&mut app, &ctx);
        assert_eq!(fake.calls().len(), before, "no platform call per frame");
        // A new scan drops the old reading.
        app.start_scan();
        assert!(app.state().free_space.is_none());
    }

    #[test]
    fn search_flow_on_a_scanned_tree() {
        let Some(dir) = fixture_dir() else { return };
        let (mut app, _fake) = app_with_fake();
        let ctx = egui::Context::default();
        scan_dir(&mut app, &ctx, dir.path());
        assert!(app.run_command("ui.toggle_search").is_ok());
        assert!(app.state().search.open);
        app.state_mut().search.query = "bin".to_owned();
        app.state_mut().search.search_now();
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            app.state_mut().theme = theme;
            assert!(frame(&mut app, &ctx) > 0);
        }
        let names: Vec<String> = app.state().search.hits.iter().map(|h| h.name.clone()).collect();
        assert_eq!(names, ["big.bin", "small.bin"]);
        assert_eq!(app.session().search_hits().len(), 2);
        // Selecting a hit goes through select.node; the tree reveals it on the next frame.
        let first = app.state().search.hits.first().map(|h| h.id).unwrap_or_default();
        assert!(app.session().execute("select.node", &json!({ "id": first })).is_ok());
        let _ = frame(&mut app, &ctx);
        assert_eq!(app.session().selection().map(|n| n.index()), Some(first));
        // Clear: the engine command runs and the panel forgets the result and the criteria.
        assert!(app.run_command("search.clear").is_ok());
        assert!(app.session().search_hits().is_empty());
        assert!(app.state().search.hits.is_empty() && app.state().search.query.is_empty());
        let _ = frame(&mut app, &ctx);
        assert!(!app.state().search.searched);
    }

    #[test]
    fn invalid_regex_shows_a_message_and_a_frame_still_builds() {
        let Some(mut app) = demo_app() else { return };
        let ctx = egui::Context::default();
        let patch = json!({"search": {"open": true, "query": "(", "mode": "regex"}});
        assert!(app.apply_ui_patch(&patch).is_ok());
        app.state_mut().search.search_now();
        assert!(frame(&mut app, &ctx) > 0);
        let error = app.state().search.error.clone().unwrap_or_default();
        assert!(error.starts_with("Invalid regular expression"), "{error}");
        assert_eq!(app.ui_state_json()["search"]["error"], error.as_str());
        assert!(app.apply_ui_patch(&json!({"search": {"bogus": 1}})).is_err());
        assert!(app.apply_ui_patch(&json!({"search": {"mode": "fuzzy"}})).is_err());
    }

    #[test]
    fn edit_menu_has_the_search_and_platform_commands_with_enabling_rules() {
        let Some(mut app) = demo_app() else { return };
        let infos = app.session().commands();
        let find = |app: &DirustatApp, id: &str| {
            app.menu_groups(&infos).into_iter().flat_map(|(_, items)| items).find(|i| i.id == id)
        };
        let find_item = find(&app, "ui.toggle_search");
        assert!(find_item.is_some_and(|i| i.shortcut == Some("Ctrl+F") && i.enabled));
        assert!(find(&app, "search.clear").is_some_and(|i| !i.enabled), "nothing to clear yet");
        assert!(find(&app, "platform.reveal").is_some_and(|i| !i.enabled && i.shortcut == Some("Ctrl+Shift+R")));
        assert!(find(&app, "platform.properties").is_some_and(|i| !i.enabled));
        let err = app.run_command("platform.reveal").err().unwrap_or_default();
        assert_eq!(err, SELECT_FIRST_MESSAGE);
        let root = app.session().tree().root().index();
        assert!(app.session().execute("select.node", &json!({ "id": root })).is_ok());
        assert!(find(&app, "platform.reveal").is_some_and(|i| i.enabled));
        // The demo paths do not exist on disk, so the (fake) platform refuses them: the command was reached.
        assert!(app.run_command("platform.reveal").err().is_some_and(|m| m != SELECT_FIRST_MESSAGE));
        assert!(app.run_command("ui.copy_path").is_ok());
        assert!(app.pending_copy.is_some());
        let entries = DirustatApp::palette_entries(&infos);
        assert!(entries.iter().any(|e| e.id == "platform.properties" && e.menu == "Edit/Properties"));
    }

    #[test]
    fn exclusion_editor_changes_the_setting_and_reports_errors() {
        let (mut app, _fake) = app_with_fake();
        let ctx = egui::Context::default();
        assert!(app.run_command("ui.preferences").is_ok());
        for theme in [ThemeKind::Dark, ThemeKind::Light] {
            app.state_mut().theme = theme;
            assert!(frame(&mut app, &ctx) > 0);
        }
        app.apply_prefs_action(prefs::PrefsAction::Add(" node_modules ".into()));
        app.apply_prefs_action(prefs::PrefsAction::Add("*.tmp".into()));
        assert_eq!(app.session().settings().exclude, ["node_modules", "*.tmp"]);
        assert_eq!(app.state().prefs_error, None);
        app.apply_prefs_action(prefs::PrefsAction::Add("*.tmp".into()));
        assert!(app.state().prefs_error.is_some(), "duplicates are refused");
        app.apply_prefs_action(prefs::PrefsAction::Add("a\u{0}b".into()));
        assert!(app.state().prefs_error.is_some(), "the engine rejects hostile patterns");
        assert_eq!(app.session().settings().exclude.len(), 2);
        let _ = frame(&mut app, &ctx);
        app.apply_prefs_action(prefs::PrefsAction::Remove(0));
        app.apply_prefs_action(prefs::PrefsAction::Remove(99));
        assert_eq!(app.session().settings().exclude, ["*.tmp"]);
        assert_eq!(app.ui_state_json()["prefs_open"], true);
    }

    #[test]
    fn exclusions_apply_to_the_next_scan() {
        let Some(dir) = fixture_dir() else { return };
        let (mut app, _fake) = app_with_fake();
        let ctx = egui::Context::default();
        app.apply_prefs_action(prefs::PrefsAction::Add("*.bin".into()));
        scan_dir(&mut app, &ctx, dir.path());
        let session = app.session();
        let tree = session.tree();
        let names: Vec<String> =
            tree.children(tree.root()).filter_map(|c| tree.get(c).map(|n| n.name().to_owned())).collect();
        assert!(!names.iter().any(|n| n.ends_with(".bin")), "{names:?}");
        assert!(names.iter().any(|n| n == "docs"));
    }

    #[test]
    fn node_counts_are_singular_for_one() {
        assert_eq!(node_count_text(1), "1 node");
        assert_eq!(node_count_text(0), "0 nodes");
        assert_eq!(node_count_text(2), "2 nodes");
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        // Rounded to the nearest tenth, not floored (QA-138).
        assert_eq!(human_size(47_998_000), "45.8 MiB");
        assert_eq!(human_size(9000), "8.8 KiB");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024 * 1024 - 1), "1.0 MiB", "a carry moves to the next unit");
        assert_eq!(human_size(1024 * 1024 - 100), "1023.9 KiB");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert!(human_size(u64::MAX).ends_with("PiB"));
    }
}
