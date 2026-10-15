//! L0: fast NTFS MFT parsing (the "WizTree-style" path).
//!
//! Status: M3.1 — safe read-only volume access (`Volume`) over an isolated Win32 FFI module; M3.2 — safe
//! MFT record parsing (`apply_fixups`, `parse_record`, `decode_data_runs`, `mft_extents`) and the chunked
//! `MftReader`. This crate is the only place in the workspace where `unsafe` is allowed (ADR 0002): the FFI
//! lives in `win32.rs` (Windows only), everything else is safe code. On other platforms every volume
//! operation reports `VolumeError::Unsupported`, but the parsers work everywhere (and are fuzzable).

mod bytes;
mod error;
mod prefetch;
mod reader;
mod record;
mod runs;
#[cfg(any(test, feature = "testutil"))]
pub mod testutil;
mod volume;
#[cfg(windows)]
mod win32;

pub use error::{ParseError, ParseLimits};
pub use prefetch::{MAX_READ_WORKERS, StreamPlan, StreamSummary, read_mft_stream};
pub use reader::{
    BlockSource, MemSource, MftGeometry, MftItem, MftReader, RawBatch, ReaderIo, ReaderLimits, RecordBatcher,
};
pub use record::{
    ATTR_ATTRIBUTE_LIST, ATTR_DATA, ATTR_FILE_NAME, ATTR_REPARSE_POINT, ATTR_STANDARD_INFORMATION, AttributeList,
    AttributeListEntry, DataStream, FIXUP_STRIDE, FileName, FileRef, MftRecord, NamedStream, Namespace, StandardInfo,
    apply_fixups, mft_extents, parse_raw_record, parse_record,
};
pub use runs::{Extent, Run, decode_data_runs, extents_from_runs};

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
