//! Backend selection: the MFT path is tried first and every failure falls back to the portable walk.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dirustat_ntfs::{MemSource, VolumeError};

use super::mft::testimg::{Img, dump, record_at};
use super::mft::{MftScan, scan_mft};
use super::*;

struct Harness {
    pool: rayon::ThreadPool,
    cancel: Arc<AtomicBool>,
    counters: Arc<Counters>,
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::write(dir.path().join("a/f.bin"), vec![0u8; 5000]).unwrap();
        fs::write(dir.path().join("a/b/g.bin"), b"hi").unwrap();
        fs::write(dir.path().join("top.txt"), b"x").unwrap();
        Self {
            pool: rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap(),
            cancel: Arc::new(AtomicBool::new(false)),
            counters: Arc::new(Counters::default()),
            dir,
        }
    }

    fn run(&self, attempt: Option<FastAttempt<'_>>) -> ScanOutcome {
        run_auto(&self.pool, self.dir.path(), &ScanOptions::default(), &self.cancel, &self.counters, attempt)
    }

    fn portable(&self) -> ScanOutcome {
        run(&self.pool, self.dir.path(), &ScanOptions::default(), &self.cancel, &self.counters)
    }
}

fn sample_image() -> (MemSource, dirustat_ntfs::NtfsVolumeData) {
    let mut img = Img::new();
    img.dir(30, 5, "mft-dir");
    img.big(31, 30, "mft-file", 100, 4096);
    img.finish()
}

fn assert_fell_back(h: &Harness, out: &ScanOutcome, reason_contains: &str) {
    let expected = dump(&h.portable().tree);
    assert_eq!(out.backend, ScanBackend::Portable);
    assert!(!out.cancelled);
    assert_eq!(dump(&out.tree), expected, "the fallback must equal the portable scan");
    let reason = out.fallback_reason.as_deref().unwrap_or_default();
    assert!(reason.contains(reason_contains), "{reason:?} should mention {reason_contains:?}");
    assert_eq!(h.counters.fallback_reason.lock().unwrap().as_deref(), out.fallback_reason.as_deref());
    assert_eq!(h.counters.backend.load(Ordering::Relaxed), ScanBackend::Portable.to_raw());
}

#[test]
fn without_an_attempt_the_portable_walk_runs_silently() {
    let h = Harness::new();
    let out = h.run(None);
    assert_eq!((out.backend, out.fallback_reason), (ScanBackend::Portable, None));
}

#[test]
fn access_denied_falls_back_to_the_portable_walk() {
    let h = Harness::new();
    let attempt = || Err(MftError::Volume(VolumeError::AccessDenied));
    let out = h.run(Some(&attempt));
    assert_fell_back(&h, &out, "administrator");
}

#[test]
fn a_non_ntfs_volume_falls_back() {
    let h = Harness::new();
    let attempt = || Err(MftError::Volume(VolumeError::NotNtfs));
    let out = h.run(Some(&attempt));
    assert_fell_back(&h, &out, "not NTFS");
}

#[test]
fn incomplete_mft_extents_fall_back() {
    let h = Harness::new();
    let (mut src, vol) = sample_image();
    let rec0 = Img::record0(1 << 20, 1 << 20, &[dirustat_ntfs::Run { lcn: Some(1), len: 1 }]);
    src.0[record_at(0)..record_at(0) + rec0.len()].copy_from_slice(&rec0);
    let attempt = || scan_mft(&src, &vol, "V:", &h.cancel, &h.counters);
    let out = h.run(Some(&attempt));
    assert_fell_back(&h, &out, "required data");
}

#[test]
fn a_corrupt_mft_falls_back() {
    let h = Harness::new();
    // Every user record has a torn sector trailer.
    let mut img = Img::new();
    for i in 0..30u32 {
        img.small(100 + i, 5, &format!("f{i}"), 1);
    }
    let (mut src2, vol2) = img.finish();
    for n in 100..130 {
        src2.0[record_at(n) + 510] ^= 0xFF;
    }
    let attempt = || scan_mft(&src2, &vol2, "V:", &h.cancel, &h.counters);
    let out = h.run(Some(&attempt));
    assert_fell_back(&h, &out, "corrupt");
    // A trashed record 0 is a parse error and falls back as well.
    let (mut src, vol) = sample_image();
    src.0[record_at(0) + 510] ^= 0xFF;
    let attempt = || scan_mft(&src, &vol, "V:", &h.cancel, &h.counters);
    let out = h.run(Some(&attempt));
    assert_fell_back(&h, &out, "update sequence");
}

