//! The Cleanups window (`ui.cleanups`, Tools > Cleanups): lists `cleanup.list`, previews a cleanup
//! (`cleanup.preview`), runs it through the same confirmation dialog as a plain delete, and edits
//! user cleanups (`cleanup.save` / `cleanup.remove`). Built-ins are read-only. Nothing runs by
//! itself; Run only opens the dialog, which sends `cleanup.run` after an explicit click.

use crate::app::human_size;
use crate::delete_dialog::{self, DeleteDialog, DeleteMode, MAX_STORED_PATHS, Subject};
use crate::search_panel::{parse_extensions, parse_size};
use crate::state::UiState;
use crate::theme::Tokens;
use crate::toast::{self, ToastKind};
use dirustat_engine::{CommandError, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Preview rows kept in the state (the engine reports up to 10000).
pub const MAX_PREVIEW_ROWS: usize = 500;
/// Result rows kept from a run.
pub const MAX_RESULT_ROWS: usize = 200;
/// Preview rows drawn.
const SHOWN_ROWS: usize = 200;

/// One cleanup of `cleanup.list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupEntry {
    pub id: String,
    pub name: String,
    /// `recycle`, `permanent`, `empty_dir_contents` or `reveal`.
    pub action: String,
    pub builtin: bool,
    pub selector: Value,
}

/// A row of a preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewItem {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub size: u64,
    /// `ok`, `protected`, `stale`, `invalid` (or `needs_confirmation`).
    pub status: String,
    pub partial_tree: bool,
    pub error: Option<String>,
}

/// The result of `cleanup.preview`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewView {
    pub id: String,
    pub name: String,
    pub action: String,
    /// Session revision the preview was read at; a different revision makes it outdated.
    pub revision: u64,
    pub count: u64,
    pub size: u64,
    pub truncated: bool,
    pub ok: u64,
    pub protected: u64,
    pub stale: u64,
    pub invalid: u64,
    pub partial: u64,
    pub items: Vec<PreviewItem>,
}

/// A row of a run report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunItem {
    pub path: String,
    pub status: String,
    pub error: Option<String>,
}

/// The outcome of a `cleanup.run` (also of a dry run).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub name: String,
    pub dry_run: bool,
    pub count: u64,
    pub size: u64,
    pub deleted: u64,
    pub deleted_size: u64,
    pub failed: u64,
    pub not_attempted: u64,
    pub stopped: Option<String>,
    pub rescan_recommended: bool,
    pub results: Vec<RunItem>,
}

/// Action choice of the editor form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionChoice {
    #[default]
    Recycle,
    Permanent,
    EmptyDirContents,
    Reveal,
}

impl ActionChoice {
    pub const ALL: [Self; 4] = [Self::Recycle, Self::Permanent, Self::EmptyDirContents, Self::Reveal];

    pub fn param(self) -> &'static str {
        match self {
            Self::Recycle => "recycle",
            Self::Permanent => "permanent",
            Self::EmptyDirContents => "empty_dir_contents",
            Self::Reveal => "reveal",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Recycle => "Move to Recycle Bin",
            Self::Permanent => "Delete permanently",
            Self::EmptyDirContents => "Empty folder contents (Recycle Bin)",
            Self::Reveal => "Show in file manager",
        }
    }

    fn from_param(text: &str) -> Self {
        Self::ALL.into_iter().find(|a| a.param() == text).unwrap_or_default()
    }
}

/// Kind choice of the editor form.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindChoice {
    #[default]
    Any,
    File,
    Dir,
}

impl KindChoice {
    pub const ALL: [Self; 3] = [Self::Any, Self::File, Self::Dir];

    pub fn param(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::File => "file",
            Self::Dir => "dir",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Files and folders",
            Self::File => "Files only",
            Self::Dir => "Folders only",
        }
    }

    fn from_param(text: &str) -> Self {
        Self::ALL.into_iter().find(|k| k.param() == text).unwrap_or_default()
    }
}

/// The editor form for a new or existing user cleanup. Everything is text until it is validated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupForm {
    /// The id of the cleanup being edited (its id cannot change); `None` for a new one.
    pub editing: Option<String>,
    pub id: String,
    pub name: String,
    pub action: ActionChoice,
    /// Name patterns separated by commas.
    pub names: String,
    /// Extensions separated by commas or spaces.
    pub extensions: String,
    pub min_size: String,
    pub max_size: String,
    pub kind: KindChoice,
    pub only_under: String,
    pub older_than_days: String,
    pub empty: bool,
    /// Inline validation error.
    pub error: Option<String>,
}

