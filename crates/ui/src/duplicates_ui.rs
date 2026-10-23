//! The Duplicate files window (`ui.duplicates`, Tools > Duplicate files): starts the read-only
//! `duplicates.find` job, shows its progress and pages through the groups of identical files.
//! A click on a file selects it in the tree and the treemap (`select.node`). The window never
//! deletes anything: removing a copy goes through the usual Delete command and its confirmation.
//! Nothing here blocks the UI thread: the engine runs the search on its own thread and the window
//! only polls `duplicates.status` once per frame.

use crate::app::human_size;
use crate::search_panel::parse_size;
use crate::theme::Tokens;
use dirustat_engine::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Groups per page.
pub const PAGE_SIZE: usize = 25;
const ROW_H: f32 = 20.0;
/// Default for the minimum size field: small duplicates are rarely worth the attention.
const DEFAULT_MIN_SIZE: &str = "1 MB";

/// One file of a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DupFile {
    pub id: u64,
    pub name: String,
    pub path: String,
}

/// A set of identical files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DupGroup {
    pub size: u64,
    pub wasted: u64,
    pub count: u64,
    pub files: Vec<DupFile>,
}

/// Where the search is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Running {
        stage: String,
        done: u64,
        total: u64,
        bytes_read: u64,
        problems: u64,
    },
    Done,
    Cancelled,
    Failed {
        message: String,
    },
}

/// Totals of a finished search.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DupSummary {
    pub total_groups: u64,
    pub total_files: u64,
    pub wasted_bytes: u64,
    pub kept_groups: u64,
    pub candidates: u64,
    pub bytes_read: u64,
    pub problems: u64,
    pub problem_samples: Vec<String>,
}

/// State of the window; the criteria are text fields like in the search bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicatesState {
    pub open: bool,
    /// Smallest file size, e.g. `1 MB`; empty means 1 byte.
    pub min_size: String,
    /// Optional folder (scan-root spelling) to restrict the search to.
    pub path_prefix: String,
    pub verify: bool,
    pub phase: Phase,
    pub summary: DupSummary,
    /// Index of the first group on the page (a multiple of [`PAGE_SIZE`]).
    pub offset: usize,
    pub groups: Vec<DupGroup>,
    pub error: Option<String>,
    /// Tree revision the shown page was read at; `None` when no page is loaded.
    #[serde(skip)]
    pub loaded_revision: Option<(u64, usize)>,
}

impl Default for DuplicatesState {
    fn default() -> Self {
        Self {
            open: false,
            min_size: DEFAULT_MIN_SIZE.to_owned(),
            path_prefix: String::new(),
            verify: false,
            phase: Phase::Idle,
            summary: DupSummary::default(),
            offset: 0,
            groups: Vec::new(),
            error: None,
            loaded_revision: None,
        }
    }
}

impl DuplicatesState {
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    /// Index of the last page.
    pub fn last_offset(&self) -> usize {
        usize::try_from(self.summary.kept_groups).unwrap_or(usize::MAX).saturating_sub(1) / PAGE_SIZE * PAGE_SIZE
    }

    /// Moves one page back or forward; the page is reloaded on the next frame.
    pub fn turn(&mut self, forward: bool) {
        self.offset = if forward {
            self.offset.saturating_add(PAGE_SIZE).min(self.last_offset())
        } else {
            self.offset.saturating_sub(PAGE_SIZE)
        };
        self.loaded_revision = None;
    }

    fn reset_result(&mut self) {
        self.summary = DupSummary::default();
        self.groups.clear();
        self.offset = 0;
        self.loaded_revision = None;
    }
}

/// What the user did in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplicatesAction {
    Start,
    Cancel,
    Clear,
    Previous,
    Next,
    Select(u64),
}