#[test]
fn a_panicking_mft_attempt_falls_back() {
    let h = Harness::new();
    let attempt = || -> Result<MftScan, MftError> { panic!("synthetic panic in the MFT path") };
    let out = h.run(Some(&attempt));
    assert_fell_back(&h, &out, "panicked");
}

#[test]
fn a_working_mft_attempt_is_used() {
    let h = Harness::new();
    let (src, vol) = sample_image();
    let attempt = || scan_mft(&src, &vol, "V:", &h.cancel, &h.counters);
    let out = h.run(Some(&attempt));
    assert_eq!((out.backend, out.fallback_reason.as_deref()), (ScanBackend::Mft, None));
    assert!(dump(&out.tree).contains(&"/mft-dir/mft-file|file|100|4096".to_owned()));
    assert_eq!(h.counters.backend.load(Ordering::Relaxed), ScanBackend::Mft.to_raw());
}

#[test]
fn cancellation_survives_a_fallback() {
    let h = Harness::new();
    h.cancel.store(true, Ordering::Relaxed);
    let attempt = || Err(MftError::Volume(VolumeError::AccessDenied));
    let out = h.run(Some(&attempt));
    assert!(out.cancelled);
    assert_eq!(out.backend, ScanBackend::Portable);
    // A cancelled MFT scan stays cancelled and does not start a second scan.
    let (src, vol) = sample_image();
    let attempt = || scan_mft(&src, &vol, "V:", &h.cancel, &h.counters);
    let out = h.run(Some(&attempt));
    assert!(out.cancelled);
    assert_eq!((out.backend, out.fallback_reason), (ScanBackend::Mft, None));
}

#[test]
fn fallback_resets_the_progress_of_the_failed_attempt() {
    let h = Harness::new();
    let attempt = || {
        h.counters.add_bytes(1_000_000);
        h.counters.store_totals(7, 7, 1_000_000, 3);
        Err(MftError::NoRoot)
    };
    let out = h.run(Some(&attempt));
    let root = out.tree.get(out.tree.root()).unwrap();
    let progress = h.counters.bytes.load(Ordering::Relaxed);
    assert_eq!(progress, root.total_size(dirustat_model::SizeMode::Logical));
    assert_eq!(out.fallback_reason.as_deref(), Some("MFT has no usable root directory record"));
}

#[test]
fn start_on_a_folder_never_tries_the_fast_path() {
    let h = Harness::new();
    let handle = start(h.dir.path(), ScanOptions::default()).unwrap();
    let out = handle.join().unwrap();
    assert_eq!((out.backend, out.fallback_reason), (ScanBackend::Portable, None));
}

#[test]
fn volume_roots_are_recognised() {
    assert_eq!(volume_root_of("c:\\").as_deref(), Some("C:"));
    assert_eq!(volume_root_of("D:").as_deref(), Some("D:"));
    assert_eq!(volume_root_of("E:/").as_deref(), Some("E:"));
    for no in ["", "C", "C:\\Users", "C:\\\\", "\\\\?\\C:\\", "1:", "CC:", "é:", "C:x", "/tmp"] {
        assert_eq!(volume_root_of(no), None, "{no:?}");
    }
}

#[test]
fn prefer_fast_path_defaults_on() {
    assert!(ScanOptions::default().prefer_fast_path);
    assert_eq!(ScanBackend::default(), ScanBackend::Portable);
    assert_eq!((ScanBackend::Portable.as_str(), ScanBackend::Mft.as_str()), ("portable", "mft"));
}

/// Scans a real volume root through `start`. Needs administrator rights and a real NTFS `C:`; it reads
/// the whole MFT, so it is never run by default: `cargo test -p dirustat-scan -- --ignored real_volume`.
#[test]
#[ignore = "needs administrator rights and a real NTFS volume"]
fn real_volume_scan_uses_the_mft() {
    let handle = start(Path::new("C:\\"), ScanOptions::default()).unwrap();
    let out = handle.join().unwrap();
    assert_eq!(out.backend, ScanBackend::Mft, "fallback reason: {:?}", out.fallback_reason);
    assert!(out.tree.len() > 1000);
}
