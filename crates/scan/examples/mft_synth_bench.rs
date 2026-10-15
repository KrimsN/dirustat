//! CPU-only benchmark of the MFT pass on a synthetic in-memory volume (no administrator rights needed):
//! `cargo run -p dirustat-scan --release --example mft_synth_bench -- [records] [runs]`.
//!
//! The image mimics a volume full of small files: about 15% directories, names of 8..=20 characters,
//! a `$STANDARD_INFORMATION` and a `$FILE_NAME` attribute per record and either a resident or a
//! non-resident unnamed `$DATA`. Everything is deterministic. Disk I/O is a memcpy here, so the numbers
//! isolate parsing, bookkeeping and tree building from the speed of the disk.
#![allow(clippy::print_stdout, clippy::print_stderr, clippy::unwrap_used, clippy::indexing_slicing)]

use std::time::Instant;

use dirustat_ntfs::testutil::{
    NonResident, RecordBuilder, UNIX_EPOCH_FILETIME, encode_runs, file_name, nonresident, resident, standard_info,
    utf16,
};
use dirustat_ntfs::{MemSource, NtfsVolumeData, Run};
use dirustat_scan::{MftStats, bench_scan_mft_mem};

const CLUSTER: usize = 4096;
const RECORD: usize = 1024;
const MFT_OFFSET: usize = CLUSTER;
const SEQ: u16 = 7;

/// Small deterministic generator (xorshift64*), so the image is the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn name(rng: &mut Rng, number: u32, dir: bool) -> String {
    let len = 8 + rng.below(13) as usize;
    let mut s = format!("{number:x}");
    while s.len() < len {
        s.push(char::from(b'a' + u8::try_from(rng.below(26)).unwrap()));
    }
    s.truncate(len);
    if !dir && len > 4 {
        s.replace_range(len - 4.., ".dat");
    }
    s
}

fn build_image(records: u32) -> (MemSource, NtfsVolumeData) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let slots = usize::try_from(records).unwrap().max(32);
    let per_cluster = CLUSTER / RECORD;
    let clusters = slots.div_ceil(per_cluster);
    let size = u64::try_from(clusters * CLUSTER).unwrap();
    let mut disk = vec![0u8; MFT_OFFSET + clusters * CLUSTER + 2 * CLUSTER];

    let mut put = |number: u32, mut b: RecordBuilder| {
        b.number = Some(number);
        let bytes = b.build();
        let at = MFT_OFFSET + usize::try_from(number).unwrap() * RECORD;
        disk[at..at + bytes.len()].copy_from_slice(&bytes);
    };
    let mtime = UNIX_EPOCH_FILETIME + 1_700_000_000 * 10_000_000;
    let base = |number: u32, dir: bool, parent: u32, nm: &str| {
        let mut b = if dir { RecordBuilder::directory() } else { RecordBuilder::file() };
        b.sequence = SEQ;
        b.number = Some(number);
        b.attr(standard_info(mtime, if dir { 0x10 } else { 0x20 })).attr(file_name(
            u64::from(parent),
            SEQ,
            1,
            &utf16(nm),
            0,
            0,
        ))
    };

    let rec0 = {
        let b = base(0, false, 5, "$MFT");
        let runs =
            [Run { lcn: Some(u64::try_from(MFT_OFFSET / CLUSTER).unwrap()), len: u64::try_from(clusters).unwrap() }];
        b.attr(nonresident(&NonResident {
            size,
            allocated: size,
            initialized: size,
            runs: &encode_runs(&runs),
            ..NonResident::default()
        }))
    };
    put(0, rec0);
    put(5, base(5, true, 5, "."));
    let mut dirs: Vec<u32> = vec![5];
    for number in 24..records.max(24) {
        let parent = dirs[usize::try_from(rng.below(dirs.len() as u64)).unwrap()];
        let dir = rng.below(100) < 15;
        let nm = name(&mut rng, number, dir);
        let mut b = base(number, dir, parent, &nm);
        if dir {
            dirs.push(number);
        } else if rng.below(4) == 0 {
            // Resident: the contents live in the record.
            b = b.attr(resident(0x80, &[], 0, &vec![b'x'; usize::try_from(rng.below(300)).unwrap()]));
        } else {
            let sz = rng.below(1 << 22) + 1;
            let alloc = sz.div_ceil(4096) * 4096;
            b = b.attr(nonresident(&NonResident {
                size: sz,
                allocated: alloc,
                initialized: sz,
                runs: &encode_runs(&[Run { lcn: Some(100), len: alloc / 4096 }]),
                ..NonResident::default()
            }));
        }
        put(number, b);
    }
    let volume = NtfsVolumeData {
        volume_serial_number: 1,
        bytes_per_sector: 512,
        bytes_per_cluster: u32::try_from(CLUSTER).unwrap(),
        bytes_per_file_record: u32::try_from(RECORD).unwrap(),
        total_clusters: u64::try_from(disk.len() / CLUSTER).unwrap(),
        total_bytes: u64::try_from(disk.len()).unwrap(),
        mft_valid_data_length: size,
        mft_start_lcn: u64::try_from(MFT_OFFSET / CLUSTER).unwrap(),
    };
    (MemSource(disk), volume)
}

fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1e6
}

fn report(stats: &MftStats, nodes: usize) {
    let t = &stats.timings;
    println!("  nodes={nodes} records_seen={} entries={} links={}", stats.records_seen, stats.entries, stats.links);
    println!(
        "  read     {:>9.1} ms  ({} calls, {:.0} MiB, {} reader thread(s), {:.0} MiB/s per thread inside read_at)",
        ms(t.read_nanos),
        t.read_calls,
        t.bytes_read as f64 / 1_048_576.0,
        t.read_threads,
        t.read_mib_per_sec()
    );
    println!("  wait     {:>9.1} ms  (consumer idle, waiting for a chunk)", ms(t.wait_nanos));
    println!("  parse    {:>9.1} ms", ms(t.parse_nanos));
    println!("  merge    {:>9.1} ms", ms(t.merge_nanos));
    println!("  collect  {:>9.1} ms  (all of phase 1)", ms(t.collect_nanos));
    println!(
        "  build    {:>9.1} ms  (phase 2: resolve {:.1} ms, nodes+aggregate {:.1} ms)",
        ms(t.build_nanos),
        ms(t.resolve_nanos),
        ms(t.nodes_nanos)
    );
    println!("  TOTAL    {:>9.1} ms", ms(t.total_nanos));
}

fn main() {
    let mut args = std::env::args().skip(1);
    let records: u32 = args.next().and_then(|a| a.replace('_', "").parse().ok()).unwrap_or(1_000_000);
    let runs: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(3);
    let started = Instant::now();
    let (image, volume) = build_image(records);
    println!(
        "image: {records} records, {} MiB, built in {:.1}s",
        image.0.len() / (1 << 20),
        started.elapsed().as_secs_f64()
    );
    for run in 1..=runs {
        println!("-- run {run}");
        // The image is cloned so every run starts with the same cold-ish copy; the clone is not timed.
        let copy = MemSource(image.0.clone());
        match bench_scan_mft_mem(copy, &volume) {
            Ok(scan) => report(&scan.stats, scan.outcome.tree.len()),
            Err(e) => eprintln!("scan failed: {e}"),
        }
    }
}
