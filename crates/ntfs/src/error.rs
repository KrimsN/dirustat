//! Errors of the safe MFT parsing layer. Every malformed input maps to one of these, never to a panic.

use std::fmt;

/// Why an MFT record, an attribute or a run list could not be parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// A field or attribute extends past the end of the available bytes.
    Truncated,
    /// The record does not start with the `FILE` signature.
    BadMagic,
    /// The update sequence array is out of bounds or a sector's trailer does not match it (torn write).
    BadFixup,
    /// Inconsistent record header (used size, first attribute offset, ...).
    BadHeader,
    /// An attribute header is inconsistent (length, offsets, name, value range).
    BadAttribute,
    /// More attributes than `ParseLimits::max_attributes`.
    TooManyAttributes,
    /// The data run list is malformed (bad header byte, truncated field).
    BadRuns,
    /// A data run has length zero.
    ZeroLengthRun,
    /// Arithmetic on run lengths or cluster numbers overflowed.
    RunOverflow,
    /// A relative cluster offset leads to a negative cluster number.
    NegativeLcn,
    /// More runs than `ParseLimits::max_runs`.
    TooManyRuns,
    /// The runs cover more clusters than `ParseLimits::max_total_clusters`.
    TooManyClusters,
    /// A `$FILE_NAME` attribute carries an unknown namespace.
    BadNamespace,
    /// A size field is larger than any real file can be.
    SizeOverflow,
    /// The information needed is not in this record (for example the `$MFT` runs live in extension records).
    Incomplete,
    /// Sector, cluster or record size is not a sane power of two.
    BadGeometry,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Truncated => "MFT data is truncated",
            Self::BadMagic => "MFT record has no FILE signature",
            Self::BadFixup => "MFT record failed update sequence validation",
            Self::BadHeader => "MFT record header is inconsistent",
            Self::BadAttribute => "MFT attribute is inconsistent",
            Self::TooManyAttributes => "MFT record has too many attributes",
            Self::BadRuns => "data run list is malformed",
            Self::ZeroLengthRun => "data run has zero length",
            Self::RunOverflow => "data run arithmetic overflowed",
            Self::NegativeLcn => "data run points to a negative cluster",
            Self::TooManyRuns => "data run list is too long",
            Self::TooManyClusters => "data runs cover too many clusters",
            Self::BadNamespace => "file name has an unknown namespace",
            Self::SizeOverflow => "size field is implausibly large",
            Self::Incomplete => "required data is not in this MFT record",
            Self::BadGeometry => "invalid sector, cluster or record size",
        })
    }
}

impl std::error::Error for ParseError {}

/// Caps applied while parsing. Every loop and allocation driven by on-disk data is bounded by one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseLimits {
    /// Attributes walked per record. A 64 KiB record holds at most 4096 minimal attributes; real ones have < 30.
    pub max_attributes: usize,
    /// Runs decoded per attribute. Each run needs >= 2 bytes, so a record cannot hold more than ~32 Ki.
    pub max_runs: usize,
    /// Total clusters (sparse included) one run list may describe: 2^40 clusters = 4 PiB at 4 KiB.
    pub max_total_clusters: u64,
    /// `$ATTRIBUTE_LIST` entries parsed per record.
    pub max_list_entries: usize,
    /// Named (alternate) data streams kept in detail; further ones are only counted.
    pub max_named_streams: usize,
    /// Decode the run list of the unnamed `$DATA`. Turn off when only names and sizes are needed.
    pub decode_runs: bool,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_attributes: 1024,
            max_runs: 65_536,
            max_total_clusters: 1 << 40,
            max_list_entries: 65_536,
            max_named_streams: 64,
            decode_runs: true,
        }
    }
}
