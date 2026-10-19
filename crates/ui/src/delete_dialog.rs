//! The delete confirmation dialog (`docs/safety.md`, "UI warning" rules).
//!
//! Nothing here deletes from a click on a menu entry: every destructive action first asks the
//! engine for a read-only report (`file.delete_preview`, `cleanup.preview`) and shows it in a
//! modal dialog. The destructive command is sent only by [`confirm`], which the window calls when
//! the user presses the confirm button; it is not a UI command, so `ui.set_state` and the control
//! channel can open or close the dialog but never confirm it (agents call the engine commands with
//! `confirm: true` themselves).
//!
//! * Recycle: "can be restored from the Recycle Bin", a plain confirm button.
//! * Permanent: a red irreversible warning and a second explicit checkbox
//!   (`i_understand_permanent`); on a partial tree a third one (`i_understand_partial`).
//! * The default focus is Cancel and Enter never confirms a permanent deletion.
//! * A UNC scan root, the scan root itself and a running scan disable the actions up front.

use crate::app::human_size;
use crate::cleanups_ui;
use crate::state::UiState;
use crate::theme::Tokens;
use crate::toast::{self, ToastKind};
use dirustat_engine::{CommandError, Session, is_unc_path};
use dirustat_model::NodeId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Paths shown in the dialog's list.
pub const SHOWN_PATHS: usize = 12;
/// Paths kept in the (serializable) dialog state; the engine reports at most 200 as well.
pub const MAX_STORED_PATHS: usize = 200;

/// Why destructive actions are off on a network scan root (tooltip text).
pub const UNC_REASON: &str =
    "Deleting is disabled on network (UNC) scan roots. Scan a local drive path to delete files.";
const SCANNING_REASON: &str = "Wait for the scan to finish";
const NO_TREE_REASON: &str = "Scan a folder first";
/// Why nothing can be deleted in the default demo (its tree has no files behind it).
pub const DEMO_REASON: &str = "This is synthetic demo data: there are no files on disk to delete. Start with \
     DIRUSTAT_DEMO_DELETABLE=1 to try the delete flow on a disposable fixture.";
pub const ROOT_REASON: &str = "The scan root cannot be deleted";

/// What the dialog will do when confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    /// Move to the Recycle Bin (reversible).
    Recycle,
    /// Delete for good.
    Permanent,
}

impl DeleteMode {
    /// The `mode` parameter of `file.delete_preview`.
    pub fn param(self) -> &'static str {
        match self {
            Self::Recycle => "recycle",
            Self::Permanent => "permanent",
        }
    }
}

/// A request to open or close the dialog from `ui.set_state` (`delete_dialog`). It behaves like
/// pressing Delete / Shift+Delete on the selected node; it can never confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DialogRequest {
    Recycle,
    Permanent,
    Close,
}

/// What the dialog acts on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Subject {
    /// One tree node, addressed by arena id and the session revision the id was read at.
    Node { id: usize, revision: u64 },
    /// A cleanup run (`cleanup.run`).
    Cleanup { id: String },
}

/// A follow-up the dialog offers after a refusal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hint {
    #[default]
    None,
    /// The tree is stale: offer a re-scan.
    Rescan,
    /// The volume has no recycle bin: offer the permanent-delete flow.
    OfferPermanent,
}

/// An error shown inside the dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogError {
    pub message: String,
    pub hint: Hint,
}

/// The state of the confirmation dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteDialog {
    pub mode: DeleteMode,
    pub subject: Subject,
    /// Name of the item, or of the cleanup.
    pub name: String,
    /// Full path of the item (empty for a cleanup).
    pub path: String,
    pub count: u64,
    pub files: u64,
    pub dirs: u64,
    pub size: u64,
    pub paths: Vec<String>,
    pub truncated: bool,
    /// The tree may omit entries that exist on disk (exclusions or read errors).
    pub partial_tree: bool,
    /// Items of a cleanup that are protected, stale or invalid and will be skipped.
    pub skipped: u64,
    /// Checkbox: "I understand this cannot be undone" (`i_understand_permanent`).
    pub ack_permanent: bool,
    /// Checkbox for a partial tree (`i_understand_partial`).
    pub ack_partial: bool,
    pub error: Option<DialogError>,
    /// Cancel already took the keyboard focus (first frame only).
    #[serde(skip)]
    pub focus_done: bool,
}

