//! Measures the MFT fast path against the portable walk on a whole volume (needs an elevated shell):
//! `cargo run -p dirustat-scan --release --example mft_bench -- <X:\> [runs]`.
//! Only aggregate numbers are printed, never names from the scanned tree.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use dirustat_scan::{MftStats, ScanOptions, start};

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
            if let Some(stats) = &out.mft_stats {
                print_mft_stats(stats);
            }
        }
        Err(err) => eprintln!("{label}: {err}"),
    }
}

fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1e6
}

/// Phase timings of the MFT pass (aggregate numbers only).
fn print_mft_stats(stats: &MftStats) {
    let t = &stats.timings;
    let mib = t.bytes_read as f64 / 1_048_576.0;
    let wall_mib_s = if t.collect_nanos == 0 { 0.0 } else { mib / (t.collect_nanos as f64 / 1e9) };
    println!(
        "    records_seen={} entries={} links={} corrupt={} orphan_links={} cycle_entries={}",
        stats.records_seen, stats.entries, stats.links, stats.corrupt, stats.orphan_links, stats.cycle_entries
    );
    println!(
        "    read   {:>9.1} ms thread time ({} calls, {mib:.0} MiB, {} reader thread(s), {:.0} MiB/s per thread)",
        ms(t.read_nanos),
        t.read_calls,
        t.read_threads,
        t.read_mib_per_sec()
    );
    println!("    wait   {:>9.1} ms (parser idle, waiting for the disk)", ms(t.wait_nanos));
    println!("    parse  {:>9.1} ms   merge {:>9.1} ms", ms(t.parse_nanos), ms(t.merge_nanos));
    println!("    collect{:>9.1} ms ({wall_mib_s:.0} MiB/s of MFT per wall second)", ms(t.collect_nanos));
    println!(
        "    build  {:>9.1} ms (resolve {:.1} ms, nodes+aggregate {:.1} ms)",
        ms(t.build_nanos),
        ms(t.resolve_nanos),
        ms(t.nodes_nanos)
    );
    println!("    total  {:>9.1} ms", ms(t.total_nanos));
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
