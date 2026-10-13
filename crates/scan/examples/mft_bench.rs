//! Measures the MFT fast path against the portable walk on a whole volume (needs an elevated shell):
//! `cargo run -p dirustat-scan --release --example mft_bench -- <X:\> [runs]`.
//! Only aggregate numbers are printed, never names from the scanned tree.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use dirustat_scan::{ScanOptions, start};

fn run_once(path: &Path, prefer_fast_path: bool) {
    let label = if prefer_fast_path { "auto" } else { "portable" };
    let started = Instant::now();
    let options = ScanOptions { prefer_fast_path, ..ScanOptions::default() };
    match start(path, options).and_then(dirustat_scan::ScanHandle::join) {
        Ok(out) => {
            let secs = started.elapsed().as_secs_f64();
            println!(
                "{label:<8} backend={} nodes={} errors={} {secs:.2}s ({:.0} nodes/s) fallback={:?}",
                out.backend.as_str(),
                out.tree.len(),
                out.tree.get(out.tree.root()).map_or(0, dirustat_model::Node::error_count),
                out.tree.len() as f64 / secs,
                out.fallback_reason
            );
        }
        Err(err) => eprintln!("{label}: {err}"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("usage: mft_bench <X:\\> [runs]");
        return;
    };
    let runs: u32 = args.next().and_then(|r| r.parse().ok()).unwrap_or(3);
    for run in 1..=runs {
        println!("-- run {run}");
        run_once(&path, true);
        run_once(&path, false);
    }
}