impl DeleteDialog {
    /// The confirm button is live: no error is showing and every needed checkbox is ticked.
    pub fn can_confirm(&self) -> bool {
        self.error.is_none()
            && match self.mode {
                DeleteMode::Recycle => true,
                DeleteMode::Permanent => self.ack_permanent && (!self.partial_tree || self.ack_partial),
            }
    }

    /// The command and parameters sent on confirm. `id` always travels with its `revision`.
    pub fn request(&self) -> (&'static str, Value) {
        let permanent = self.mode == DeleteMode::Permanent;
        let mut params = match &self.subject {
            Subject::Node { id, revision } => json!({ "id": id, "revision": revision, "confirm": true }),
            Subject::Cleanup { id } => json!({ "id": id, "confirm": true }),
        };
        if permanent && let Some(map) = params.as_object_mut() {
            map.insert("i_understand_permanent".to_owned(), json!(true));
            if self.partial_tree && self.ack_partial {
                map.insert("i_understand_partial".to_owned(), json!(true));
            }
        }
        let command = match (&self.subject, permanent) {
            (Subject::Node { .. }, false) => "file.delete",
            (Subject::Node { .. }, true) => "file.delete_permanent",
            (Subject::Cleanup { .. }, _) => "cleanup.run",
        };
        (command, params)
    }

    fn title(&self) -> &'static str {
        match (&self.subject, self.mode) {
            (Subject::Node { .. }, DeleteMode::Recycle) => "Move to Recycle Bin?",
            (Subject::Node { .. }, DeleteMode::Permanent) => "Delete permanently?",
            (Subject::Cleanup { .. }, DeleteMode::Recycle) => "Run cleanup (Recycle Bin)?",
            (Subject::Cleanup { .. }, DeleteMode::Permanent) => "Run cleanup (delete permanently)?",
        }
    }

    fn confirm_label(&self) -> &'static str {
        match (&self.subject, self.mode) {
            (Subject::Node { .. }, DeleteMode::Recycle) => "Move to Recycle Bin",
            (Subject::Node { .. }, DeleteMode::Permanent) => "Delete permanently",
            (Subject::Cleanup { .. }, DeleteMode::Recycle) => "Run cleanup",
            (Subject::Cleanup { .. }, DeleteMode::Permanent) => "Delete permanently",
        }
    }

    /// Builds the dialog of a node from a `file.delete_preview` report.
    fn from_report(report: &Value, mode: DeleteMode, id: usize, revision: u64, name: &str) -> Self {
        let num = |k: &str| report.get(k).and_then(Value::as_u64).unwrap_or(0);
        let paths = report
            .get("paths")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).take(MAX_STORED_PATHS).map(str::to_owned).collect())
            .unwrap_or_default();
        Self {
            mode,
            subject: Subject::Node { id, revision },
            name: name.to_owned(),
            path: report.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
            count: num("count"),
            files: num("files"),
            dirs: num("dirs"),
            size: num("size"),
            paths,
            truncated: report.get("truncated").and_then(Value::as_bool).unwrap_or(false),
            partial_tree: report.get("partial_tree").and_then(Value::as_bool).unwrap_or(false),
            skipped: 0,
            ack_permanent: false,
            ack_partial: false,
            error: None,
            focus_done: false,
        }
    }
}

/// Why destructive actions are unavailable right now, or `None` when they are allowed.
pub fn block_reason(session: &Session) -> Option<&'static str> {
    if session.is_scanning() {
        return Some(SCANNING_REASON);
    }
    let tree = session.tree();
    if tree.len() <= 1 {
        return Some(NO_TREE_REASON);
    }
    let root = session.node_path(tree.root());
    if root.as_deref().is_some_and(is_unc_path) {
        return Some(UNC_REASON);
    }
    // A scan root is always absolute; a relative one is the neutral label of the demo tree.
    if root.as_deref().is_some_and(|p| !std::path::Path::new(p).is_absolute()) {
        return Some(DEMO_REASON);
    }
    None
}

