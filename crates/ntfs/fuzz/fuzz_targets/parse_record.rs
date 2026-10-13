#![no_main]

use dirustat_ntfs::{ParseLimits, apply_fixups, mft_extents, parse_raw_record, parse_record};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = ParseLimits::default();
    // Record after fixups (the parser proper).
    if let Ok(record) = parse_record(data, &limits) {
        let _ = mft_extents(&record, 4096);
        let _ = record.best_name();
        let _ = record.link_names();
    }
    // Full path: fixups plus parsing.
    let mut copy = data.to_vec();
    let _ = parse_raw_record(&mut copy, &limits);
    let mut copy = data.to_vec();
    let _ = apply_fixups(&mut copy, 512);
});