fn as_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Brings the state up to date with the session: polls the job and reads the page that is shown.
/// Cheap when nothing changed; never waits for the search.
pub fn refresh(session: &mut Session, state: &mut DuplicatesState) {
    let status = match session.execute("duplicates.status", &json!({})) {
        Ok(v) => v,
        Err(e) => {
            state.error = Some(e.to_string());
            return;
        }
    };
    match status.get("state").and_then(Value::as_str).unwrap_or("idle") {
        "running" => {
            let p = status.get("progress").cloned().unwrap_or(Value::Null);
            state.phase = Phase::Running {
                stage: status.get("stage").and_then(Value::as_str).unwrap_or_default().to_owned(),
                done: as_u64(&p, "stage_done"),
                total: as_u64(&p, "stage_total"),
                bytes_read: as_u64(&p, "bytes_read"),
                problems: as_u64(&p, "problems"),
            };
        }
        "done" => {
            if state.phase != Phase::Done {
                state.phase = Phase::Done;
                state.loaded_revision = None;
                let s = status.get("summary").cloned().unwrap_or(Value::Null);
                state.summary = DupSummary {
                    total_groups: as_u64(&s, "total_groups"),
                    total_files: as_u64(&s, "total_files"),
                    wasted_bytes: as_u64(&s, "wasted_bytes"),
                    kept_groups: as_u64(&s, "kept_groups"),
                    candidates: as_u64(&s, "candidates"),
                    bytes_read: as_u64(&s, "bytes_read"),
                    problems: as_u64(&s, "problems"),
                    problem_samples: s
                        .get("problem_samples")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default(),
                };
            }
            load_page(session, state);
        }
        "cancelled" => {
            state.phase = Phase::Cancelled;
            state.reset_result();
        }
        "failed" => {
            let message = status.get("error").and_then(Value::as_str).unwrap_or("the search failed").to_owned();
            state.phase = Phase::Failed { message };
            state.reset_result();
        }
        _ => {
            // Idle: nothing started, or the tree was replaced and the result with it.
            state.phase = Phase::Idle;
            state.reset_result();
        }
    }
}

/// Reads the current page unless it is already loaded for this tree revision.
fn load_page(session: &mut Session, state: &mut DuplicatesState) {
    let key = (session.revision(), state.offset);
    if state.loaded_revision == Some(key) {
        return;
    }
    let params = json!({ "offset": state.offset, "limit": PAGE_SIZE, "revision": key.0 });
    match session.execute("duplicates.results", &params) {
        Ok(v) => {
            state.groups = v
                .get("groups")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(group_from_json).collect())
                .unwrap_or_default();
            state.error = None;
            state.loaded_revision = Some(key);
        }
        Err(e) => {
            state.groups.clear();
            state.error = Some(e.to_string());
            // Do not retry every frame; the next action or status change reloads.
            state.loaded_revision = Some(key);
        }
    }
}

fn group_from_json(g: &Value) -> DupGroup {
    let files = g
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| DupFile {
                    id: as_u64(f, "id"),
                    name: f.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    path: f.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
                })
                .collect()
        })
        .unwrap_or_default();
    DupGroup { size: as_u64(g, "size"), wasted: as_u64(g, "wasted"), count: as_u64(g, "count"), files }
}

/// Carries out an action of the window.
pub fn apply(session: &mut Session, state: &mut DuplicatesState, action: DuplicatesAction) {
    match action {
        DuplicatesAction::Start => start(session, state),
        DuplicatesAction::Cancel => {
            let _ = session.execute("duplicates.cancel", &json!({}));
        }
        DuplicatesAction::Clear => {
            let _ = session.execute("duplicates.clear", &json!({}));
            state.phase = Phase::Idle;
            state.reset_result();
            state.error = None;
        }
        DuplicatesAction::Previous => state.turn(false),
        DuplicatesAction::Next => state.turn(true),
        DuplicatesAction::Select(id) => {
            if let Err(e) = session.execute("select.node", &json!({ "id": id })) {
                state.error = Some(e.to_string());
            }
        }
    }
}

fn start(session: &mut Session, state: &mut DuplicatesState) {
    let min_size = match parse_size(&state.min_size) {
        Ok(v) => v.unwrap_or(1).max(1),
        Err(e) => {
            state.error = Some(e);
            return;
        }
    };
    let prefix = state.path_prefix.trim();
    let params = if prefix.is_empty() {
        json!({ "min_size": min_size, "verify": state.verify })
    } else {
        json!({ "min_size": min_size, "verify": state.verify, "path_prefix": prefix })
    };
    match session.execute("duplicates.find", &params) {
        Ok(_) => {
            state.error = None;
            state.reset_result();
            state.phase =
                Phase::Running { stage: "collecting".to_owned(), done: 0, total: 0, bytes_read: 0, problems: 0 };
        }
        Err(e) => state.error = Some(e.to_string()),
    }
}

/// One line of the list: a group header or a file of the group above it.
enum Row<'a> {
    Header(&'a DupGroup),
    File(&'a DupFile),
}

fn stage_label(stage: &str) -> &'static str {
    match stage {
        "collecting" => "Grouping files by size",
        "partial_hash" => "Hashing the start and end of files",
        "full_hash" => "Hashing whole files",
        "verify" => "Comparing files byte by byte",
        _ => "Finishing",
    }
}

