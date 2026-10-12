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

/// Everything the shell needs to remember between frames.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    /// Text in the path field of the top bar.
    pub path_text: String,
    pub panels: PanelVisibility,
    pub scan: ScanStatus,
    pub theme: ThemeKind,
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
