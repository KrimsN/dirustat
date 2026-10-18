//! Error type of the platform layer. Every failure is data; nothing here panics.

use std::fmt;

/// Why a platform service failed. `Clone + Eq` so tests and the fake can inject and compare it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlatformError {
    /// The service does not exist on this OS (or is not implemented yet).
    Unsupported(&'static str),
    /// The path does not exist (or vanished before the call).
    NotFound(String),
    /// The path is empty, relative, contains NUL or a quote, or is absurdly long.
    InvalidPath(String),
    /// The OS refused the operation.
    PermissionDenied(String),
    /// Any other OS-level failure, with the operation name and the OS message or code.
    Failed { op: &'static str, message: String },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "unsupported on this platform: {what}"),
            Self::NotFound(p) => write!(f, "not found: {p}"),
            Self::InvalidPath(why) => write!(f, "invalid path: {why}"),
            Self::PermissionDenied(p) => write!(f, "permission denied: {p}"),
            Self::Failed { op, message } => write!(f, "{op} failed: {message}"),
        }
    }
}

impl std::error::Error for PlatformError {}

impl PlatformError {
    /// Maps an I/O error of operation `op` on `path` to the matching variant.
    pub fn from_io(op: &'static str, path: &str, err: &std::io::Error) -> Self {
        match err.kind() {
            std::io::ErrorKind::NotFound => Self::NotFound(path.to_owned()),
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied(path.to_owned()),
            _ => Self::Failed { op, message: err.to_string() },
        }
    }
}

/// Result alias of the platform layer.
pub type Result<T> = std::result::Result<T, PlatformError>;
