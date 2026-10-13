//! The seam to the UI shell. `ui.*` and `app.quit` are delegated to a [`UiBridge`] implemented by
//! the shell (the control crate knows nothing about egui).

use crate::error::ControlError;
use serde_json::Value;
use std::path::PathBuf;

/// Region of the window to capture, in logical pixels. All values are finite and non-negative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Validated `ui.screenshot` request.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotRequest {
    /// Target PNG path (non-empty, no NUL).
    pub path: PathBuf,
    pub region: Option<Region>,
    /// Replace an existing file. Default false: an existing path is an error. Missing parent
    /// directories are never created.
    pub overwrite: bool,
}

/// UI side of the control channel. Called from server threads, so implementations must be
/// thread-safe (typically a channel to the UI thread plus a bounded wait) and must never block
/// forever. Return [`ControlError::unsupported`] for what the shell cannot do.
pub trait UiBridge: Send + Sync {
    /// Serialized UI state (`ui.state`).
    fn ui_state(&self) -> Result<Value, ControlError>;
    /// Applies a patch object to the UI state (`ui.set_state`); returns the new state or `{}`.
    fn ui_set_state(&self, patch: &Value) -> Result<Value, ControlError>;
    /// Saves a window PNG (`ui.screenshot`); returns e.g. `{"path": ..}`.
    fn ui_screenshot(&self, request: &ScreenshotRequest) -> Result<Value, ControlError>;
    /// Requests a graceful shutdown (`app.quit`).
    fn app_quit(&self) -> Result<Value, ControlError>;
}

/// Bridge for headless use: everything is `unsupported`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullBridge;

impl UiBridge for NullBridge {
    fn ui_state(&self) -> Result<Value, ControlError> {
        Err(ControlError::unsupported("ui.state needs the desktop app"))
    }
    fn ui_set_state(&self, _patch: &Value) -> Result<Value, ControlError> {
        Err(ControlError::unsupported("ui.set_state needs the desktop app"))
    }
    fn ui_screenshot(&self, _request: &ScreenshotRequest) -> Result<Value, ControlError> {
        Err(ControlError::unsupported("ui.screenshot needs the desktop app"))
    }
    fn app_quit(&self) -> Result<Value, ControlError> {
        Err(ControlError::unsupported("app.quit needs the desktop app"))
    }
}