/// Draws the window and returns what the user asked for. `selected` is the arena index of the
/// selected node, if any.
pub fn show(
    ctx: &egui::Context,
    state: &mut DuplicatesState,
    selected: Option<usize>,
    tokens: &Tokens,
) -> Option<DuplicatesAction> {
    let mut action = None;
    let mut open = state.open;
    let muted = Tokens::color(tokens.text_muted);
    egui::Window::new("Duplicate files").open(&mut open).default_width(820.0).default_height(520.0).show(ctx, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "Finds files with identical content. Read-only: nothing is deleted here; select a file and use Delete.",
                )
                .color(muted),
            )
            .wrap(),
        );
        let running = state.is_running();
        ui.horizontal(|ui| {
            ui.label("Minimum size");
            ui.add_enabled(!running, egui::TextEdit::singleline(&mut state.min_size).desired_width(70.0));
            ui.label("Folder");
            ui.add_enabled(
                !running,
                egui::TextEdit::singleline(&mut state.path_prefix)
                    .hint_text("whole scan")
                    .desired_width(220.0),
            );
            ui.add_enabled(!running, egui::Checkbox::new(&mut state.verify, "Compare bytes"))
                .on_hover_text("Also compare the files byte by byte after their hashes match (slower).");
        });
        ui.horizontal(|ui| {
            if running {
                if ui.button("Cancel").clicked() {
                    action = Some(DuplicatesAction::Cancel);
                }
            } else if ui.button("Find duplicates").clicked() {
                action = Some(DuplicatesAction::Start);
            }
            if ui.add_enabled(!running && state.phase != Phase::Idle, egui::Button::new("Clear")).clicked() {
                action = Some(DuplicatesAction::Clear);
            }
        });
        if let Some(error) = &state.error {
            ui.colored_label(Tokens::color(tokens.error), error);
        }
        status_line(ui, state, tokens, &mut action);
        ui.separator();
        list(ui, state, selected, tokens, &mut action);
    });
    state.open = open;
    action
}

fn status_line(ui: &mut egui::Ui, state: &DuplicatesState, tokens: &Tokens, action: &mut Option<DuplicatesAction>) {
    let muted = Tokens::color(tokens.text_muted);
    match &state.phase {
        Phase::Idle => {
            ui.colored_label(muted, "Press Find duplicates to search the scanned tree.");
        }
        Phase::Running { stage, done, total, bytes_read, problems } => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "{}: {done} of {total} files, {} read, {problems} unreadable",
                    stage_label(stage),
                    human_size(*bytes_read)
                ));
            });
        }
        Phase::Cancelled => {
            ui.colored_label(muted, "Cancelled.");
        }
        Phase::Failed { message } => {
            ui.colored_label(Tokens::color(tokens.error), message);
        }
        Phase::Done => {
            let s = &state.summary;
            ui.horizontal_wrapped(|ui| {
                if s.total_groups == 0 {
                    ui.label(format!("No duplicates among {} candidate files.", s.candidates));
                } else {
                    ui.label(format!(
                        "{} groups, {} files, {} wasted.",
                        s.total_groups,
                        s.total_files,
                        human_size(s.wasted_bytes)
                    ));
                }
                if s.total_groups > s.kept_groups {
                    ui.colored_label(muted, format!("Showing the {} largest groups.", s.kept_groups));
                }
                if s.problems > 0 {
                    let note = ui.colored_label(Tokens::color(tokens.error), format!("{} files skipped", s.problems));
                    note.on_hover_text(s.problem_samples.join("\n"));
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(state.offset > 0, egui::Button::new("Previous")).clicked() {
                    *action = Some(DuplicatesAction::Previous);
                }
                let last =
                    state.offset.saturating_add(PAGE_SIZE).min(usize::try_from(s.kept_groups).unwrap_or(usize::MAX));
                let total = usize::try_from(s.kept_groups).unwrap_or(usize::MAX);
                if ui.add_enabled(last < total, egui::Button::new("Next")).clicked() {
                    *action = Some(DuplicatesAction::Next);
                }
                let first = if total == 0 { 0 } else { state.offset.saturating_add(1) };
                ui.colored_label(muted, format!("groups {first}-{last} of {total}"));
            });
        }
    }
}