/// Everything the window remembers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupsState {
    pub open: bool,
    pub entries: Vec<CleanupEntry>,
    pub selected: Option<String>,
    pub preview: Option<PreviewView>,
    pub last_run: Option<RunSummary>,
    pub form: Option<CleanupForm>,
    /// Id whose removal waits for the second click.
    pub confirm_remove: Option<String>,
    pub error: Option<String>,
    /// The list was read from the session (cleared to reload).
    #[serde(skip)]
    pub loaded: bool,
    /// Fingerprint of the saved cleanups `entries` were read at; a different one (saved through
    /// the engine, for example by a control client) makes the list stale.
    #[serde(skip)]
    pub settings_key: Option<u64>,
}

/// What the user did in the window this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupsAction {
    Reload,
    Select(String),
    Preview(String),
    Run(String),
    DryRun(String),
    New,
    Edit(String),
    AskRemove(String),
    Remove(String),
    CancelRemove,
    Save,
    CancelEdit,
}

fn strip(message: &str) -> String {
    message.strip_prefix("invalid params: ").unwrap_or(message).to_owned()
}

fn u64_of(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn entries_of(list: &Value) -> Vec<CleanupEntry> {
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|c| CleanupEntry {
                    id: str_of(c, "id"),
                    name: str_of(c, "name"),
                    action: str_of(c, "action"),
                    builtin: c.get("builtin").and_then(Value::as_bool).unwrap_or(false),
                    selector: c.get("selector").cloned().unwrap_or(Value::Null),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Fingerprint of the user cleanups saved in the session settings (cheap: a handful of entries).
pub fn settings_key(session: &Session) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&session.settings().cleanups).unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// True when the list has to be read again: never read, or the saved cleanups changed since.
pub fn needs_reload(session: &Session, state: &CleanupsState) -> bool {
    !state.loaded || state.settings_key != Some(settings_key(session))
}

/// Reads `cleanup.list` into the state.
pub fn load(session: &mut Session, state: &mut UiState) {
    state.cleanups.settings_key = Some(settings_key(session));
    match session.execute("cleanup.list", &json!({})) {
        Ok(v) => {
            state.cleanups.entries = entries_of(v.get("cleanups").unwrap_or(&Value::Null));
            state.cleanups.error = None;
        }
        Err(e) => state.cleanups.error = Some(strip(&e.to_string())),
    }
    state.cleanups.loaded = true;
}

fn parse_preview(v: &Value) -> PreviewView {
    let items: Vec<PreviewItem> = v
        .get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_PREVIEW_ROWS)
                .map(|i| PreviewItem {
                    name: str_of(i, "name"),
                    kind: str_of(i, "kind"),
                    path: str_of(i, "path"),
                    size: u64_of(i, "size"),
                    status: str_of(i, "status"),
                    partial_tree: i.get("partial_tree").and_then(Value::as_bool).unwrap_or(false),
                    error: i.get("error").and_then(Value::as_str).map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default();
    // Counts come from the shown rows; the engine's `count` and `size` cover all of them.
    let count_of =
        |status: &str| u64::try_from(items.iter().filter(|i| i.status == status).count()).unwrap_or(u64::MAX);
    PreviewView {
        id: str_of(v, "id"),
        name: str_of(v, "name"),
        action: str_of(v, "action"),
        revision: u64_of(v, "revision"),
        count: u64_of(v, "count"),
        size: u64_of(v, "size"),
        truncated: v.get("truncated").and_then(Value::as_bool).unwrap_or(false),
        ok: count_of("ok"),
        protected: count_of("protected"),
        stale: count_of("stale"),
        invalid: count_of("invalid").saturating_add(count_of("needs_confirmation")),
        partial: u64::try_from(items.iter().filter(|i| i.partial_tree).count()).unwrap_or(u64::MAX),
        items,
    }
}

/// Runs `cleanup.preview` (read-only) and keeps the result.
///
/// A failure is also shown inline in the window (the previous preview is dropped, it no longer
/// belongs to the selection) and returned as text.
pub fn preview(session: &mut Session, state: &mut UiState, id: &str) -> Result<(), String> {
    match session.execute("cleanup.preview", &json!({ "id": id })) {
        Ok(v) => {
            state.cleanups.preview = Some(parse_preview(&v));
            state.cleanups.error = None;
            Ok(())
        }
        Err(e) => {
            let message = cleanup_error(&e, &state.cleanups.entries, id);
            state.cleanups.preview = None;
            state.cleanups.error = Some(message.clone());
            Err(message)
        }
    }
}

/// Text for a refused cleanup preview or run. Worded for cleanups (not "cannot delete this item")
/// and names the folder a cleanup is limited to when that folder is not in the scanned tree.
pub fn cleanup_error(err: &CommandError, entries: &[CleanupEntry], id: &str) -> String {
    let CommandError::InvalidParams(message) = err else {
        return delete_dialog::friendly(err, DeleteMode::Recycle).message;
    };
    if message.contains("does not resolve to a node") {
        let entry = entries.iter().find(|e| e.id == id);
        let name = entry.map_or(id, |e| e.name.as_str());
        let under = entry.and_then(|e| e.selector.get("only_under")).and_then(Value::as_str);
        return match under {
            Some(path) => format!(
                "Cleanup \"{name}\" is limited to \"{path}\", which is outside the scanned folder. Scan a folder that contains it, or edit the cleanup."
            ),
            None => format!("Cleanup \"{name}\" refers to a folder that is not in the scanned tree."),
        };
    }
    strip(message)
}

/// Opens the confirmation dialog for a cleanup, from a fresh preview. The `reveal` action changes
/// nothing on disk and runs at once. Errors are texts for a toast.
pub fn open_run_dialog(session: &mut Session, state: &mut UiState, id: &str) -> Result<(), String> {
    if state.delete_dialog.is_some() {
        return Ok(());
    }
    if let Some(reason) = delete_dialog::block_reason(session) {
        return Err(reason.to_owned());
    }
    preview(session, state, id)?;
    let Some(view) = state.cleanups.preview.clone() else { return Err("No preview".to_owned()) };
    if view.action == "reveal" {
        let result = session
            .execute("cleanup.run", &json!({ "id": id, "confirm": true }))
            .map_err(|e| cleanup_error(&e, &state.cleanups.entries, id))?;
        state.cleanups.last_run = Some(parse_run(&result, &view.name));
        toast::push(state, ToastKind::Info, "Showing the largest match in the file manager.");
        return Ok(());
    }
    let ok_items: Vec<&PreviewItem> = view.items.iter().filter(|i| i.status == "ok").collect();
    if ok_items.is_empty() {
        return Err(format!("Nothing to clean: \"{}\" matches no items that can be acted on.", view.name));
    }
    let ok_count = u64::try_from(ok_items.len()).unwrap_or(u64::MAX);
    let files = u64::try_from(ok_items.iter().filter(|i| i.kind != "dir").count()).unwrap_or(u64::MAX);
    let size = ok_items.iter().fold(0u64, |acc, i| acc.saturating_add(i.size));
    let mode = if view.action == "permanent" { DeleteMode::Permanent } else { DeleteMode::Recycle };
    state.delete_dialog = Some(DeleteDialog {
        mode,
        subject: Subject::Cleanup { id: id.to_owned() },
        name: view.name.clone(),
        path: String::new(),
        count: ok_count,
        files,
        dirs: ok_count.saturating_sub(files),
        size,
        paths: ok_items.iter().take(MAX_STORED_PATHS).map(|i| i.path.clone()).collect(),
        truncated: view.truncated,
        partial_tree: ok_items.iter().any(|i| i.partial_tree),
        skipped: view.count.saturating_sub(ok_count),
        ack_permanent: false,
        ack_partial: false,
        error: None,
        focus_done: false,
    });
    Ok(())
}

fn parse_run(v: &Value, name: &str) -> RunSummary {
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_RESULT_ROWS)
                .map(|r| RunItem {
                    path: str_of(r, "path"),
                    status: str_of(r, "status"),
                    error: r.get("error").and_then(Value::as_str).map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default();
    RunSummary {
        id: str_of(v, "id"),
        name: name.to_owned(),
        dry_run: v.get("dry_run").and_then(Value::as_bool).unwrap_or(false),
        count: u64_of(v, "count"),
        size: u64_of(v, "size"),
        deleted: u64_of(v, "deleted"),
        deleted_size: u64_of(v, "deleted_size"),
        failed: u64_of(v, "failed"),
        not_attempted: u64_of(v, "not_attempted"),
        stopped: v.get("stopped").and_then(Value::as_str).map(str::to_owned),
        rescan_recommended: v.get("rescan_recommended").and_then(Value::as_bool).unwrap_or(false),
        results,
    }
}

/// Runs the cleanup as a dry run: the report of what would happen, nothing changes on disk.
pub fn dry_run(session: &mut Session, state: &mut UiState, id: &str) -> Result<(), String> {
    let name = state.cleanups.entries.iter().find(|e| e.id == id).map(|e| e.name.clone()).unwrap_or_default();
    let v = session
        .execute("cleanup.run", &json!({ "id": id, "dry_run": true, "confirm": true }))
        .map_err(|e| cleanup_error(&e, &state.cleanups.entries, id))?;
    state.cleanups.last_run = Some(parse_run(&v, &name));
    Ok(())
}

/// The toast text for a finished run.
fn run_text(s: &RunSummary, mode: DeleteMode) -> String {
    let verb = match mode {
        DeleteMode::Recycle => "moved to the Recycle Bin",
        DeleteMode::Permanent => "deleted permanently",
    };
    let mut text = format!("Cleanup \"{}\": {} {verb} ({} freed)", s.name, s.deleted, human_size(s.deleted_size));
    if s.failed > 0 {
        text.push_str(&format!(", {} failed", s.failed));
    }
    if s.not_attempted > 0 {
        text.push_str(&format!(", {} not attempted", s.not_attempted));
    }
    text.push('.');
    if let Some(reason) = &s.stopped {
        text.push_str(&format!(" Stopped: {reason}."));
    }
    if s.rescan_recommended {
        text.push_str(" Scan again to refresh the tree.");
    }
    text
}

/// Records the result of a confirmed `cleanup.run` (the dialog already sent it).
pub fn finish_run(state: &mut UiState, dialog: &DeleteDialog, result: &Value) {
    let summary = parse_run(result, &dialog.name);
    let kind = if summary.failed > 0 || summary.stopped.is_some() { ToastKind::Error } else { ToastKind::Success };
    toast::push(state, kind, run_text(&summary, dialog.mode));
    state.cleanups.last_run = Some(summary);
}

/// URL-safe id from a name: lowercase letters, digits and dashes.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(64).collect()
}

fn split_list(text: &str) -> Vec<String> {
    text.split([',', ';', '\n']).map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned).collect()
}

/// Validates the form and builds the `definition` of `cleanup.save`. The engine validates again.
pub fn build_definition(form: &CleanupForm) -> Result<Value, String> {
    let name = form.name.trim();
    if name.is_empty() {
        return Err("Give the cleanup a name".to_owned());
    }
    let id = match (&form.editing, form.id.trim()) {
        (Some(id), _) => id.clone(),
        (None, "") => slugify(name),
        (None, typed) => typed.to_owned(),
    };
    if id.is_empty() {
        return Err("The id needs at least one letter or digit".to_owned());
    }
    let mut selector = serde_json::Map::new();
    let names = split_list(&form.names);
    if !names.is_empty() {
        selector.insert("names".to_owned(), json!(names));
    }
    let extensions = parse_extensions(&form.extensions);
    if !extensions.is_empty() {
        selector.insert("extensions".to_owned(), json!(extensions));
    }
    if let Some(min) = parse_size(&form.min_size).map_err(|e| format!("Minimum size: {e}"))? {
        selector.insert("min_size".to_owned(), json!(min));
    }
    if let Some(max) = parse_size(&form.max_size).map_err(|e| format!("Maximum size: {e}"))? {
        selector.insert("max_size".to_owned(), json!(max));
    }
    selector.insert("kind".to_owned(), json!(form.kind.param()));
    let under = form.only_under.trim();
    if !under.is_empty() {
        selector.insert("only_under".to_owned(), json!(under));
    }
    let days = form.older_than_days.trim();
    if !days.is_empty() {
        let n: u32 = days.parse().map_err(|_| "Older than: type a whole number of days".to_owned())?;
        selector.insert("older_than_days".to_owned(), json!(n));
    }
    if form.empty {
        selector.insert("empty".to_owned(), json!(true));
    }
    Ok(json!({ "id": id, "name": name, "action": form.action.param(), "selector": selector }))
}

/// A form filled from an existing cleanup.
pub fn form_from_entry(entry: &CleanupEntry) -> CleanupForm {
    let sel = &entry.selector;
    let list = |key: &str| {
        sel.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    let num = |key: &str| sel.get(key).and_then(Value::as_u64).map(|n| n.to_string()).unwrap_or_default();
    CleanupForm {
        editing: Some(entry.id.clone()),
        id: entry.id.clone(),
        name: entry.name.clone(),
        action: ActionChoice::from_param(&entry.action),
        names: list("names"),
        extensions: list("extensions"),
        min_size: num("min_size"),
        max_size: num("max_size"),
        kind: KindChoice::from_param(sel.get("kind").and_then(Value::as_str).unwrap_or("any")),
        only_under: sel.get("only_under").and_then(Value::as_str).unwrap_or_default().to_owned(),
        older_than_days: num("older_than_days"),
        empty: sel.get("empty").and_then(Value::as_bool).unwrap_or(false),
        error: None,
    }
}

/// Saves the open form through `cleanup.save`; validation errors stay inline in the form.
pub fn save_form(session: &mut Session, state: &mut UiState) {
    let Some(form) = state.cleanups.form.as_mut() else { return };
    let definition = match build_definition(form) {
        Ok(d) => d,
        Err(e) => {
            form.error = Some(e);
            return;
        }
    };
    let id = definition.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    match session.execute("cleanup.save", &json!({ "definition": definition })) {
        Ok(v) => {
            state.cleanups.entries = entries_of(v.get("cleanups").unwrap_or(&Value::Null));
            state.cleanups.form = None;
            state.cleanups.selected = Some(id);
            state.cleanups.preview = None;
            state.cleanups.error = None;
        }
        Err(e) => form.error = Some(strip(&e.to_string())),
    }
}

/// Removes a user cleanup through `cleanup.remove` (touches no file).
pub fn remove(session: &mut Session, state: &mut UiState, id: &str) {
    state.cleanups.confirm_remove = None;
    match session.execute("cleanup.remove", &json!({ "id": id })) {
        Ok(v) => {
            state.cleanups.entries = entries_of(v.get("cleanups").unwrap_or(&Value::Null));
            if state.cleanups.selected.as_deref() == Some(id) {
                state.cleanups.selected = None;
                state.cleanups.preview = None;
            }
            state.cleanups.error = None;
        }
        Err(e) => state.cleanups.error = Some(strip(&e.to_string())),
    }
}

/// One line describing what a selector picks.
pub fn selector_summary(sel: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    let list = |key: &str| -> Vec<&str> {
        sel.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
    };
    let names = list("names");
    if !names.is_empty() {
        parts.push(format!("names {}", names.join(", ")));
    }
    let exts = list("extensions");
    if !exts.is_empty() {
        parts.push(format!("extensions {}", exts.join(", ")));
    }
    if let Some(n) = sel.get("min_size").and_then(Value::as_u64) {
        parts.push(format!("at least {}", human_size(n)));
    }
    if let Some(n) = sel.get("max_size").and_then(Value::as_u64) {
        parts.push(format!("at most {}", human_size(n)));
    }
    match sel.get("kind").and_then(Value::as_str) {
        Some("file") => parts.push("files only".to_owned()),
        Some("dir") => parts.push("folders only".to_owned()),
        _ => {}
    }
    if let Some(n) = sel.get("older_than_days").and_then(Value::as_u64) {
        parts.push(format!("older than {n} days"));
    }
    if sel.get("empty").and_then(Value::as_bool) == Some(true) {
        parts.push("empty folders".to_owned());
    }
    if let Some(p) = sel.get("only_under").and_then(Value::as_str) {
        parts.push(format!("under {p}"));
    }
    if parts.is_empty() { "no criteria".to_owned() } else { parts.join("; ") }
}

fn action_label(action: &str) -> &'static str {
    ActionChoice::from_param(action).label()
}

/// Draws the window. `block` is the reason destructive actions are off (UNC root, running scan);
/// `revision` marks an old preview as outdated.
pub fn show(
    ctx: &egui::Context,
    state: &mut CleanupsState,
    tokens: &Tokens,
    block: Option<&str>,
    revision: u64,
) -> Option<CleanupsAction> {
    let mut action = None;
    let mut open = state.open;
    egui::Window::new("Cleanups").open(&mut open).default_width(700.0).default_height(520.0).show(ctx, |ui| {
        if state.form.is_some() {
            action = form_ui(ui, state, tokens);
        } else {
            action = list_ui(ui, state, tokens, block, revision);
        }
    });
    state.open = open;
    action
}

fn list_ui(
    ui: &mut egui::Ui,
    state: &CleanupsState,
    tokens: &Tokens,
    block: Option<&str>,
    revision: u64,
) -> Option<CleanupsAction> {
    let muted = Tokens::color(tokens.text_muted);
    let mut action = None;
    ui.add(
        egui::Label::new(
            egui::RichText::new("Nothing runs automatically: a cleanup acts only after you preview it and confirm.")
                .color(muted),
        )
        .wrap(),
    );
    if let Some(reason) = block {
        ui.colored_label(Tokens::color(tokens.warning), reason);
    }
    if let Some(error) = &state.error {
        ui.colored_label(Tokens::color(tokens.error), error);
    }
    ui.horizontal(|ui| {
        if ui.button("New cleanup...").clicked() {
            action = Some(CleanupsAction::New);
        }
        if ui.button("Refresh").clicked() {
            action = Some(CleanupsAction::Reload);
        }
    });
    egui::ScrollArea::vertical().id_salt("cleanup_list").max_height(130.0).show(ui, |ui| {
        for entry in &state.entries {
            let selected = state.selected.as_deref() == Some(entry.id.as_str());
            let label = if entry.builtin { format!("{}   (built-in)", entry.name) } else { entry.name.clone() };
            if ui.selectable_label(selected, label).clicked() {
                action = Some(CleanupsAction::Select(entry.id.clone()));
            }
        }
        if state.entries.is_empty() {
            ui.colored_label(muted, "No cleanups");
        }
    });
    ui.separator();
    if let Some(entry) = state.selected.as_ref().and_then(|id| state.entries.iter().find(|e| &e.id == id)) {
        ui.label(egui::RichText::new(&entry.name).strong());
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("{} - {}", action_label(&entry.action), selector_summary(&entry.selector)))
                    .color(muted),
            )
            .wrap(),
        );
        action = action.or(entry_buttons(ui, state, entry, block));
    } else {
        ui.colored_label(muted, "Select a cleanup to preview or run it");
    }
    if let Some(view) = &state.preview {
        ui.separator();
        preview_ui(ui, view, tokens, revision);
    }
    if let Some(run) = &state.last_run {
        ui.separator();
        run_ui(ui, run, tokens);
    }
    action
}

