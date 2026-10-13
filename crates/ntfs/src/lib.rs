//! L0: fast NTFS MFT parsing (the "WizTree-style" path).
//!
//! Status: M3.1 — safe read-only volume access (`Volume`) over an isolated Win32 FFI module. MFT record
//! parsing, fixups and data runs arrive in M3.2. This crate is the only place in the workspace where
//! `unsafe` is allowed (ADR 0002): the FFI lives in `win32.rs` (Windows only), everything else is safe
//! code. On other platforms every volume operation reports `VolumeError::Unsupported`.

mod volume;
#[cfg(windows)]
mod win32;

pub use volume::{MAX_READ_BYTES, NtfsVolumeData, Volume, VolumeError, check_read_range, volume_path};

/// Result of probing for the fast path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MftSupport {
    /// The volume can be opened and is NTFS: the MFT fast path may be used.
    Available,
    /// The platform or privileges do not allow reading the MFT; use the portable walk.
    Unavailable,
}

/// Probes a volume and reports why the fast path is unavailable, if it is.
pub fn probe_detailed(volume: &str) -> Result<NtfsVolumeData, VolumeError> {
    Volume::open(volume).map(|v| *v.data())
}

/// Checks whether the fast path is available for a volume (opens it read-only and closes it again).
pub fn probe(volume: &str) -> MftSupport {
    match probe_detailed(volume) {
        Ok(_) => MftSupport::Available,
        Err(_) => MftSupport::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_rejects_bad_names() {
        assert_eq!(probe("not a volume"), MftSupport::Unavailable);
        assert_eq!(probe_detailed(""), Err(VolumeError::InvalidName));
    }
}
