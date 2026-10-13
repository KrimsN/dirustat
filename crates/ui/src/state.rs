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
}

/// Status-bar hint shown while [`UiState::elevation_hint`] is set.
pub const ELEVATION_HINT: &str =
    "Fast MFT scan needs administrator rights. Restart as administrator for WizTree-class speed.";

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

    /// Applies a `ui.set_state` patch atomically: on any error the state is left unchanged.
    pub fn apply_patch(&mut self, patch: &serde_json::Value) -> Result<(), String> {
        let p = UiPatch::deserialize(patch).map_err(|e| e.to_string())?;
        if let Some(z) = p.zoom_percent
            && !(MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT).contains(&z)
        {
            return Err(format!("zoom_percent must be within {MIN_ZOOM_PERCENT}..={MAX_ZOOM_PERCENT}"));
        }
        let too_long = |s: &Option<String>| s.as_ref().is_some_and(|t| t.len() > MAX_TEXT_LEN);
        if too_long(&p.path_text) || too_long(&p.palette_query) || p.selected_ext.iter().any(too_long) {
            return Err(format!("text fields are limited to {MAX_TEXT_LEN} bytes"));
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