fn entry_buttons(
    ui: &mut egui::Ui,
    state: &CleanupsState,
    entry: &CleanupEntry,
    block: Option<&str>,
) -> Option<CleanupsAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if ui.button("Preview").clicked() {
            action = Some(CleanupsAction::Preview(entry.id.clone()));
        }
        let acts = entry.action != "reveal";
        let run = ui.add_enabled(block.is_none(), egui::Button::new("Run..."));
        let run = match block {
            Some(reason) => run.on_disabled_hover_text(reason),
            None => run,
        };
        if run.clicked() {
            action = Some(CleanupsAction::Run(entry.id.clone()));
        }
        if acts && ui.button("Dry run").on_hover_text("Report what would happen; changes nothing").clicked() {
            action = Some(CleanupsAction::DryRun(entry.id.clone()));
        }
        if entry.builtin {
            ui.weak("Built-in cleanups are read-only");
        } else if state.confirm_remove.as_deref() == Some(entry.id.as_str()) {
            if ui.button("Yes, remove").clicked() {
                action = Some(CleanupsAction::Remove(entry.id.clone()));
            }
            if ui.button("Keep").clicked() {
                action = Some(CleanupsAction::CancelRemove);
            }
        } else {
            if ui.button("Edit...").clicked() {
                action = Some(CleanupsAction::Edit(entry.id.clone()));
            }
            if ui.button("Remove").clicked() {
                action = Some(CleanupsAction::AskRemove(entry.id.clone()));
            }
        }
    });
    action
}

