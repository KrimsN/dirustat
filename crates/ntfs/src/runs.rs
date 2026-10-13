//! Data run lists (the mapping of a non-resident attribute to clusters) and extent helpers.

use crate::bytes::{le_int, le_uint, slice_at};
use crate::error::{ParseError, ParseLimits};

/// One run of a non-resident attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// First logical cluster of the run on the volume; `None` for a sparse hole.
    pub lcn: Option<u64>,
    /// Length in clusters, always > 0.
    pub len: u64,
}

/// Decodes a data run list. Decoding stops at the `0` terminator or at the end of `bytes`.
///
/// Relative cluster offsets are signed and accumulate; a negative or overflowing absolute cluster, a zero
/// length, a malformed header or exceeding the caps in `limits` is an error.
pub fn decode_data_runs(bytes: &[u8], limits: &ParseLimits) -> Result<Vec<Run>, ParseError> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    let mut total: u64 = 0;
    while let Some(&header) = bytes.get(pos) {
        if header == 0 {
            break;
        }
        pos = pos.checked_add(1).ok_or(ParseError::RunOverflow)?;
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return Err(ParseError::BadRuns);
        }
        let len_bytes = slice_at(bytes, pos, len_size).ok_or(ParseError::BadRuns)?;
        pos = pos.checked_add(len_size).ok_or(ParseError::RunOverflow)?;
        let off_bytes = slice_at(bytes, pos, off_size).ok_or(ParseError::BadRuns)?;
        pos = pos.checked_add(off_size).ok_or(ParseError::RunOverflow)?;

        let len = le_uint(len_bytes);
        if len == 0 {
            return Err(ParseError::ZeroLengthRun);
        }
        if runs.len() >= limits.max_runs {
            return Err(ParseError::TooManyRuns);
        }
        total = total.checked_add(len).ok_or(ParseError::RunOverflow)?;
        if total > limits.max_total_clusters {
            return Err(ParseError::TooManyClusters);
        }
        let run_lcn = if off_size == 0 {
            None
        } else {
            lcn = lcn.checked_add(le_int(off_bytes)).ok_or(ParseError::RunOverflow)?;
            let absolute = u64::try_from(lcn).map_err(|_| ParseError::NegativeLcn)?;
            absolute.checked_add(len).ok_or(ParseError::RunOverflow)?;
            Some(absolute)
        };
        runs.push(Run { lcn: run_lcn, len });
    }
    Ok(runs)
}

/// A contiguous piece of a file on the volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// First virtual cluster (position inside the file) of the extent.
    pub vcn: u64,
    /// First logical cluster on the volume.
    pub lcn: u64,
    /// Length in clusters, > 0.
    pub clusters: u64,
}

impl Extent {
    /// Byte offset on the volume, `None` on overflow.
    pub fn byte_offset(&self, bytes_per_cluster: u32) -> Option<u64> {
        self.lcn.checked_mul(u64::from(bytes_per_cluster))
    }

    /// Length in bytes, `None` on overflow.
    pub fn byte_len(&self, bytes_per_cluster: u32) -> Option<u64> {
        self.clusters.checked_mul(u64::from(bytes_per_cluster))
    }
}