fn list(
    ui: &mut egui::Ui,
    state: &DuplicatesState,
    selected: Option<usize>,
    tokens: &Tokens,
    action: &mut Option<DuplicatesAction>,
) {
    let rows: Vec<Row<'_>> = state
        .groups
        .iter()
        .flat_map(|g| std::iter::once(Row::Header(g)).chain(g.files.iter().map(Row::File)))
        .collect();
    egui::ScrollArea::vertical().id_salt("duplicates_rows").auto_shrink([false, false]).show_rows(
        ui,
        ROW_H,
        rows.len(),
        |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in rows.get(range).unwrap_or_default() {
                match row {
                    Row::Header(g) => {
                        let text =
                            format!("{} copies of {} - {} wasted", g.count, human_size(g.size), human_size(g.wasted));
                        ui.add_sized(
                            [ui.available_width(), ROW_H],
                            egui::Label::new(egui::RichText::new(text).strong()).truncate(),
                        );
                    }
                    Row::File(f) => {
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let painter = ui.painter_at(rect);
                        if selected.is_some_and(|s| u64::try_from(s).ok() == Some(f.id)) {
                            painter.rect_filled(rect, 0.0, Tokens::color(tokens.accent).gamma_multiply(0.35));
                        } else if resp.hovered() {
                            painter.rect_filled(rect, 0.0, Tokens::color(tokens.surface));
                        }
                        painter.text(
                            egui::pos2(rect.min.x + 18.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &f.path,
                            egui::FontId::proportional(tokens.font_body),
                            Tokens::color(tokens.text),
                        );
                        if resp.clicked() {
                            *action = Some(DuplicatesAction::Select(f.id));
                        }
                        resp.on_hover_text(&f.path);
                    }
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::Tree;

    /// A session over a real folder with two identical files and a different one.
    fn session() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![7u8; 3000]).unwrap();
        std::fs::write(dir.path().join("b.bin"), vec![7u8; 3000]).unwrap();
        std::fs::write(dir.path().join("c.bin"), vec![8u8; 3000]).unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        (dir, s)
    }

    fn settle(session: &mut Session, state: &mut DuplicatesState) {
        for _ in 0..10_000 {
            refresh(session, state);
            if !state.is_running() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the search did not finish");
    }

    #[test]
    fn start_refresh_and_select_through_commands() {
        let (_dir, mut s) = session();
        let mut state = DuplicatesState { min_size: "1".to_owned(), ..DuplicatesState::default() };
        apply(&mut s, &mut state, DuplicatesAction::Start);
        assert!(state.is_running(), "{state:?}");
        settle(&mut s, &mut state);
        assert_eq!(state.phase, Phase::Done);
        assert_eq!((state.summary.total_groups, state.summary.wasted_bytes), (1, 3000));
        assert_eq!(state.groups.len(), 1);
        let group = &state.groups[0];
        assert_eq!((group.count, group.files.len()), (2, 2));
        let id = group.files[0].id;
        apply(&mut s, &mut state, DuplicatesAction::Select(id));
        assert_eq!(s.selection().map(|n| u64::try_from(n.index()).unwrap()), Some(id));
        // A new tree drops the result: the window goes back to idle.
        s.replace_tree(Tree::new("other"));
        refresh(&mut s, &mut state);
        assert_eq!(state.phase, Phase::Idle);
        assert!(state.groups.is_empty());
    }

    #[test]
    fn bad_criteria_are_reported_not_run() {
        let (_dir, mut s) = session();
        let mut state = DuplicatesState { min_size: "lots".to_owned(), ..DuplicatesState::default() };
        apply(&mut s, &mut state, DuplicatesAction::Start);
        assert!(state.error.is_some() && !state.is_running());
        state.min_size = "1 KB".to_owned();
        state.path_prefix = "a\u{0}b".to_owned();
        apply(&mut s, &mut state, DuplicatesAction::Start);
        assert!(state.error.is_some() && !s.duplicates_running());
    }

    #[test]
    fn default_minimum_hides_small_files_and_clear_resets() {
        let (_dir, mut s) = session();
        let mut state = DuplicatesState::default();
        apply(&mut s, &mut state, DuplicatesAction::Start);
        settle(&mut s, &mut state);
        assert_eq!(state.summary.total_groups, 0, "3 KB files are below the 1 MB default");
        apply(&mut s, &mut state, DuplicatesAction::Clear);
        assert_eq!(state.phase, Phase::Idle);
    }

    #[test]
    fn the_window_opens_by_command_and_by_patch() {
        let mut ui = crate::state::UiState::default();
        assert!(!ui.duplicates.open);
        ui.apply_patch(&json!({"duplicates_open": true})).unwrap();
        assert!(ui.duplicates.open);
        assert!(crate::commands::run(&mut ui, "ui.duplicates").is_some());
        assert!(!ui.duplicates.open);
        let state = serde_json::to_value(&ui).unwrap();
        assert_eq!(state["duplicates"]["phase"]["phase"], "idle");
    }

    #[test]
    fn paging_is_clamped() {
        let mut s = DuplicatesState::default();
        s.summary.kept_groups = 60;
        assert_eq!(s.last_offset(), 50);
        s.turn(true);
        s.turn(true);
        s.turn(true);
        assert_eq!(s.offset, 50);
        s.turn(false);
        assert_eq!(s.offset, 25);
        assert_eq!(DuplicatesState::default().last_offset(), 0);
    }
}
