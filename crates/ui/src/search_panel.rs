//! The search bar and result list (`Ctrl+F`, `ui.toggle_search`).
//!
//! The panel only edits [`SearchState`]; the search itself is the engine command `search.find`
//! and the result is held by the session (`Session::search_hits`) so the tree and the treemap
//! highlight it. A search runs after a short pause in typing (debounce) or at once on Enter, and
//! at most once per (criteria, tree revision) pair. The engine bounds the work with a time budget
//! and a result cap, so one search cannot stall the UI for long. Everything that comes from the
//! user (query text, sizes, extensions) is validated here and again by the engine; failures are
//! shown inline in plain words.

use crate::app::human_size;
use crate::theme::Tokens;
use dirustat_engine::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Pause after the last edit before a search starts.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Hits requested from the engine (largest first); the engine cap is 10 000.
pub const RESULT_LIMIT: u64 = 500;
/// Rows of the result list that are kept in the serialized state.
pub const MAX_STATE_HITS: usize = 500;

/// How the query text is read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Substring,
    Wildcard,
    Regex,
}

impl SearchMode {
    pub const ALL: [Self; 3] = [Self::Substring, Self::Wildcard, Self::Regex];

    /// The `mode` value of `search.find`.
    pub fn param(self) -> &'static str {
        match self {
            Self::Substring => "substring",
            Self::Wildcard => "wildcard",
            Self::Regex => "regex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Substring => "Contains",
            Self::Wildcard => "Wildcard",
            Self::Regex => "Regex",
        }
    }
}

/// One row of the result list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Arena index of the node (`select.node` takes it).
    pub id: usize,
    pub name: String,
    pub kind: String,
    pub path: String,
    pub size: u64,
}

/// Bookkeeping that is not part of the serialized state: ignored by equality, reset on clone.
#[derive(Debug, Default)]
pub struct SearchRuntime {
    /// Criteria and revision of the last search that ran.
    last_key: Option<(String, u64)>,
    /// Criteria waiting for the debounce, and since when.
    pending: Option<((String, u64), Instant)>,
    /// Run at once (Enter pressed), skipping the debounce.
    force: bool,
    /// The query field takes keyboard focus on the next frame.
    pub focus: bool,
    /// Arena indices highlighted in the tree and the treemap, cached per held result.
    hit_set: HashSet<usize>,
    hit_key: Option<(u64, usize, Option<usize>)>,
}

impl Clone for SearchRuntime {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for SearchRuntime {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for SearchRuntime {}

/// Search criteria typed by the user and the last result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchState {
    /// Whether the search bar is shown.
    pub open: bool,
    pub query: String,
    pub mode: SearchMode,
    pub case_sensitive: bool,
    /// Test the full path instead of the name.
    pub match_path: bool,
    /// Size filters as typed (`10 MB`, `1.5GiB`); empty means no limit.
    pub min_size: String,
    pub max_size: String,
    /// Comma or space separated extensions (`rs, toml`).
    pub extensions: String,
    /// A search ran and its result is shown.
    pub searched: bool,
    pub hits: Vec<SearchHit>,
    pub total_matched: u64,
    pub truncated: bool,
    pub budget_exhausted: bool,
    /// Inline message for invalid input (bad regex, bad size).
    pub error: Option<String>,
    #[serde(skip)]
    pub runtime: SearchRuntime,
}

/// Parses a size such as `10`, `10 KB`, `1.5GiB` into bytes. Units are binary (`KB` = `KiB` =
/// 1024 B), the same as the sizes shown everywhere else. Empty text means "no limit".
pub fn parse_size(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let bad = || format!("\"{}\" is not a size (examples: 500 KB, 1.5 GiB)", text.chars().take(40).collect::<String>());
    let (whole, frac) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() && frac.is_empty() || frac.contains('.') {
        return Err(bad());
    }
    let shift: u32 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        "t" | "tb" | "tib" => 40,
        _ => return Err(bad()),
    };
    let unit_bytes = 1u128 << shift;
    let whole_value: u128 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| bad())? };
    // At most six fraction digits count; the rest is below one byte for every unit.
    let frac_digits: String = frac.chars().take(6).collect();
    let frac_value: u128 = if frac_digits.is_empty() { 0 } else { frac_digits.parse().map_err(|_| bad())? };
    let frac_scale = 10u128.pow(u32::try_from(frac_digits.len()).unwrap_or(6));
    let bytes = whole_value
        .checked_mul(unit_bytes)
        .and_then(|w| frac_value.checked_mul(unit_bytes).map(|f| (w, f / frac_scale)))
        .and_then(|(w, f)| w.checked_add(f))
        .ok_or_else(|| "size is too large".to_owned())?;
    u64::try_from(bytes).map(Some).map_err(|_| "size is too large".to_owned())
}