/// Why `node` cannot be deleted (adds the scan-root rule to [`block_reason`]).
pub fn node_block_reason(session: &Session, node: NodeId) -> Option<&'static str> {
    block_reason(session).or_else(|| (node == session.tree().root()).then_some(ROOT_REASON))
}

/// A user-readable message for a refused or failed destructive command.
pub fn friendly(err: &CommandError, mode: DeleteMode) -> DialogError {
    let (message, hint) = match err {
        CommandError::ProtectedPath(_) => (
            "diRUSTat refuses to delete this: it is protected (a system folder, a user profile, the scan \
             root, or a network location). Nothing was changed."
                .to_owned(),
            Hint::None,
        ),
        CommandError::StaleTree(_) => (
            "The folder tree no longer matches the disk: something changed after the scan. Nothing was \
             deleted. Scan again to refresh the tree."
                .to_owned(),
            Hint::Rescan,
        ),
        CommandError::Unsupported(_) if mode == DeleteMode::Recycle => (
            "This volume has no Recycle Bin (network, removable or unknown drive), so nothing was touched. \
             You can delete the item permanently instead."
                .to_owned(),
            Hint::OfferPermanent,
        ),
        CommandError::ConfirmationRequired(_) => {
            ("diRUSTat needs your explicit confirmation for this action.".to_owned(), Hint::None)
        }
        CommandError::NotEnabled(_) => ("Deleting is not available right now.".to_owned(), Hint::None),
        CommandError::InvalidParams(m) => (format!("Cannot delete this item: {m}"), Hint::None),
        other => (other.to_string(), Hint::None),
    };
    DialogError { message, hint }
}

/// Opens the confirmation dialog for `node` (the Delete / Shift+Delete / context-menu path).
///
/// Only a read-only preview runs here. The error is the text for a toast: a refusal never opens
/// the dialog and never deletes. A dialog that is already open is left alone.
pub fn open_for_node(session: &mut Session, state: &mut UiState, node: NodeId, mode: DeleteMode) -> Result<(), String> {
    if state.delete_dialog.is_some() {
        return Ok(());
    }
    if let Some(reason) = node_block_reason(session, node) {
        return Err(reason.to_owned());
    }
    let revision = session.revision();
    let name = session.tree().get(node).map(|n| n.name().to_owned()).unwrap_or_default();
    let params = json!({ "id": node.index(), "revision": revision, "mode": mode.param() });
    let report = session.execute("file.delete_preview", &params).map_err(|e| friendly(&e, mode).message)?;
    state.delete_dialog = Some(DeleteDialog::from_report(&report, mode, node.index(), revision, &name));
    Ok(())
}

/// Opens the dialog for the selected node; the error is a toast text.
pub fn open_for_selection(session: &mut Session, state: &mut UiState, mode: DeleteMode) -> Result<(), String> {
    let node = session.selection().ok_or_else(|| "Select a file or folder first".to_owned())?;
    open_for_node(session, state, node, mode)
}

/// Replaces the open dialog by the permanent-delete flow for the same node (after `unsupported`
/// on a volume without a recycle bin). The user still has to tick the boxes and confirm.
pub fn switch_to_permanent(session: &mut Session, state: &mut UiState) {
    let Some(dialog) = state.delete_dialog.take() else { return };
    let Subject::Node { id, revision } = dialog.subject else {
        state.delete_dialog = Some(dialog);
        return;
    };
    let params = json!({ "id": id, "revision": revision, "mode": DeleteMode::Permanent.param() });
    match session.execute("file.delete_preview", &params) {
        Ok(report) => {
            state.delete_dialog =
                Some(DeleteDialog::from_report(&report, DeleteMode::Permanent, id, revision, &dialog.name));
        }
        Err(e) => {
            let mut next = dialog;
            next.error = Some(friendly(&e, DeleteMode::Permanent));
            state.delete_dialog = Some(next);
        }
    }
}

