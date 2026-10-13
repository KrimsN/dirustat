//! Protocol errors and the stable error codes.

use dirustat_engine::CommandError;
use std::fmt;

/// Command errors from the engine registry (`docs/control-protocol.md`).
pub const UNKNOWN_COMMAND: &str = "unknown_command";
pub const INVALID_PARAMS: &str = "invalid_params";
pub const UNSUPPORTED: &str = "unsupported";
pub const CONFIRMATION_REQUIRED: &str = "confirmation_required";
pub const NOT_ENABLED: &str = "not_enabled";
pub const INTERNAL: &str = "internal";
/// Transport-level codes.
pub const UNKNOWN_METHOD: &str = "unknown_method";
pub const BAD_REQUEST: &str = "bad_request";
pub const UNAUTHORIZED: &str = "unauthorized";
pub const TOO_LARGE: &str = "too_large";
pub const RATE_LIMITED: &str = "rate_limited";

/// An error with a stable machine-readable code and a human-readable message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlError {
    code: String,
    message: String,
}

impl ControlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into() }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(UNSUPPORTED, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(INTERNAL, message)
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<CommandError> for ControlError {
    fn from(err: CommandError) -> Self {
        Self::new(err.code(), err.to_string())
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ControlError {}