fn status_color(status: &str, tokens: &Tokens) -> egui::Color32 {
    Tokens::color(match status {
        "ok" | "done" => tokens.success,
        "protected" | "stale" | "needs_confirmation" => tokens.warning,
        _ => tokens.error,
    })
}

fn preview_ui(ui: &mut egui::Ui, view: &PreviewView, tokens: &Tokens, revision: u64) {
    let muted = Tokens::color(tokens.text_muted);
    ui.label(egui::RichText::new(format!("Preview: {}", view.name)).strong());
    let mut line = format!("{} matched, {}", view.count, human_size(view.size));
    for (n, word) in [
        (view.ok, "ok"),
        (view.protected, "protected"),
        (view.stale, "stale"),
        (view.invalid, "invalid"),
        (view.partial, "partial"),
    ] {
        if n > 0 {
            line.push_str(&format!(" - {n} {word}"));
        }
    }
    ui.label(line);
    if view.revision != revision {
        ui.colored_label(Tokens::color(tokens.warning), "The tree changed since this preview; preview again.");
    }
    if view.truncated {
        ui.colored_label(Tokens::color(tokens.warning), "The list is cut at the engine limit.");
    }
    egui::ScrollArea::vertical().id_salt("cleanup_preview").max_height(200.0).auto_shrink([false, true]).show(
        ui,
        |ui| {
            egui::Grid::new("cleanup_preview_grid").striped(true).num_columns(3).show(ui, |ui| {
                for item in view.items.iter().take(SHOWN_ROWS) {
                    let label =
                        if item.partial_tree && item.status == "ok" { "ok, partial" } else { item.status.as_str() };
                    let status = ui.colored_label(status_color(&item.status, tokens), label);
                    if let Some(error) = &item.error {
                        status.on_hover_text(error);
                    }
                    ui.label(egui::RichText::new(human_size(item.size)).color(muted));
                    crate::text_fit::path_label(ui, &item.path, tokens.font_small, None);
                    ui.end_row();
                }
            });
            if view.items.len() > SHOWN_ROWS {
                ui.colored_label(muted, format!("... and {} more", view.items.len().saturating_sub(SHOWN_ROWS)));
            }
        },
    );
}