/// Sends the destructive command of the open dialog. UI-only: called when the user presses the
/// confirm button, never from a command. Does nothing unless [`DeleteDialog::can_confirm`].
///
/// On success the dialog closes, a toast reports the outcome and the stale views are dropped (the
/// engine bumped the revision; ids are invalid, the selection is cleared). On failure the dialog
/// stays open with a friendly message and, where it helps, a follow-up button.
pub fn confirm(session: &mut Session, state: &mut UiState) {
    let Some(mut dialog) = state.delete_dialog.take() else { return };
    if !dialog.can_confirm() {
        state.delete_dialog = Some(dialog);
        return;
    }
    let (command, params) = dialog.request();
    match session.execute(command, &params) {
        Ok(result) => {
            state.notice = None;
            state.journal.loaded = false;
            state.cleanups.preview = None;
            if matches!(dialog.subject, Subject::Cleanup { .. }) {
                cleanups_ui::finish_run(state, &dialog, &result);
            } else {
                toast::push(state, ToastKind::Success, success_text(&dialog, &result));
            }
        }
        Err(err) => {
            dialog.error = Some(friendly(&err, dialog.mode));
            state.journal.loaded = false;
            state.delete_dialog = Some(dialog);
        }
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "3 items (2 files, 1 folder)" for the counts of a report.
pub fn describe_counts(count: u64, files: u64, dirs: u64) -> String {
    if count <= 1 {
        return "1 item".to_owned();
    }
    format!(
        "{} ({}, {})",
        plural(count, "item", "items"),
        plural(files, "file", "files"),
        plural(dirs, "folder", "folders")
    )
}

fn success_text(dialog: &DeleteDialog, result: &Value) -> String {
    let count = result.get("count").and_then(Value::as_u64).unwrap_or(dialog.count);
    let size = result.get("size").and_then(Value::as_u64).unwrap_or(dialog.size);
    let what = match dialog.mode {
        DeleteMode::Recycle => "Moved to the Recycle Bin",
        DeleteMode::Permanent => "Deleted permanently",
    };
    let mut text = format!("{what}: {} ({}), {} freed.", dialog.name, plural(count, "item", "items"), human_size(size));
    if dialog.mode == DeleteMode::Recycle {
        text.push_str(" You can restore it from the Recycle Bin.");
    }
    text
}

/// What the user did in the dialog this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogEvent {
    None,
    Cancel,
    Confirm,
    Rescan,
    SwitchToPermanent,
}

/// Whether a press of the confirm button counts. A permanent deletion is never triggered by the
/// keyboard Enter key: the user must click it.
pub fn activation_allowed(mode: DeleteMode, enter_pressed: bool) -> bool {
    mode == DeleteMode::Recycle || !enter_pressed
}

/// Draws the modal dialog and reports what the user did.
pub fn show(ctx: &egui::Context, dialog: &mut DeleteDialog, tokens: &Tokens) -> DialogEvent {
    let permanent = dialog.mode == DeleteMode::Permanent;
    let danger = Tokens::color(tokens.error);
    let muted = Tokens::color(tokens.text_muted);
    let mut event = DialogEvent::None;
    let width = (ctx.content_rect().width() - 80.0).clamp(280.0, 540.0);
    let frame = egui::Frame::popup(&ctx.global_style())
        .fill(Tokens::color(tokens.surface))
        .stroke(egui::Stroke::new(
            if permanent { 2.0 } else { 1.0 },
            if permanent { danger } else { Tokens::color(tokens.border) },
        ))
        .corner_radius(egui::CornerRadius::same(tokens.radius_medium))
        .inner_margin(egui::Margin::same(16));
    let modal = egui::Modal::new(egui::Id::new("delete_dialog")).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = tokens.spacing;
        let title = egui::RichText::new(dialog.title()).heading().color(if permanent { danger } else { Tokens::color(tokens.text) });
        ui.label(title);
        ui.label(egui::RichText::new(&dialog.name).strong());
        if !dialog.path.is_empty() {
            crate::text_fit::path_label(ui, &dialog.path, tokens.font_body, Some(muted));
        }
        ui.label(format!("{} - {}", describe_counts(dialog.count, dialog.files, dialog.dirs), human_size(dialog.size)));
        if dialog.skipped > 0 {
            ui.colored_label(
                Tokens::color(tokens.warning),
                format!("{} will be skipped (protected, changed on disk or invalid).", plural(dialog.skipped, "item", "items")),
            );
        }
        path_list(ui, dialog, tokens);
        if permanent {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0, danger))
                .corner_radius(egui::CornerRadius::same(tokens.radius_small))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "This cannot be undone. The items are deleted for good and do not go to the Recycle Bin.",
                            )
                            .strong()
                            .color(danger),
                        )
                        .wrap(),
                    );
                    ui.checkbox(&mut dialog.ack_permanent, "I understand this cannot be undone");
                });
        } else {
            ui.add(egui::Label::new(egui::RichText::new("You can restore it from the Recycle Bin.").color(muted)).wrap());
        }
        if dialog.partial_tree {
            let warn = Tokens::color(tokens.warning);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "The scan of this folder was incomplete (exclusions or unreadable parts). It may contain \
                         items that are not listed here.",
                    )
                    .color(warn),
                )
                .wrap(),
            );
            if permanent {
                ui.checkbox(&mut dialog.ack_partial, "I understand more items than listed may be deleted");
            }
        }
        if let Some(error) = &dialog.error {
            ui.add(egui::Label::new(egui::RichText::new(&error.message).color(danger)).wrap());
        }
        ui.add_space(tokens.spacing);
        event = buttons(ui, dialog, tokens);
    });
    if event == DialogEvent::None && modal.should_close() {
        event = DialogEvent::Cancel;
    }
    event
}

