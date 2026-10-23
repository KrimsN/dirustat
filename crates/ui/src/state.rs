//! Serializable UI state: readable and drivable through the control channel.

use crate::theme::ThemeKind;
use serde::{Deserialize, Serialize};

/// Which panels are visible (`view.*` commands toggle them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelVisibility {
    pub tree: bool,
    pub map: bool,
    pub extensions: bool,
    pub status_bar: bool,
}

impl Default for PanelVisibility {
    fn default() -> Self {
        Self { tree: true, map: true, extensions: true, status_bar: true }
    }
}

/// State of the scan as shown in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScanStatus {
    #[default]
    Idle,
    Running {
        dirs: u64,
        files: u64,
        bytes: u64,
        errors: u64,
    },
    Done {
        errors: u64,
    },
    Cancelled,
    Failed {
        message: String,
    },
}

/// Sortable columns of the file-type (extension) list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtSortColumn {
    Extension,
    Files,
    #[default]
    Size,
    Percent,
}

/// Sort order of the file-type list (size descending by default).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtSort {
    pub column: ExtSortColumn,
    pub descending: bool,
}

impl Default for ExtSort {
    fn default() -> Self {
        Self { column: ExtSortColumn::Size, descending: true }
    }
}

/// Everything the shell needs to remember between frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    /// Text in the path field of the top bar.
    pub path_text: String,
    pub panels: PanelVisibility,
    pub scan: ScanStatus,
    pub theme: ThemeKind,
    /// Extension picked in the file-type list (lowercase, empty = files without one); the map
    /// highlights it.
    #[serde(default)]
    pub selected_ext: Option<String>,
    #[serde(default)]
    pub ext_sort: ExtSort,
    /// Directory tree: arena indices of the expanded folders (reset when the tree is replaced).
    #[serde(default)]
    pub tree_expanded: std::collections::BTreeSet<usize>,
    /// Directory tree: sort column and direction.
    #[serde(default)]
    pub tree_sort: crate::tree_panel::TreeSort,
    /// Session revision the expansion set belongs to.
    #[serde(default)]
    pub tree_revision: Option<u64>,
    /// Flattened-row cache of the tree panel; never serialized, ignored by equality.
    #[serde(skip)]
    pub tree_cache: crate::tree_panel::TreeCache,
    /// Interface zoom in percent (`ui.zoom_*` commands), within `MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT`.
    #[serde(default = "default_zoom")]
    pub zoom_percent: u16,
    /// Whether the command palette is open.
    #[serde(default)]
    pub palette_open: bool,
    /// Text typed into the command palette.
    #[serde(default)]
    pub palette_query: String,
    /// Index of the highlighted palette row.
    #[serde(default)]
    pub palette_index: usize,
    /// Last command error shown in the status bar (cleared by the next successful command).
    #[serde(default)]
    pub notice: Option<String>,
    /// Volume (`C:`) whose scan root could use the MFT fast path if the app had administrator
    /// rights; drives the non-modal status-bar hint. Set when a scan starts.
    #[serde(default)]
    pub elevation_hint: Option<String>,
    /// Whether the "administrator rights" explanation window (`ui.relaunch_elevated_help`) is open.
    #[serde(default)]
    pub elevation_help_open: bool,
    /// Search bar criteria and the last result (`ui.toggle_search`, `search.find`).
    #[serde(default)]
    pub search: crate::search_panel::SearchState,
    /// Volumes shown by the drive picker (`list.drives`); read-only for patches.
    #[serde(default)]
    pub drives: Vec<crate::drive_picker::DriveEntry>,
    /// The drive list is up to date; cleared to reload it (`ui.refresh_drives`, scan end).
    #[serde(skip)]
    pub drives_loaded: bool,
    /// Capacity of the volume of the last finished scan (`platform.free_space`).
    #[serde(default)]
    pub free_space: Option<FreeSpace>,
    /// Scan root and tree revision `free_space` was read for.
    #[serde(skip)]
    pub free_key: Option<(String, u64)>,
    /// Whether the preferences window (`ui.preferences`) is open.
    #[serde(default)]
    pub prefs_open: bool,
    /// Text in the "add exclusion" field.
    #[serde(default)]
    pub exclude_input: String,
    /// Inline error of the exclusion editor.
    #[serde(default)]
    pub prefs_error: Option<String>,
    /// The delete confirmation dialog, when open. Read-only for patches: it opens through
    /// `delete_dialog` requests (like pressing Delete) and is confirmed only by a click.
    #[serde(default)]
    pub delete_dialog: Option<crate::delete_dialog::DeleteDialog>,
    /// A request from `ui.set_state` to open or close the delete dialog, handled on the next frame.
    #[serde(skip)]
    pub dialog_request: Option<crate::delete_dialog::DialogRequest>,
    /// A request from `ui.set_state` to open the Cleanups window on a cleanup and run its read-only
    /// preview, handled on the next frame.
    #[serde(skip)]
    pub cleanup_request: Option<String>,
    /// The notice shown in the corner (outcome of a delete or a cleanup run).
    #[serde(default)]
    pub toast: Option<crate::toast::Toast>,
    /// When the toast appeared.
    #[serde(skip)]
    pub toast_since: Option<std::time::Instant>,
    /// The Cleanups window (`ui.cleanups`).
    #[serde(default)]
    pub cleanups: crate::cleanups_ui::CleanupsState,
    /// The Journal window (`ui.journal`).
    #[serde(default)]
    pub journal: crate::journal_ui::JournalState,
    /// The Duplicate files window (`ui.duplicates`).
    #[serde(default)]
    pub duplicates: crate::duplicates_ui::DuplicatesState,
    /// Lifetime bookkeeping of `notice`; not serialized and ignored by equality.
    #[serde(skip)]
    pub notice_clock: NoticeClock,
    /// The lost-selection path already announced in a toast (so it is shown once per loss).
    #[serde(skip)]
    pub lost_announced: Option<String>,
}

