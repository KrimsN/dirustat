//! Mounted-volume information supplied by the shell (the engine itself uses no platform FFI).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Upper bound on drives reported by a provider.
pub const MAX_DRIVES: usize = 256;

/// One mounted volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveInfo {
    /// Mount point or drive root (`C:\`, `/`).
    pub path: String,
    pub label: String,
    pub total: u64,
    pub free: u64,
}

impl DriveInfo {
    /// Free space never exceeds the total.
    pub(crate) fn sanitized(mut self) -> Self {
        self.free = self.free.min(self.total);
        self
    }
}

/// Callback that lists the volumes; set by the app, empty by default.
pub(crate) struct DrivesProvider(pub(crate) Option<Box<dyn Fn() -> Vec<DriveInfo> + Send>>);

impl fmt::Debug for DrivesProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "DrivesProvider(set)" } else { "DrivesProvider(none)" })
    }
}
