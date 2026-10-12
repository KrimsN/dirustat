//! Measures a portable scan: `cargo run -p dirustat-scan --release --example scan_bench -- <dir> [runs]`.
//! Only aggregate numbers are printed, never names from the scanned tree.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::time::Instant;

use dirustat_scan::{ScanOptions, start};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("usage: scan_bench <dir> [runs]");
        return;
    };
    let runs: u32 = args.next().and_then(|r| r.parse().ok()).unwrap_or(3);
    for run in 1..=runs {
        let started = Instant::now();
        match start(&path, ScanOptions::default()).and_then(dirustat_scan::ScanHandle::join) {
            Ok(out) => {
                let secs = started.elapsed().as_secs_f64();
                println!(
                    "run {run}: {} nodes in {secs:.2}s ({:.0} nodes/s)",
                    out.tree.len(),
                    out.tree.len() as f64 / secs
                );
            }
            Err(err) => eprintln!("run {run}: {err}"),
        }
    }
}