/// Converts runs into extents with running virtual cluster numbers. Sparse runs are an error: the `$MFT`
/// itself is never sparse.
pub fn extents_from_runs(runs: &[Run]) -> Result<Vec<Extent>, ParseError> {
    let mut vcn = 0u64;
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        let lcn = run.lcn.ok_or(ParseError::BadRuns)?;
        out.push(Extent { vcn, lcn, clusters: run.len });
        vcn = vcn.checked_add(run.len).ok_or(ParseError::RunOverflow)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> ParseLimits {
        ParseLimits::default()
    }

    fn big() -> ParseLimits {
        ParseLimits { max_total_clusters: u64::MAX, ..lim() }
    }

    #[test]
    fn decodes_simple_and_relative_runs() {
        // (len 8, lcn 0x10), then (len 4, lcn 0x10 - 2), then sparse len 3.
        let bytes = [0x11, 0x08, 0x10, 0x11, 0x04, 0xFE, 0x01, 0x03, 0x00];
        assert_eq!(
            decode_data_runs(&bytes, &lim()),
            Ok(vec![Run { lcn: Some(0x10), len: 8 }, Run { lcn: Some(0x0E), len: 4 }, Run { lcn: None, len: 3 },])
        );
    }

    #[test]
    fn empty_and_terminator_only_lists_are_empty() {
        assert_eq!(decode_data_runs(&[], &lim()), Ok(vec![]));
        assert_eq!(decode_data_runs(&[0], &lim()), Ok(vec![]));
    }

    #[test]
    fn missing_terminator_is_accepted() {
        assert_eq!(decode_data_runs(&[0x11, 0x02, 0x05], &lim()), Ok(vec![Run { lcn: Some(5), len: 2 }]));
    }

    #[test]
    fn rejects_zero_length_run() {
        assert_eq!(decode_data_runs(&[0x11, 0x00, 0x05], &lim()), Err(ParseError::ZeroLengthRun));
    }

    #[test]
    fn rejects_negative_lcn() {
        assert_eq!(decode_data_runs(&[0x11, 0x01, 0xFF], &lim()), Err(ParseError::NegativeLcn));
        // Goes negative only on the second run.
        assert_eq!(decode_data_runs(&[0x11, 0x01, 0x02, 0x11, 0x01, 0xFB], &lim()), Err(ParseError::NegativeLcn));
    }

    #[test]
    fn rejects_truncated_and_oversized_fields() {
        assert_eq!(decode_data_runs(&[0x11, 0x01], &lim()), Err(ParseError::BadRuns));
        assert_eq!(decode_data_runs(&[0x21, 0x01, 0x05], &lim()), Err(ParseError::BadRuns));
        assert_eq!(decode_data_runs(&[0x09, 0x01], &lim()), Err(ParseError::BadRuns));
        assert_eq!(decode_data_runs(&[0x91, 0x01], &lim()), Err(ParseError::BadRuns));
        assert_eq!(decode_data_runs(&[0x10, 0x01], &lim()), Err(ParseError::BadRuns), "zero length size");
    }

    #[test]
    fn rejects_overflow() {
        // Length u64::MAX exceeds the default cluster cap.
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&[0xFF; 8]);
        assert_eq!(decode_data_runs(&bytes, &lim()), Err(ParseError::TooManyClusters));
        // lcn + len overflows u64.
        let mut bytes = vec![0x88];
        bytes.extend_from_slice(&[0xFF; 8]);
        bytes.extend_from_slice(&[0x7F, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode_data_runs(&bytes, &big()), Err(ParseError::RunOverflow));
        // Two runs whose lengths sum past u64::MAX.
        let mut bytes = vec![0x88];
        bytes.extend_from_slice(&[0xFF; 8]);
        bytes.extend_from_slice(&[0x01; 8]);
        bytes.extend_from_slice(&[0x11, 0x02, 0x01]);
        assert!(decode_data_runs(&bytes, &big()).is_err());
        // Relative offsets that overflow i64.
        let mut bytes = vec![0x81, 0x01];
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
        bytes.extend_from_slice(&[0x81, 0x01]);
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F]);
        assert!(decode_data_runs(&bytes, &big()).is_err());
    }

    #[test]
    fn enforces_run_count_cap() {
        let limits = ParseLimits { max_runs: 2, ..lim() };
        let bytes = [0x11, 0x01, 0x01, 0x11, 0x01, 0x01, 0x11, 0x01, 0x01];
        assert_eq!(decode_data_runs(&bytes, &limits), Err(ParseError::TooManyRuns));
        let two: Vec<u8> = bytes.iter().copied().take(6).collect();
        assert_eq!(decode_data_runs(&two, &limits).map(|r| r.len()), Ok(2));
    }

    #[test]
    fn eight_byte_fields_work() {
        let mut bytes = vec![0x88];
        bytes.extend_from_slice(&5u64.to_le_bytes());
        bytes.extend_from_slice(&0x1234i64.to_le_bytes());
        assert_eq!(decode_data_runs(&bytes, &big()), Ok(vec![Run { lcn: Some(0x1234), len: 5 }]));
    }

    #[test]
    fn extents_follow_vcn_and_reject_sparse() {
        let runs = [Run { lcn: Some(100), len: 4 }, Run { lcn: Some(10), len: 2 }];
        assert_eq!(
            extents_from_runs(&runs),
            Ok(vec![Extent { vcn: 0, lcn: 100, clusters: 4 }, Extent { vcn: 4, lcn: 10, clusters: 2 }])
        );
        assert_eq!(extents_from_runs(&[Run { lcn: None, len: 1 }]), Err(ParseError::BadRuns));
        let e = Extent { vcn: 0, lcn: u64::MAX, clusters: 2 };
        assert_eq!(e.byte_offset(4096), None);
        assert_eq!(e.byte_len(4096), Some(8192));
    }
}