/// The list repeats nothing: a single item whose path is already printed above is not listed again.
pub fn shows_path_list(dialog: &DeleteDialog) -> bool {
    match dialog.paths.as_slice() {
        [] => false,
        [only] => only != &dialog.path,
        _ => true,
    }
}

fn path_list(ui: &mut egui::Ui, dialog: &DeleteDialog, tokens: &Tokens) {
    if !shows_path_list(dialog) {
        return;
    }
    let muted = Tokens::color(tokens.text_muted);
    egui::ScrollArea::vertical().id_salt("delete_dialog_paths").max_height(130.0).show(ui, |ui| {
        for path in dialog.paths.iter().take(SHOWN_PATHS) {
            crate::text_fit::path_label(ui, path, tokens.font_small, None);
        }
        let shown = u64::try_from(dialog.paths.len().min(SHOWN_PATHS)).unwrap_or(0);
        let more = dialog.count.saturating_sub(shown);
        if more > 0 {
            ui.colored_label(muted, format!("... and {more} more"));
        }
    });
}

fn buttons(ui: &mut egui::Ui, dialog: &mut DeleteDialog, tokens: &Tokens) -> DialogEvent {
    let mut event = DialogEvent::None;
    ui.horizontal(|ui| {
        let cancel = ui.button("Cancel");
        if !dialog.focus_done {
            cancel.request_focus();
            dialog.focus_done = true;
        }
        if cancel.clicked() {
            event = DialogEvent::Cancel;
        }
        match dialog.error.as_ref().map(|e| e.hint) {
            Some(Hint::Rescan) if ui.button("Scan again").clicked() => event = DialogEvent::Rescan,
            Some(Hint::OfferPermanent) if ui.button("Delete permanently instead...").clicked() => {
                event = DialogEvent::SwitchToPermanent;
            }
            _ => {}
        }
        if dialog.error.is_some() {
            return;
        }
        let permanent = dialog.mode == DeleteMode::Permanent;
        let mut button = egui::Button::new(dialog.confirm_label());
        if permanent {
            button =
                egui::Button::new(egui::RichText::new(dialog.confirm_label()).color(Tokens::color(tokens.background)))
                    .fill(Tokens::color(tokens.error));
        }
        let response = ui.add_enabled(dialog.can_confirm(), button);
        let response = if permanent && !dialog.can_confirm() {
            response.on_disabled_hover_text("Tick the confirmation box first")
        } else {
            response
        };
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if response.clicked() && activation_allowed(dialog.mode, enter) {
            event = DialogEvent::Confirm;
        }
    });
    event
}
