//! The snapshot comparison window (`ui.compare`, Tools > Compare with snapshot): compares a loaded
//! snapshot with the current tree through `snapshot.compare` and lists the biggest changes. A click
//! on a row selects the node in the tree and the treemap when it exists in the current tree. The
//! comparison runs on a worker thread (the app spawns it); the window only shows state.

use crate::app::human_size;
use crate::theme::Tokens;
use dirustat_engine::{CommandError, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const ROW_H: f32 = 20.0;
/// Entries requested from the engine.
const LIMIT: u64 = 500;

/// A loaded snapshot offered as the base of a comparison.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapInfo {
    pub id: u64,
    pub label: String,
}

/// One changed entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRow {
    pub kind: String,
    pub path: String,
    pub node_kind: String,
    pub delta: i64,
    /// Node id in the current tree; `None` when the entry is not in it (removed entries).
    pub id: Option<u64>,
}

/// A finished comparison.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareResult {
    pub base_label: String,
    pub identical: bool,
    pub truncated: bool,
    pub base_total: u64,
    pub other_total: u64,
    pub added: u64,
    pub removed: u64,
    pub grown: u64,
    pub shrunk: u64,
    /// Revision of the current tree the ids belong to.
    pub revision: u64,
    pub rows: Vec<ChangeRow>,
}

/// State of the window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompareState {
    pub open: bool,
    /// Chosen base snapshot id.
    pub base: Option<u64>,
    /// Folder below the root to compare, e.g. `Users/me`; empty means everything.
    pub prefix: String,
    pub include_dirs: bool,
    pub snapshots: Vec<SnapInfo>,
    pub result: Option<CompareResult>,
    pub error: Option<String>,
    /// A comparison is running on a worker thread.
    #[serde(skip)]
    pub running: bool,
}

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareAction {
    Run,
    /// Open a snapshot file (the app shows the native dialog).
    OpenFile,
    /// Select this node of the current tree.
    Select(u64),
}

/// Reads the list of loaded snapshots (cheap) and keeps `base` valid.
pub fn refresh(session: &mut Session, state: &mut CompareState) {
    let Ok(list) = session.execute("snapshot.list", &json!({})) else { return };
    let items = list.get("snapshots").and_then(Value::as_array).cloned().unwrap_or_default();
    state.snapshots = items
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_u64()?;
            let path = s.get("path").and_then(Value::as_str).unwrap_or_default();
            let name = std::path::Path::new(path).file_name().map_or(path, |n| n.to_str().unwrap_or(path));
            Some(SnapInfo { id, label: format!("#{id} {name}") })
        })
        .collect();
    if !state.snapshots.iter().any(|s| Some(s.id) == state.base) {
        state.base = state.snapshots.first().map(|s| s.id);
    }
}

/// The `snapshot.compare` params for the chosen base; `None` when there is none.
pub fn request(state: &CompareState) -> Option<Value> {
    let base = state.base?;
    let mut params = json!({ "base": base, "other": "live", "limit": LIMIT, "include_dirs": state.include_dirs });
    let prefix = state.prefix.trim();
    if let (false, Some(map)) = (prefix.is_empty(), params.as_object_mut()) {
        map.insert("prefix".to_owned(), json!(prefix));
    }
    Some(params)
}

fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Stores the answer of `snapshot.compare`.
pub fn apply_result(state: &mut CompareState, answer: Result<Value, CommandError>) {
    state.running = false;
    match answer {
        Err(e) => {
            state.error = Some(e.to_string());
            state.result = None;
        }
        Ok(v) => {
            state.error = None;
            let counts = v.get("counts").cloned().unwrap_or(Value::Null);
            let rows = v
                .get("changes")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|c| ChangeRow {
                            kind: c.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            path: c.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            node_kind: c.get("node_kind").and_then(Value::as_str).unwrap_or_default().to_owned(),
                            delta: c.get("delta").and_then(Value::as_i64).unwrap_or(0),
                            id: c.get("new_id").and_then(Value::as_u64),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let label = v.get("base").and_then(|b| b.get("path")).and_then(Value::as_str).unwrap_or("snapshot");
            state.result = Some(CompareResult {
                base_label: label.to_owned(),
                identical: v.get("identical").and_then(Value::as_bool).unwrap_or(false),
                truncated: v.get("truncated").and_then(Value::as_bool).unwrap_or(false),
                base_total: count(&v, "base_total"),
                other_total: count(&v, "other_total"),
                added: count(&counts, "added"),
                removed: count(&counts, "removed"),
                grown: count(&counts, "grown"),
                shrunk: count(&counts, "shrunk"),
                revision: v.get("other").map_or(0, |o| count(o, "revision")),
                rows,
            });
        }
    }
}

/// Selects `id` if the result still belongs to the current tree.
pub fn select(session: &mut Session, state: &mut CompareState, id: u64) {
    let fresh = state.result.as_ref().is_some_and(|r| r.revision == session.revision());
    if !fresh {
        state.error = Some("The tree changed since this comparison; run it again".to_owned());
        return;
    }
    if let Err(e) = session.execute("select.node", &json!({ "id": id })) {
        state.error = Some(e.to_string());
    }
}

fn signed_size(delta: i64) -> String {
    let sign = if delta < 0 { '-' } else { '+' };
    format!("{sign}{}", human_size(delta.unsigned_abs()))
}

/// Draws the window and returns what the user asked for. `selected` is the selected arena index.
pub fn show(
    ctx: &egui::Context,
    state: &mut CompareState,
    selected: Option<usize>,
    tokens: &Tokens,
) -> Option<CompareAction> {
    let mut action = None;
    let mut open = state.open;
    let muted = Tokens::color(tokens.text_muted);
    egui::Window::new("Compare with snapshot").open(&mut open).default_width(760.0).default_height(480.0).show(
        ctx,
        |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "Compares a saved snapshot (older) with the current tree (newer) by path. Read-only.",
                    )
                    .color(muted),
                )
                .wrap(),
            );
            ui.horizontal(|ui| {
                let label = state
                    .snapshots
                    .iter()
                    .find(|s| Some(s.id) == state.base)
                    .map_or("no snapshot loaded".to_owned(), |s| s.label.clone());
                egui::ComboBox::from_id_salt("compare_base").selected_text(label).show_ui(ui, |ui| {
                    for s in &state.snapshots {
                        ui.selectable_value(&mut state.base, Some(s.id), &s.label);
                    }
                });
                if ui.button("Open snapshot\u{2026}").clicked() {
                    action = Some(CompareAction::OpenFile);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Folder");
                ui.add(egui::TextEdit::singleline(&mut state.prefix).hint_text("whole tree").desired_width(240.0));
                ui.checkbox(&mut state.include_dirs, "Include folders");
                let can_run = state.base.is_some() && !state.running;
                if ui.add_enabled(can_run, egui::Button::new("Compare")).clicked() {
                    action = Some(CompareAction::Run);
                }
                if state.running {
                    ui.spinner();
                }
            });
            if let Some(error) = &state.error {
                ui.colored_label(Tokens::color(tokens.error), error);
            }
            let Some(result) = &state.result else {
                ui.colored_label(muted, "Choose a snapshot and press Compare.");
                return;
            };
            if result.identical {
                ui.label(format!("Identical to {}.", result.base_label));
            } else {
                ui.label(format!(
                    "{} added, {} removed, {} grown, {} shrunk. Total {} -> {} ({}).",
                    result.added,
                    result.removed,
                    result.grown,
                    result.shrunk,
                    human_size(result.base_total),
                    human_size(result.other_total),
                    signed_size(
                        i64::try_from(result.other_total)
                            .unwrap_or(i64::MAX)
                            .saturating_sub(i64::try_from(result.base_total).unwrap_or(i64::MAX))
                    ),
                ));
            }
            if result.truncated {
                ui.colored_label(muted, format!("Showing the {} biggest changes.", result.rows.len()));
            }
            ui.separator();
            rows(ui, result, selected, tokens, &mut action);
        },
    );
    state.open = open;
    action
}