fn run_ui(ui: &mut egui::Ui, run: &RunSummary, tokens: &Tokens) {
    let title = if run.dry_run { "Dry run" } else { "Last run" };
    ui.label(egui::RichText::new(format!("{title}: {}", run.name)).strong());
    let mut line = if run.dry_run {
        format!("would act on {} items ({})", run.count.saturating_sub(run.failed), human_size(run.size))
    } else {
        format!("deleted {} ({})", run.deleted, human_size(run.deleted_size))
    };
    line.push_str(&format!(", failed {}, not attempted {}", run.failed, run.not_attempted));
    ui.label(line);
    if let Some(reason) = &run.stopped {
        ui.add(
            egui::Label::new(egui::RichText::new(format!("Stopped: {reason}")).color(Tokens::color(tokens.error)))
                .wrap(),
        );
    }
    if run.rescan_recommended {
        ui.colored_label(Tokens::color(tokens.warning), "Scan again to refresh the tree.");
    }
    let problems: Vec<&RunItem> =
        run.results.iter().filter(|r| r.status != "done" && r.status != "ok").take(20).collect();
    if !problems.is_empty() {
        egui::ScrollArea::vertical().id_salt("cleanup_run").max_height(120.0).show(ui, |ui| {
            for r in problems {
                ui.horizontal(|ui| {
                    ui.colored_label(status_color(&r.status, tokens), &r.status);
                    crate::text_fit::path_label(ui, &r.path, tokens.font_small, None);
                });
            }
        });
    }
}

