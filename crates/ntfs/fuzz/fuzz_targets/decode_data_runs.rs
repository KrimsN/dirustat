#![no_main]

use dirustat_ntfs::{ParseLimits, decode_data_runs, extents_from_runs};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(runs) = decode_data_runs(data, &ParseLimits::default()) {
        let _ = extents_from_runs(&runs);
    }
    let wide = ParseLimits { max_total_clusters: u64::MAX, ..ParseLimits::default() };
    let _ = decode_data_runs(data, &wide);
});
