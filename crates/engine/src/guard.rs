//! Last-resort panic guard (G-007). A safety net, not a licence to panic: it turns a panic inside a
//! command or a scan into a handled error and keeps the session alive.

use crate::CommandError;
use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

/// Runs `f`, converting a panic into `CommandError::Panicked`. `what` names the guarded operation.
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, CommandError>) -> Result<T, CommandError> {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let message = panic_message(payload.as_ref());
            tracing::error!(operation = what, %message, "panic caught by the last-resort guard");
            Err(CommandError::Panicked { what: what.to_owned(), message })
        }
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_panic_becomes_error() {
        let r: Result<(), _> = guarded("t", || panic!("boom"));
        assert_eq!(r, Err(CommandError::Panicked { what: "t".into(), message: "boom".into() }));
    }

    #[test]
    fn string_panic_keeps_message() {
        let r: Result<(), _> = guarded("t", || panic!("code {}", 7));
        assert_eq!(r, Err(CommandError::Panicked { what: "t".into(), message: "code 7".into() }));
    }

    #[test]
    fn opaque_payload_is_handled() {
        let r: Result<(), _> = guarded("t", || panic::panic_any(42_u8));
        assert!(matches!(r, Err(CommandError::Panicked { .. })));
    }

    #[test]
    fn normal_results_pass_through() {
        assert_eq!(guarded("t", || Ok(5)), Ok(5));
        let err = CommandError::InvalidParams("x".into());
        assert_eq!(guarded::<()>("t", || Err(err.clone())), Err(err));
    }
}