fn form_ui(ui: &mut egui::Ui, state: &mut CleanupsState, tokens: &Tokens) -> Option<CleanupsAction> {
    let muted = Tokens::color(tokens.text_muted);
    let mut action = None;
    let form = state.form.as_mut()?;
    ui.heading(if form.editing.is_some() { "Edit cleanup" } else { "New cleanup" });
    egui::Grid::new("cleanup_form").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(380.0));
        ui.end_row();
        if form.editing.is_none() {
            ui.label("Id");
            ui.add(
                egui::TextEdit::singleline(&mut form.id).desired_width(380.0).hint_text("optional, made from the name"),
            );
            ui.end_row();
        }
        ui.label("Action");
        egui::ComboBox::from_id_salt("cleanup_action").selected_text(form.action.label()).show_ui(ui, |ui| {
            for a in ActionChoice::ALL {
                ui.selectable_value(&mut form.action, a, a.label());
            }
        });
        ui.end_row();
        ui.label("Kind");
        egui::ComboBox::from_id_salt("cleanup_kind").selected_text(form.kind.label()).show_ui(ui, |ui| {
            for k in KindChoice::ALL {
                ui.selectable_value(&mut form.kind, k, k.label());
            }
        });
        ui.end_row();
        ui.label("Names");
        ui.add(egui::TextEdit::singleline(&mut form.names).desired_width(380.0).hint_text("*.tmp, ~*, node_modules"));
        ui.end_row();
        ui.label("Extensions");
        ui.add(egui::TextEdit::singleline(&mut form.extensions).desired_width(380.0).hint_text("log, bak"));
        ui.end_row();
        ui.label("Minimum size");
        ui.add(egui::TextEdit::singleline(&mut form.min_size).desired_width(120.0).hint_text("10 MB"));
        ui.end_row();
        ui.label("Maximum size");
        ui.add(egui::TextEdit::singleline(&mut form.max_size).desired_width(120.0));
        ui.end_row();
        ui.label("Older than (days)");
        ui.add(egui::TextEdit::singleline(&mut form.older_than_days).desired_width(120.0));
        ui.end_row();
        ui.label("Only under");
        ui.add(
            egui::TextEdit::singleline(&mut form.only_under)
                .desired_width(380.0)
                .hint_text("absolute folder inside the scan"),
        );
        ui.end_row();
        ui.label("Empty folders");
        ui.checkbox(&mut form.empty, "Only folders that contain no files (needs Folders only)");
        ui.end_row();
    });
    ui.add(egui::Label::new(egui::RichText::new("Names use * and ?; the selector needs at least one criterion. A cleanup never runs until you preview and confirm it.").color(muted)).wrap());
    if let Some(error) = &form.error {
        ui.add(egui::Label::new(egui::RichText::new(error).color(Tokens::color(tokens.error))).wrap());
    }
    ui.horizontal(|ui| {
        if ui.button("Save").clicked() {
            action = Some(CleanupsAction::Save);
        }
        if ui.button("Cancel").clicked() {
            action = Some(CleanupsAction::CancelEdit);
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_safe() {
        assert_eq!(slugify("Old Logs (30d)!"), "old-logs-30d");
        assert_eq!(slugify("***"), "");
        assert!(slugify(&"a".repeat(200)).chars().count() <= 64);
    }

    #[test]
    fn form_builds_a_definition_and_reports_bad_input() {
        let form = CleanupForm {
            name: "Old logs".into(),
            action: ActionChoice::Recycle,
            extensions: ".log, bak".into(),
            min_size: "1 MB".into(),
            kind: KindChoice::File,
            older_than_days: "30".into(),
            ..CleanupForm::default()
        };
        let def = build_definition(&form).unwrap_or(Value::Null);
        assert_eq!(def["id"], "old-logs");
        assert_eq!(def["action"], "recycle");
        assert_eq!(def["selector"]["extensions"], json!(["log", "bak"]));
        assert_eq!(def["selector"]["min_size"], 1_048_576);
        assert_eq!(def["selector"]["older_than_days"], 30);
        assert_eq!(def["selector"]["kind"], "file");
        for (edit, needle) in [
            (CleanupForm { name: "  ".into(), ..form.clone() }, "name"),
            (CleanupForm { min_size: "lots".into(), ..form.clone() }, "Minimum size"),
            (CleanupForm { max_size: "1 parsec".into(), ..form.clone() }, "Maximum size"),
            (CleanupForm { older_than_days: "-3".into(), ..form.clone() }, "whole number"),
            (CleanupForm { name: "***".into(), ..form.clone() }, "id"),
        ] {
            let err = build_definition(&edit).err().unwrap_or_default();
            assert!(err.contains(needle), "{err}");
        }
    }

    #[test]
    fn form_round_trips_an_entry() {
        let entry = CleanupEntry {
            id: "mine".into(),
            name: "Mine".into(),
            action: "permanent".into(),
            builtin: false,
            selector: json!({"names": ["*.tmp"], "kind": "file", "max_size": 2048, "older_than_days": 7}),
        };
        let form = form_from_entry(&entry);
        assert_eq!((form.action, form.kind), (ActionChoice::Permanent, KindChoice::File));
        let def = build_definition(&form).unwrap_or(Value::Null);
        assert_eq!(def["id"], "mine");
        assert_eq!(def["selector"]["max_size"], 2048);
        assert!(selector_summary(&entry.selector).contains("files only"));
        assert_eq!(selector_summary(&json!({})), "no criteria");
    }
}
