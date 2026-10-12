//! L0: fast NTFS MFT parsing (the "WizTree-style" path).
//!
//! Status: stub (milestone M3). Will parse MFT records read from a raw volume. This crate is the only
//! place in the workspace where `unsafe` is allowed (ADR 0002): Win32 FFI only (`CreateFileW`,
//! `DeviceIoControl`, volume reads). Record parsing is safe code only, with a fuzz target.

/// Result of probing for the fast path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MftSupport {
    /// The platform or privileges do not allow reading the MFT; use the portable walk.
    Unavailable,
}

/// Checks whether the fast path is available for a volume. Always `Unavailable` for now.
pub fn probe(_volume: &str) -> MftSupport {
    MftSupport::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_unavailable() {
        assert_eq!(probe("C:"), MftSupport::Unavailable);
    }
}
