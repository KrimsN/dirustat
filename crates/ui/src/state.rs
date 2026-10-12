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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
}