/// Splits `rs, .toml *.md` into extensions without dots or globs.
pub fn parse_extensions(text: &str) -> Vec<String> {
    text.split([',', ' ', ';', '\t'])
        .map(|e| e.trim().trim_start_matches('*').trim_start_matches('.'))
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Turns an engine error into text for the inline message.
pub fn friendly_error(message: &str) -> String {
    let m = message.strip_prefix("invalid params: ").unwrap_or(message);
    if let Some(rest) = m.strip_prefix("bad regex:") {
        // The regex crate prints the pattern and a caret line first; the reason follows `error:`.
        let reason = rest
            .lines()
            .find_map(|l| l.trim().strip_prefix("error:"))
            .or_else(|| rest.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("syntax error")
            .trim();
        return format!("Invalid regular expression: {reason}");
    }
    if let Some(rest) = m.strip_prefix("bad wildcard:") {
        return format!("Invalid wildcard pattern:{rest}");
    }
    if m.contains("min_size is greater than max_size") {
        return "The minimum size is larger than the maximum size".to_owned();
    }
    m.to_owned()
}

impl SearchState {
    /// True when there is nothing to search for (no text and no filter).
    pub fn is_blank(&self) -> bool {
        self.query.is_empty()
            && self.min_size.trim().is_empty()
            && self.max_size.trim().is_empty()
            && parse_extensions(&self.extensions).is_empty()
    }

    /// The `search.find` parameters for the typed criteria, or the message to show.
    pub fn build_params(&self) -> Result<Value, String> {
        let min = parse_size(&self.min_size).map_err(|e| format!("Minimum size: {e}"))?;
        let max = parse_size(&self.max_size).map_err(|e| format!("Maximum size: {e}"))?;
        let mut params = json!({
            "query": self.query,
            "mode": self.mode.param(),
            "case_sensitive": self.case_sensitive,
            "match_path": self.match_path,
            "limit": RESULT_LIMIT,
        });
        if let Some(obj) = params.as_object_mut() {
            if let Some(v) = min {
                obj.insert("min_size".to_owned(), json!(v));
            }
            if let Some(v) = max {
                obj.insert("max_size".to_owned(), json!(v));
            }
            let ext = parse_extensions(&self.extensions);
            if !ext.is_empty() {
                obj.insert("extensions".to_owned(), json!(ext));
            }
        }
        Ok(params)
    }

    /// Opens or closes the bar; opening focuses the query field.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.runtime.focus = self.open;
    }

    /// Forgets the result and the criteria (the `search.clear` command ran).
    pub fn reset(&mut self) {
        let open = self.open;
        *self = Self { open, ..Self::default() };
        self.runtime.focus = open;
    }

    fn clear_result(&mut self) {
        self.searched = false;
        self.hits.clear();
        self.total_matched = 0;
        self.truncated = false;
        self.budget_exhausted = false;
    }

    /// Asks for an immediate search on the next [`Self::update`] (Enter in the query field).
    pub fn search_now(&mut self) {
        self.runtime.force = true;
    }

    /// Arena indices of the held search result, valid for the current tree revision.
    pub fn hit_set(&self) -> &HashSet<usize> {
        &self.runtime.hit_set
    }

    /// Keeps the highlight set in step with the session's held hits.
    fn sync_hit_set(&mut self, session: &Session) {
        let hits = session.search_hits();
        let key = (session.revision(), hits.len(), hits.first().map(|n| n.index()));
        if self.runtime.hit_key != Some(key) {
            self.runtime.hit_set = hits.iter().map(|n| n.index()).collect();
            self.runtime.hit_key = Some(key);
        }
    }

    /// Runs the search when it is due; returns how long to wait before the next call when a
    /// debounce is pending. Never blocks beyond the engine's own bounded search.
    pub fn update(&mut self, session: &mut Session, now: Instant) -> Option<Duration> {
        let wait = self.step(session, now);
        self.sync_hit_set(session);
        wait
    }

    fn step(&mut self, session: &mut Session, now: Instant) -> Option<Duration> {
        // The held result vanished elsewhere (search.clear from a client): drop the stale list.
        let current = self.runtime.last_key.as_ref().is_some_and(|(_, r)| *r == session.revision());
        if current && self.searched && !self.hits.is_empty() && session.search_hits().is_empty() {
            self.clear_result();
        }
        if !self.open {
            return None;
        }
        let params = match self.build_params() {
            Ok(p) => p,
            Err(message) => {
                self.error = Some(message);
                self.clear_result();
                self.runtime.last_key = None;
                self.runtime.pending = None;
                return None;
            }
        };
        let key = (params.to_string(), session.revision());
        if self.runtime.last_key.as_ref() == Some(&key) {
            self.runtime.pending = None;
            self.runtime.force = false;
            return None;
        }
        let since = match &self.runtime.pending {
            Some((k, t)) if *k == key => *t,
            _ => {
                self.runtime.pending = Some((key.clone(), now));
                now
            }
        };
        let waited = now.saturating_duration_since(since);
        if !self.runtime.force && waited < DEBOUNCE {
            return Some(DEBOUNCE.saturating_sub(waited));
        }
        self.runtime.force = false;
        self.runtime.pending = None;
        self.run(session, &params);
        self.runtime.last_key = Some(key);
        None
    }

    fn run(&mut self, session: &mut Session, params: &Value) {
        if self.is_blank() {
            // Nothing to look for: drop any held result instead of listing the whole tree.
            let _ = session.execute("search.clear", &json!({}));
            self.error = None;
            self.clear_result();
            return;
        }
        match session.execute("search.find", params) {
            Ok(value) => {
                self.error = None;
                self.searched = true;
                self.total_matched = value.get("total_matched").and_then(Value::as_u64).unwrap_or(0);
                self.truncated = value.get("truncated").and_then(Value::as_bool).unwrap_or(false);
                self.budget_exhausted = value.get("budget_exhausted").and_then(Value::as_bool).unwrap_or(false);
                self.hits = value
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(hit_from_json).take(MAX_STATE_HITS).collect())
                    .unwrap_or_default();
            }
            Err(e) => {
                // A failed search must not leave the previous highlight behind.
                let _ = session.execute("search.clear", &json!({}));
                self.clear_result();
                self.error = Some(friendly_error(&e.to_string()));
            }
        }
    }

    /// One-line summary above the result list.
    pub fn summary(&self) -> String {
        if !self.searched {
            return String::new();
        }
        let shown = self.hits.len();
        let mut text = if self.total_matched == 0 {
            "No matches".to_owned()
        } else if u64::try_from(shown).unwrap_or(u64::MAX) < self.total_matched {
            format!("{} matches, the largest {shown} shown", self.total_matched)
        } else if self.total_matched == 1 {
            "1 match".to_owned()
        } else {
            format!("{} matches", self.total_matched)
        };
        if self.budget_exhausted {
            text.push_str(" (search stopped early: time limit)");
        }
        text
    }
}