/// How long a status-bar notice stays when nothing else clears it.
pub const NOTICE_SECONDS: u64 = 8;

/// When the current notice appeared and which scan phase it belongs to.
#[derive(Debug, Default)]
pub struct NoticeClock {
    seen: Option<(String, std::time::Instant)>,
    phase: Option<u8>,
}

impl Clone for NoticeClock {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for NoticeClock {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for NoticeClock {}

/// Total and free bytes of the scanned volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeSpace {
    pub path: String,
    pub total: u64,
    pub free: u64,
}

/// Status-bar hint shown while [`UiState::elevation_hint`] is set.
pub const ELEVATION_HINT: &str =
    "Fast MFT scan needs administrator rights. Restart as administrator for maximum scan speed.";

/// Smallest accepted interface zoom.
pub const MIN_ZOOM_PERCENT: u16 = 50;
/// Largest accepted interface zoom.
pub const MAX_ZOOM_PERCENT: u16 = 300;
/// Zoom step of `ui.zoom_in` / `ui.zoom_out`.
pub const ZOOM_STEP: u16 = 10;
/// Cap on text fields accepted from a patch.
pub const MAX_TEXT_LEN: usize = 32_768;

fn default_zoom() -> u16 {
    100
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            path_text: String::new(),
            panels: PanelVisibility::default(),
            scan: ScanStatus::default(),
            theme: ThemeKind::default(),
            selected_ext: None,
            ext_sort: ExtSort::default(),
            tree_expanded: std::collections::BTreeSet::new(),
            tree_sort: crate::tree_panel::TreeSort::default(),
            tree_revision: None,
            tree_cache: crate::tree_panel::TreeCache::default(),
            zoom_percent: default_zoom(),
            palette_open: false,
            palette_query: String::new(),
            palette_index: 0,
            notice: None,
            elevation_hint: None,
            elevation_help_open: false,
            search: crate::search_panel::SearchState::default(),
            drives: Vec::new(),
            drives_loaded: false,
            free_space: None,
            free_key: None,
            prefs_open: false,
            exclude_input: String::new(),
            prefs_error: None,
            delete_dialog: None,
            dialog_request: None,
            cleanup_request: None,
            toast: None,
            toast_since: None,
            cleanups: crate::cleanups_ui::CleanupsState::default(),
            journal: crate::journal_ui::JournalState::default(),
            duplicates: crate::duplicates_ui::DuplicatesState::default(),
            notice_clock: NoticeClock::default(),
            lost_announced: None,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelPatch {
    tree: Option<bool>,
    map: Option<bool>,
    extensions: Option<bool>,
    status_bar: Option<bool>,
}

fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Strict patch for `ui.set_state`: unknown fields and out-of-range values are errors.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiPatch {
    path_text: Option<String>,
    panels: Option<PanelPatch>,
    theme: Option<ThemeKind>,
    #[serde(default, deserialize_with = "nullable")]
    selected_ext: Option<Option<String>>,
    ext_sort: Option<ExtSort>,
    tree_sort: Option<crate::tree_panel::TreeSort>,
    zoom_percent: Option<u16>,
    palette_open: Option<bool>,
    palette_query: Option<String>,
    search: Option<SearchPatch>,
    prefs_open: Option<bool>,
    cleanups_open: Option<bool>,
    journal_open: Option<bool>,
    duplicates_open: Option<bool>,
    /// Opens the Cleanups window, selects this cleanup and runs its (read-only) preview.
    cleanups_preview: Option<String>,
    /// Opens (or closes) the delete dialog for the selected node. Never confirms.
    delete_dialog: Option<crate::delete_dialog::DialogRequest>,
}

/// Patch of the search bar criteria (the result fields are outputs and not patchable).
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchPatch {
    open: Option<bool>,
    query: Option<String>,
    mode: Option<crate::search_panel::SearchMode>,
    case_sensitive: Option<bool>,
    match_path: Option<bool>,
    min_size: Option<String>,
    max_size: Option<String>,
    extensions: Option<String>,
}

impl UiState {
    /// Records the fast-path probe result for the scan root: the hint is set only when the root is
    /// a volume root (`volume` is `Some`) and the probe says administrator rights are missing.
    pub fn set_elevation_hint(&mut self, volume: Option<&str>, status: Option<&dirustat_engine::FastPathStatus>) {
        self.elevation_hint = match (volume, status) {
            (Some(v), Some(s)) if s.needs_admin => Some(v.to_owned()),
            _ => None,
        };
    }

    /// Expires the status-bar notice (QA-127): a notice goes away after [`NOTICE_SECONDS`] and as
    /// soon as the scan changes phase (started, finished, cancelled, failed), because the
    /// condition it reported ("wait for the scan to finish") is over. Returns how long until the
    /// shown notice expires, so the caller can schedule a repaint.
    pub fn tick_notice(&mut self, now: std::time::Instant) -> Option<std::time::Duration> {
        let phase = match self.scan {
            ScanStatus::Idle => 0,
            ScanStatus::Running { .. } => 1,
            ScanStatus::Done { .. } => 2,
            ScanStatus::Cancelled => 3,
            ScanStatus::Failed { .. } => 4,
        };
        if self.notice_clock.phase.is_some_and(|p| p != phase) {
            self.notice = None;
        }
        self.notice_clock.phase = Some(phase);
        let Some(text) = &self.notice else {
            self.notice_clock.seen = None;
            return None;
        };
        let since = match &self.notice_clock.seen {
            Some((seen, t)) if seen == text => *t,
            _ => {
                self.notice_clock.seen = Some((text.clone(), now));
                now
            }
        };
        let lifetime = std::time::Duration::from_secs(NOTICE_SECONDS);
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= lifetime {
            self.notice = None;
            self.notice_clock.seen = None;
            return None;
        }
        Some(lifetime.saturating_sub(elapsed))
    }

    /// Applies a `ui.set_state` patch atomically: on any error the state is left unchanged.
    pub fn apply_patch(&mut self, patch: &serde_json::Value) -> Result<(), String> {
        let p = UiPatch::deserialize(patch).map_err(|e| e.to_string())?;
        if let Some(z) = p.zoom_percent
            && !(MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT).contains(&z)
        {
            return Err(format!("zoom_percent must be within {MIN_ZOOM_PERCENT}..={MAX_ZOOM_PERCENT}"));
        }
        let too_long = |s: &Option<String>| s.as_ref().is_some_and(|t| t.len() > MAX_TEXT_LEN);
        let search_texts_long = p
            .search
            .as_ref()
            .is_some_and(|s| [&s.query, &s.min_size, &s.max_size, &s.extensions].into_iter().any(too_long));
        if too_long(&p.path_text)
            || too_long(&p.palette_query)
            || too_long(&p.cleanups_preview)
            || p.selected_ext.iter().any(too_long)
            || search_texts_long
        {
            return Err(format!("text fields are limited to {MAX_TEXT_LEN} bytes"));
        }
        if let Some(s) = p.search {
            let t = &mut self.search;
            t.open = s.open.unwrap_or(t.open);
            t.mode = s.mode.unwrap_or(t.mode);
            t.case_sensitive = s.case_sensitive.unwrap_or(t.case_sensitive);
            t.match_path = s.match_path.unwrap_or(t.match_path);
            if let Some(v) = s.query {
                t.query = v;
            }
            if let Some(v) = s.min_size {
                t.min_size = v;
            }
            if let Some(v) = s.max_size {
                t.max_size = v;
            }
            if let Some(v) = s.extensions {
                t.extensions = v;
            }
        }
        self.prefs_open = p.prefs_open.unwrap_or(self.prefs_open);
        self.cleanups.open = p.cleanups_open.unwrap_or(self.cleanups.open);
        self.journal.open = p.journal_open.unwrap_or(self.journal.open);
        self.duplicates.open = p.duplicates_open.unwrap_or(self.duplicates.open);
        if p.delete_dialog.is_some() {
            self.dialog_request = p.delete_dialog;
        }
        if let Some(id) = p.cleanups_preview {
            self.cleanup_request = Some(id);
        }
        if let Some(v) = p.path_text {
            self.path_text = v;
        }
        if let Some(pp) = p.panels {
            self.panels.tree = pp.tree.unwrap_or(self.panels.tree);
            self.panels.map = pp.map.unwrap_or(self.panels.map);
            self.panels.extensions = pp.extensions.unwrap_or(self.panels.extensions);
            self.panels.status_bar = pp.status_bar.unwrap_or(self.panels.status_bar);
        }
        self.theme = p.theme.unwrap_or(self.theme);
        if let Some(v) = p.selected_ext {
            self.selected_ext = v;
        }
        self.ext_sort = p.ext_sort.unwrap_or(self.ext_sort);
        self.tree_sort = p.tree_sort.unwrap_or(self.tree_sort);
        self.zoom_percent = p.zoom_percent.unwrap_or(self.zoom_percent);
        self.palette_open = p.palette_open.unwrap_or(self.palette_open);
        if let Some(v) = p.palette_query {
            self.palette_query = v;
            self.palette_index = 0;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_through_json() {
        let state = UiState {
            path_text: "C:\\data".into(),
            scan: ScanStatus::Running { dirs: 1, files: 2, bytes: 3, errors: 4 },
            ..UiState::default()
        };
        let json = serde_json::to_string(&state).unwrap_or_default();
        let back: Result<UiState, _> = serde_json::from_str(&json);
        assert_eq!(back.ok(), Some(state));
    }

    #[test]
    fn new_panels_round_trip_and_patch_atomically() {
        let mut state = UiState::default();
        state.search.open = true;
        state.search.query = "x".into();
        state.drives.push(crate::drive_picker::DriveEntry {
            path: "C:\\".into(),
            label: "L".into(),
            total: 9,
            free: 3,
        });
        state.free_space = Some(FreeSpace { path: "C:\\".into(), total: 9, free: 3 });
        state.prefs_open = true;
        state.exclude_input = "*.tmp".into();
        let json = serde_json::to_string(&state).unwrap_or_default();
        let back: Result<UiState, _> = serde_json::from_str(&json);
        assert_eq!(back.ok(), Some(state.clone()));
        // Old saved states without the new fields still load.
        let old: Result<UiState, _> = serde_json::from_str(
            "{\"path_text\":\"\",\"panels\":{\"tree\":true,\"map\":true,\"extensions\":true,\"status_bar\":true},\
             \"scan\":{\"state\":\"idle\"},\"theme\":\"dark\"}",
        );
        assert!(old.is_ok());
        let ok =
            serde_json::json!({"search": {"open": false, "min_size": "1 MB", "extensions": "rs"}, "prefs_open": false});
        assert!(state.apply_patch(&ok).is_ok());
        assert!(!state.search.open && state.search.min_size == "1 MB" && !state.prefs_open);
        let before = state.clone();
        for bad in [
            serde_json::json!({"search": {"hits": []}, "zoom_percent": 120}),
            serde_json::json!({"search": {"query": "x".repeat(MAX_TEXT_LEN + 1)}}),
            serde_json::json!({"drives": []}),
        ] {
            assert!(state.apply_patch(&bad).is_err(), "{bad}");
            assert_eq!(state, before);
        }
    }

    #[test]
    fn notice_expires_after_its_lifetime() {
        let mut s = UiState::default();
        let t0 = std::time::Instant::now();
        let secs = std::time::Duration::from_secs;
        assert_eq!(s.tick_notice(t0), None);
        s.notice = Some("Wait for the scan to finish".into());
        assert_eq!(s.tick_notice(t0), Some(secs(NOTICE_SECONDS)));
        assert!(s.notice.is_some());
        assert_eq!(s.tick_notice(t0 + secs(NOTICE_SECONDS - 1)), Some(secs(1)));
        assert!(s.notice.is_some());
        assert_eq!(s.tick_notice(t0 + secs(NOTICE_SECONDS)), None);
        assert!(s.notice.is_none());
        // A different notice restarts the clock.
        s.notice = Some("a".into());
        let _ = s.tick_notice(t0 + secs(100));
        s.notice = Some("b".into());
        assert_eq!(s.tick_notice(t0 + secs(105)), Some(secs(NOTICE_SECONDS)));
    }

    #[test]
    fn notice_clears_when_the_scan_phase_changes() {
        let t = std::time::Instant::now();
        let running = |n: u64| ScanStatus::Running { dirs: n, files: n, bytes: n, errors: 0 };
        for (from, to) in [
            (running(1), ScanStatus::Done { errors: 0 }),
            (running(1), ScanStatus::Cancelled),
            (ScanStatus::Done { errors: 0 }, running(0)),
            (ScanStatus::Idle, ScanStatus::Failed { message: "x".into() }),
        ] {
            let mut s = UiState { scan: from, ..UiState::default() };
            let _ = s.tick_notice(t);
            s.notice = Some("The scan root cannot be deleted".into());
            assert!(s.tick_notice(t).is_some());
            assert!(s.notice.is_some(), "stays while nothing changed");
            s.scan = to;
            assert_eq!(s.tick_notice(t), None);
            assert!(s.notice.is_none());
        }
        // Progress updates inside one phase do not clear it.
        let mut s = UiState { scan: running(1), ..UiState::default() };
        let _ = s.tick_notice(t);
        s.notice = Some("n".into());
        s.scan = running(9);
        assert!(s.tick_notice(t).is_some());
    }

    #[test]
    fn patch_applies_and_is_atomic() {
        let mut s = UiState::default();
        let ok =
            serde_json::json!({"panels": {"tree": false}, "theme": "light", "zoom_percent": 120, "selected_ext": "rs"});
        assert!(s.apply_patch(&ok).is_ok());
        assert!(!s.panels.tree && s.panels.map);
        assert_eq!((s.theme, s.zoom_percent, s.selected_ext.as_deref()), (ThemeKind::Light, 120, Some("rs")));
        assert!(s.apply_patch(&serde_json::json!({"selected_ext": null})).is_ok());
        assert_eq!(s.selected_ext, None);
        let before = s.clone();
        let bad = [
            serde_json::json!({"zoom_percent": 5000}),
            serde_json::json!({"zoom_percent": -1}),
            serde_json::json!({"theme": "light", "bogus": 1}),
            serde_json::json!({"panels": {"nope": true}}),
            serde_json::json!({"path_text": "x".repeat(MAX_TEXT_LEN + 1)}),
            serde_json::json!([1, 2]),
            serde_json::json!(null),
            serde_json::json!("x"),
            serde_json::json!({"scan": {"state": "idle"}}),
        ];
        for b in bad {
            assert!(s.apply_patch(&b).is_err(), "{b}");
            assert_eq!(s, before);
        }
    }
}