fn rows(
    ui: &mut egui::Ui,
    result: &CompareResult,
    selected: Option<usize>,
    tokens: &Tokens,
    action: &mut Option<CompareAction>,
) {
    egui::ScrollArea::vertical().id_salt("compare_rows").auto_shrink([false, false]).show_rows(
        ui,
        ROW_H,
        result.rows.len(),
        |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for row in result.rows.get(range).unwrap_or_default() {
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter_at(rect);
                let is_selected = row.id.is_some_and(|id| selected.is_some_and(|s| u64::try_from(s).ok() == Some(id)));
                if is_selected {
                    painter.rect_filled(rect, 0.0, Tokens::color(tokens.accent).gamma_multiply(0.35));
                } else if resp.hovered() {
                    painter.rect_filled(rect, 0.0, Tokens::color(tokens.surface));
                }
                let color = match row.kind.as_str() {
                    "added" | "grown" => Tokens::color(tokens.warning),
                    _ => Tokens::color(tokens.success),
                };
                let font = egui::FontId::proportional(tokens.font_body);
                painter.text(
                    egui::pos2(rect.min.x + 4.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &row.kind,
                    font.clone(),
                    Tokens::color(tokens.text_muted),
                );
                painter.text(
                    egui::pos2(rect.min.x + 70.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    signed_size(row.delta),
                    font.clone(),
                    color,
                );
                painter.text(
                    egui::pos2(rect.min.x + 160.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &row.path,
                    font,
                    Tokens::color(tokens.text),
                );
                match row.id {
                    Some(id) if resp.clicked() => *action = Some(CompareAction::Select(id)),
                    Some(_) => {
                        resp.on_hover_text(&row.path);
                    }
                    None => {
                        resp.on_hover_text(format!("{} (not in the current tree)", row.path));
                    }
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::{NodeKind, Tree};

    fn tree(extra: bool) -> Tree {
        let mut t = Tree::new("R");
        t.add_child(t.root(), "a.bin", NodeKind::File, 100).unwrap();
        if extra {
            t.add_child(t.root(), "b.bin", NodeKind::File, 900).unwrap();
        }
        t.aggregate();
        t
    }

    #[test]
    fn request_needs_a_base_and_result_maps_rows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.dirsnap");
        let mut session = Session::new();
        session.replace_tree(tree(false));
        let path = file.to_string_lossy().into_owned();
        session.execute("snapshot.save", &json!({ "path": path })).unwrap();
        session.execute("snapshot.load", &json!({ "path": path, "show": false })).unwrap();
        session.replace_tree(tree(true));

        let mut state = CompareState::default();
        assert!(request(&state).is_none());
        refresh(&mut session, &mut state);
        assert_eq!(state.snapshots.len(), 1);
        assert!(state.snapshots[0].label.ends_with("s.dirsnap"));
        let params = request(&state).unwrap();
        let answer = session.execute("snapshot.compare", &params);
        apply_result(&mut state, answer);
        let result = state.result.clone().unwrap();
        assert!(!result.identical && state.error.is_none());
        assert_eq!((result.added, result.removed), (1, 0));
        assert_eq!(result.rows.len(), 1);
        assert_eq!((result.rows[0].path.as_str(), result.rows[0].delta), ("b.bin", 900));
        let id = result.rows[0].id.unwrap();
        select(&mut session, &mut state, id);
        assert!(state.error.is_none());
        assert_eq!(session.selection().map(|n| n.index()), usize::try_from(id).ok());

        // A replaced tree makes the ids stale.
        session.replace_tree(tree(true));
        select(&mut session, &mut state, id);
        assert!(state.error.is_some());
    }

    #[test]
    fn errors_are_shown_and_deltas_are_signed() {
        let mut state = CompareState { running: true, ..CompareState::default() };
        apply_result(&mut state, Err(CommandError::InvalidParams("no loaded snapshot".to_owned())));
        assert!(!state.running && state.error.is_some() && state.result.is_none());
        assert!(signed_size(-2048).starts_with('-') && signed_size(2048).starts_with('+'));
    }
}