fn hit_from_json(item: &Value) -> Option<SearchHit> {
    Some(SearchHit {
        id: usize::try_from(item.get("id")?.as_u64()?).ok()?,
        name: item.get("name")?.as_str()?.to_owned(),
        kind: item.get("kind")?.as_str()?.to_owned(),
        path: item.get("path")?.as_str()?.to_owned(),
        size: item.get("size")?.as_u64()?,
    })
}

/// What the user asked for in the search bar this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BarActions {
    pub clear: bool,
    pub close: bool,
}

/// Draws the criteria rows. Edits go straight into `state`.
pub fn show_bar(ui: &mut egui::Ui, state: &mut SearchState, tokens: &Tokens) -> BarActions {
    let mut actions = BarActions::default();
    ui.horizontal(|ui| {
        ui.label("Find:");
        let edit = egui::TextEdit::singleline(&mut state.query)
            .desired_width((ui.available_width() - 440.0).max(120.0))
            .hint_text("Name or pattern");
        let response = ui.add(edit);
        if std::mem::take(&mut state.runtime.focus) {
            response.request_focus();
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            state.search_now();
        }
        egui::ComboBox::from_id_salt("search_mode").selected_text(state.mode.label()).width(90.0).show_ui(ui, |ui| {
            for mode in SearchMode::ALL {
                ui.selectable_value(&mut state.mode, mode, mode.label());
            }
        });
        ui.checkbox(&mut state.case_sensitive, "Match case");
        ui.checkbox(&mut state.match_path, "Full path");
        if ui.button("Clear").on_hover_text("Clear the search and the highlight (search.clear)").clicked() {
            actions.clear = true;
        }
        if ui.button("Close").clicked() {
            actions.close = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Size from:");
        ui.add(egui::TextEdit::singleline(&mut state.min_size).desired_width(80.0).hint_text("e.g. 1 MB"));
        ui.label("to:");
        ui.add(egui::TextEdit::singleline(&mut state.max_size).desired_width(80.0).hint_text("e.g. 2 GB"));
        ui.label("Extensions:");
        ui.add(egui::TextEdit::singleline(&mut state.extensions).desired_width(140.0).hint_text("rs, toml"));
        if let Some(error) = &state.error {
            ui.colored_label(Tokens::color(tokens.error), error);
        }
    });
    actions
}

const ROW_H: f32 = 20.0;
const SIZE_W: f32 = 80.0;
const NAME_W: f32 = 200.0;
const PAD: f32 = 6.0;

/// Draws the result list; returns the arena index of the clicked hit.
pub fn show_results(ui: &mut egui::Ui, state: &SearchState, selected: Option<usize>, tokens: &Tokens) -> Option<usize> {
    ui.horizontal(|ui| {
        ui.colored_label(Tokens::color(tokens.text_muted), state.summary());
    });
    let mut clicked = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, state.hits.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for hit in state.hits.get(range).unwrap_or_default() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let painter = ui.painter_at(rect);
            if selected == Some(hit.id) {
                painter.rect_filled(rect, 0.0, Tokens::color(tokens.accent).gamma_multiply(0.35));
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, Tokens::color(tokens.surface));
            }
            let font = egui::FontId::proportional(tokens.font_body);
            let cy = rect.center().y;
            let name_cell = egui::Rect::from_min_max(rect.min, egui::pos2(rect.min.x + NAME_W, rect.max.y));
            painter.with_clip_rect(name_cell).text(
                egui::pos2(rect.min.x + PAD, cy),
                egui::Align2::LEFT_CENTER,
                &hit.name,
                font.clone(),
                Tokens::color(tokens.text),
            );
            painter.text(
                egui::pos2(rect.min.x + NAME_W + SIZE_W, cy),
                egui::Align2::RIGHT_CENTER,
                human_size(hit.size),
                font.clone(),
                Tokens::color(tokens.text),
            );
            let path_cell =
                egui::Rect::from_min_max(egui::pos2(rect.min.x + NAME_W + SIZE_W + PAD, rect.min.y), rect.max);
            painter.with_clip_rect(path_cell).text(
                egui::pos2(path_cell.min.x + PAD, cy),
                egui::Align2::LEFT_CENTER,
                &hit.path,
                font,
                Tokens::color(tokens.text_muted),
            );
            if resp.clicked() {
                clicked = Some(hit.id);
            }
            resp.on_hover_text(&hit.path);
        }
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::{NodeKind, Tree};

    fn session_with_tree() -> Session {
        let mut t = Tree::new("root");
        let docs = t.add_child(t.root(), "Docs", NodeKind::Dir, 0).unwrap_or(t.root());
        let _ = t.add_child(docs, "Report.TXT", NodeKind::File, 500);
        let _ = t.add_child(docs, "notes.md", NodeKind::File, 50);
        let _ = t.add_child(t.root(), "big.bin", NodeKind::File, 9000);
        let _ = t.add_child(t.root(), "small.bin", NodeKind::File, 10);
        t.aggregate();
        let mut s = Session::new();
        s.replace_tree(t);
        let _ = s.execute("view.size_mode", &json!({"mode": "logical"}));
        s
    }

    fn open_with(query: &str) -> SearchState {
        SearchState { open: true, query: query.to_owned(), ..SearchState::default() }
    }

    /// Runs `update` with a clock that has passed the debounce.
    fn settle(state: &mut SearchState, session: &mut Session) {
        let t0 = Instant::now();
        let _ = state.update(session, t0);
        let _ = state.update(session, t0 + DEBOUNCE + Duration::from_millis(1));
    }

    #[test]
    fn sizes_parse_with_binary_units_and_reject_garbage() {
        assert_eq!(parse_size(""), Ok(None));
        assert_eq!(parse_size("  "), Ok(None));
        assert_eq!(parse_size("500"), Ok(Some(500)));
        assert_eq!(parse_size("10 KB"), Ok(Some(10 * 1024)));
        assert_eq!(parse_size("1.5GiB"), Ok(Some(1_610_612_736)));
        assert_eq!(parse_size("2m"), Ok(Some(2 * 1024 * 1024)));
        assert_eq!(parse_size(".5 k"), Ok(Some(512)));
        for bad in ["abc", "1..2", "12 parsecs", "-5", ".", "1e9", "99999999999999999999999999 TB"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
        assert!(parse_size(&"9".repeat(500)).is_err());
    }

    #[test]
    fn extensions_are_split_and_cleaned() {
        assert_eq!(parse_extensions("rs, .toml *.md;txt"), ["rs", "toml", "md", "txt"]);
        assert!(parse_extensions(" , ").is_empty());
    }

    #[test]
    fn typed_criteria_become_search_find_params() {
        let s = SearchState {
            query: "bin".into(),
            mode: SearchMode::Wildcard,
            min_size: "1 KB".into(),
            extensions: "bin".into(),
            ..SearchState::default()
        };
        let p = s.build_params().unwrap_or_default();
        assert_eq!(p["query"], "bin");
        assert_eq!(p["mode"], "wildcard");
        assert_eq!(p["min_size"], 1024);
        assert_eq!(p["extensions"], json!(["bin"]));
        assert!(p.get("max_size").is_none());
        let bad = SearchState { max_size: "lots".into(), ..SearchState::default() };
        assert!(bad.build_params().is_err_and(|m| m.starts_with("Maximum size")));
    }

    #[test]
    fn search_runs_after_the_debounce_once_per_revision() {
        let mut session = session_with_tree();
        let mut state = open_with("bin");
        let t0 = Instant::now();
        let wait = state.update(&mut session, t0);
        assert!(wait.is_some_and(|w| w <= DEBOUNCE), "debouncing: {wait:?}");
        assert!(!state.searched);
        assert_eq!(state.update(&mut session, t0 + DEBOUNCE + Duration::from_millis(1)), None);
        assert!(state.searched);
        assert_eq!(state.total_matched, 2);
        assert_eq!(state.hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["big.bin", "small.bin"]);
        assert_eq!(state.hits.first().map(|h| h.size), Some(9000));
        assert_eq!(state.hit_set().len(), 2, "the session holds the hits for highlighting");
        assert_eq!(session.search_hits().len(), 2);
        assert_eq!(state.summary(), "2 matches");
        // Unchanged criteria and revision: no second search (the result list would be rebuilt).
        state.hits.clear();
        settle(&mut state, &mut session);
        assert!(state.hits.is_empty());
        // A new tree revision triggers a fresh search.
        let revision = session.revision();
        session.replace_tree(Tree::new("other"));
        assert!(session.revision() > revision);
        settle(&mut state, &mut session);
        assert_eq!(state.total_matched, 0);
        assert_eq!(state.summary(), "No matches");
        assert!(state.hit_set().is_empty());
    }

    #[test]
    fn enter_skips_the_debounce_and_edits_restart_it() {
        let mut session = session_with_tree();
        let mut state = open_with("notes");
        state.search_now();
        assert_eq!(state.update(&mut session, Instant::now()), None);
        assert_eq!(state.total_matched, 1);
        let t0 = Instant::now();
        state.query = "report".into();
        assert!(state.update(&mut session, t0).is_some());
        state.query = "report.".into();
        // The timer restarted with the new criteria.
        assert!(state.update(&mut session, t0 + DEBOUNCE).is_some());
        assert_eq!(state.update(&mut session, t0 + DEBOUNCE * 3), None);
        assert_eq!(state.hits.first().map(|h| h.name.as_str()), Some("Report.TXT"));
    }

    #[test]
    fn invalid_regex_is_a_friendly_inline_message_and_clears_the_highlight() {
        let mut session = session_with_tree();
        let mut state = open_with("bin");
        settle(&mut state, &mut session);
        assert_eq!(session.search_hits().len(), 2);
        state.query = "(unclosed".into();
        state.mode = SearchMode::Regex;
        settle(&mut state, &mut session);
        let message = state.error.clone().unwrap_or_default();
        assert!(message.starts_with("Invalid regular expression: "), "{message}");
        assert!(!message.contains("invalid params") && !message.contains('\n'), "{message}");
        assert!(state.hits.is_empty() && !state.searched);
        assert!(session.search_hits().is_empty(), "a failed search leaves no stale highlight");
        // Fixing the pattern recovers.
        state.query = "^big".into();
        settle(&mut state, &mut session);
        assert_eq!(state.error, None);
        assert_eq!(state.total_matched, 1);
    }

    #[test]
    fn hostile_input_is_rejected_without_panicking() {
        let mut session = session_with_tree();
        for (query, mode) in [
            ("a\u{0}b", SearchMode::Substring),
            ("(a{1000}){1000}", SearchMode::Regex),
            ("[", SearchMode::Wildcard),
            ("\u{202e}\u{fffd}", SearchMode::Regex),
        ] {
            let mut state = SearchState { query: query.to_owned(), mode, ..open_with("") };
            settle(&mut state, &mut session);
            assert!(state.error.is_some() || state.searched, "{query:?}");
        }
        let mut long = open_with(&"x".repeat(600));
        settle(&mut long, &mut session);
        assert!(long.error.as_deref().is_some_and(|e| e.contains("longer")));
        let mut sizes = open_with("a");
        sizes.min_size = "5 GB".into();
        sizes.max_size = "1 KB".into();
        settle(&mut sizes, &mut session);
        assert_eq!(sizes.error.as_deref(), Some("The minimum size is larger than the maximum size"));
    }

    #[test]
    fn blank_criteria_clear_instead_of_listing_everything() {
        let mut session = session_with_tree();
        let mut state = open_with("bin");
        settle(&mut state, &mut session);
        state.query.clear();
        settle(&mut state, &mut session);
        assert!(!state.searched && state.hits.is_empty() && session.search_hits().is_empty());
        // A filter alone is a valid search.
        state.min_size = "100".into();
        settle(&mut state, &mut session);
        assert_eq!(state.hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["big.bin", "Docs", "Report.TXT"]);
    }

    #[test]
    fn clear_from_a_client_drops_the_stale_list_and_reset_forgets_everything() {
        let mut session = session_with_tree();
        let mut state = open_with("bin");
        settle(&mut state, &mut session);
        assert_eq!(state.hits.len(), 2);
        assert!(session.execute("search.clear", &json!({})).is_ok());
        settle(&mut state, &mut session);
        assert!(state.hits.is_empty() && state.hit_set().is_empty());
        state.reset();
        assert!(state.open && state.query.is_empty() && state.runtime.focus);
    }

    #[test]
    fn closed_bar_does_not_search() {
        let mut session = session_with_tree();
        let mut state = SearchState { query: "bin".into(), ..SearchState::default() };
        settle(&mut state, &mut session);
        assert!(!state.searched && session.search_hits().is_empty());
        state.toggle();
        assert!(state.open && state.runtime.focus);
    }

    #[test]
    fn state_round_trips_and_ignores_the_runtime() {
        let mut state = open_with("bin");
        state.mode = SearchMode::Regex;
        state.hits.push(SearchHit { id: 3, name: "a".into(), kind: "file".into(), path: "r/a".into(), size: 1 });
        let json = serde_json::to_string(&state).unwrap_or_default();
        assert!(!json.contains("runtime"));
        let back: Result<SearchState, _> = serde_json::from_str(&json);
        assert_eq!(back.ok(), Some(state));
        let partial: Result<SearchState, _> = serde_json::from_str("{\"open\":true}");
        assert!(partial.is_ok_and(|s| s.open && s.mode == SearchMode::Substring));
    }

    #[test]
    fn friendly_error_rewrites_engine_messages() {
        assert_eq!(
            friendly_error("invalid params: bad regex: regex parse error:\n    (\n    ^\nerror: unclosed group"),
            "Invalid regular expression: unclosed group"
        );
        assert_eq!(friendly_error("invalid params: bad regex: oops"), "Invalid regular expression: oops");
        assert!(friendly_error("invalid params: bad wildcard: x").starts_with("Invalid wildcard pattern"));
        assert_eq!(friendly_error("something else"), "something else");
    }
}
