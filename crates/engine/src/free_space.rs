//! Free space of the scanned volume as a band next to the treemap (`treemap.free_space`, G-018).
//!
//! When the setting is on and the platform reports the free space of the volume that holds the scan
//! root, the map bounds are split: the hierarchical layout gets the left part and a free-space band
//! the right part, whose width is proportional to `free / (used + free)` (clamped, see [`split_bounds`]).
//! The band is not a tree node, so selection, hit testing and the layout cache are untouched; only the
//! bounds handed to the layout change.
//!
//! The volume is asked on a helper thread (a stuck network or dead drive must not freeze the shell):
//! [`Session::free_space_split`] never blocks, it reports `pending` until the answer is in and asks
//! again whenever the tree is replaced.

use crate::Session;
use crate::cmd::BusyGuard;
use dirustat_platform::SpaceInfo;
use dirustat_treemap::Rect;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};

/// Narrowest band that is still visible, in layout units.
pub const MIN_BAND_WIDTH: f32 = 6.0;
/// The band never takes more than this share of the width, so a nearly empty volume leaves room for
/// the tree.
pub const MAX_BAND_SHARE: f32 = 0.5;
/// Panels narrower than this get no band.
pub const MIN_PANEL_WIDTH: f32 = 64.0;

/// Free space of the volume as shown in the band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FreeSpace {
    /// Bytes free for the current user.
    pub free: u64,
    /// Capacity of the volume.
    pub total: u64,
    /// Bytes counted in the scanned tree (the other side of the proportion).
    pub used: u64,
}

/// The map bounds split for the free-space band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreeSplit {
    /// Bounds to lay the tree out in (all of `bounds` when there is no band).
    pub tree: Rect,
    /// The band and the numbers it stands for; `None` when the setting is off, the volume is unknown
    /// (not answered yet, not a local drive, the query failed), nothing is free or the panel is tiny.
    pub band: Option<(Rect, FreeSpace)>,
    /// The volume query is outstanding: ask again soon.
    pub pending: bool,
}

/// Splits `bounds` for `free` bytes next to `used` bytes: the band is the right-hand strip. Its share
/// of the width is `free / (used + free)`, at least [`MIN_BAND_WIDTH`] and at most [`MAX_BAND_SHARE`].
/// No band (the whole `bounds` for the tree) when `free` is 0, `bounds` is invalid or narrower than
/// [`MIN_PANEL_WIDTH`].
pub fn split_bounds(bounds: Rect, used: u64, free: u64) -> (Rect, Option<Rect>) {
    let valid = bounds.x.is_finite() && bounds.y.is_finite() && bounds.w.is_finite() && bounds.h.is_finite();
    if !valid || bounds.h <= 0.0 || bounds.w < MIN_PANEL_WIDTH || free == 0 {
        return (bounds, None);
    }
    let (u, f) = (to_f64(used), to_f64(free));
    let share = (f / (u + f)).clamp(0.0, f64::from(MAX_BAND_SHARE));
    let width = (bounds.w * narrow(share)).clamp(MIN_BAND_WIDTH, bounds.w * MAX_BAND_SHARE).floor();
    let tree = Rect { w: bounds.w - width, ..bounds };
    let band = Rect { x: bounds.x + tree.w, w: width, ..bounds };
    (tree, Some(band))
}

#[allow(clippy::cast_precision_loss)]
fn to_f64(v: u64) -> f64 {
    v as f64
}

#[allow(clippy::cast_possible_truncation)]
fn narrow(v: f64) -> f32 {
    if v.is_finite() { v as f32 } else { 0.0 }
}

/// What is known about the volume of the current tree.
#[derive(Debug, Default)]
enum Answer {
    #[default]
    NotAsked,
    Pending(mpsc::Receiver<Option<SpaceInfo>>),
    Known(SpaceInfo),
    /// Not a local drive, or the query failed: not retried until the tree is replaced.
    Unavailable,
}

/// Volume state of a session, valid for one tree revision.
#[derive(Debug, Default)]
pub struct VolumeState {
    revision: u64,
    answer: Answer,
    /// Held by the helper thread while the OS has not answered; at most one helper exists.
    busy: Arc<AtomicBool>,
}

impl Session {
    /// Splits `bounds` for the free-space band according to the `treemap.show_free_space` setting.
    /// Never blocks: the first calls after a tree change report `pending` while the helper thread asks
    /// the OS. Free space and the scanned size follow the current size mode.
    pub fn free_space_split(&mut self, bounds: Rect) -> FreeSplit {
        let plain = FreeSplit { tree: bounds, band: None, pending: false };
        // A shown snapshot is a past scan: the volume's free space says nothing about it.
        if !self.settings.treemap.show_free_space || self.snapshot_view().is_some() {
            return plain;
        }
        if self.volume.revision != self.revision {
            self.volume.revision = self.revision;
            self.volume.answer = Answer::NotAsked;
        }
        if matches!(self.volume.answer, Answer::NotAsked) {
            self.volume.answer = self.ask_volume();
        }
        let received = match &self.volume.answer {
            Answer::Pending(rx) => Some(rx.try_recv()),
            _ => None,
        };
        match received {
            Some(Ok(Some(space))) => self.volume.answer = Answer::Known(space),
            // The helper reported a failure, or it died without answering.
            Some(Ok(None) | Err(mpsc::TryRecvError::Disconnected)) => self.volume.answer = Answer::Unavailable,
            Some(Err(mpsc::TryRecvError::Empty)) => return FreeSplit { pending: true, ..plain },
            None => {}
        }
        let Answer::Known(space) = self.volume.answer else { return plain };
        let used = self.tree.get(self.tree.root()).map_or(0, |n| n.total_size(self.map_size_mode()));
        let (tree, band) = split_bounds(bounds, used, space.free);
        let numbers = FreeSpace { free: space.free, total: space.total, used };
        FreeSplit { tree, band: band.map(|b| (b, numbers)), pending: false }
    }

    /// Free space of the volume as last answered (`None` before the answer, for an unknown volume or
    /// with the setting off). Does not start a query.
    pub fn volume_space(&self) -> Option<SpaceInfo> {
        match self.volume.answer {
            Answer::Known(space) if self.volume.revision == self.revision => Some(space),
            _ => None,
        }
    }

    /// Starts the volume query for the scan root, or marks the volume unavailable.
    fn ask_volume(&self) -> Answer {
        let Some(path) = self.node_path(self.tree.root()).filter(|p| crate::is_local_path(p)) else {
            return Answer::Unavailable;
        };
        let Some(claim) = BusyGuard::claim(&self.volume.busy) else { return Answer::Unavailable };
        let (tx, rx) = mpsc::channel();
        let platform = self.platform();
        let spawned = std::thread::Builder::new().name("dirustat-volume-space".to_owned()).spawn(move || {
            let answer = platform.free_space(&PathBuf::from(path)).ok();
            drop(claim);
            // A closed receiver only means the tree was replaced meanwhile.
            let _ = tx.send(answer);
        });
        match spawned {
            Ok(_) => Answer::Pending(rx),
            Err(_) => Answer::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::{NodeKind, Tree};
    use dirustat_platform::FakePlatform;
    use std::time::{Duration, Instant};

    fn bounds() -> Rect {
        Rect { x: 0.0, y: 0.0, w: 600.0, h: 400.0 }
    }

    #[test]
    fn band_width_is_proportional_clamped_and_never_overlaps_the_tree() {
        let (tree, band) = split_bounds(bounds(), 500, 100);
        let band = band.unwrap();
        assert_eq!(band.w, 100.0, "100 / 600 of 600 px");
        assert_eq!((band.x, band.y, band.h), (500.0, 0.0, 400.0));
        assert_eq!((tree.x, tree.w, tree.h), (0.0, 500.0, 400.0));
        assert_eq!(tree.x + tree.w, band.x, "the two parts meet exactly");
        // A nearly empty volume leaves at least half of the width to the tree.
        let (tree, band) = split_bounds(bounds(), 1, 1_000_000);
        assert_eq!((tree.w, band.unwrap().w), (300.0, 300.0));
        // A sliver of free space still gets a visible band.
        let (_, band) = split_bounds(bounds(), 1_000_000_000, 1);
        assert_eq!(band.unwrap().w, MIN_BAND_WIDTH);
    }

    #[test]
    fn nothing_free_or_hostile_bounds_give_no_band() {
        assert_eq!(split_bounds(bounds(), 10, 0), (bounds(), None));
        assert_eq!(split_bounds(Rect { w: 40.0, ..bounds() }, 10, 10).1, None);
        for bad in [f32::NAN, f32::INFINITY, -5.0, 0.0] {
            let b = Rect { w: bad, ..bounds() };
            assert_eq!(split_bounds(b, 10, 10).1, None, "w = {bad}");
            let b = Rect { h: bad, ..bounds() };
            assert_eq!(split_bounds(b, 10, 10).1, None, "h = {bad}");
        }
        let (tree, band) = split_bounds(bounds(), u64::MAX, u64::MAX);
        assert_eq!((tree.w, band.map(|b| b.w)), (300.0, Some(300.0)));
    }

    fn session_with_space(free: u64) -> (tempfile::TempDir, Session, Arc<FakePlatform>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.bin"), vec![0_u8; 50]).unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let fake = Arc::new(FakePlatform::new());
        fake.set_space(SpaceInfo { total: 1000, free });
        s.set_platform(fake.clone());
        (dir, s, fake)
    }

    fn settle(s: &mut Session) -> FreeSplit {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let split = s.free_space_split(bounds());
            if !split.pending {
                return split;
            }
            assert!(Instant::now() < deadline, "the volume query never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_setting_gates_the_band_and_the_fake_volume_sizes_it() {
        let (_dir, mut s, _fake) = session_with_space(50);
        s.set_map_size_mode(dirustat_model::SizeMode::Logical);
        let off = s.free_space_split(bounds());
        assert_eq!((off.tree, off.band, off.pending), (bounds(), None, false), "off by default");
        s.execute("treemap.free_space", &serde_json::json!({"enabled": true})).unwrap();
        let split = settle(&mut s);
        let (band, numbers) = split.band.expect("band when the volume answered");
        assert_eq!(numbers, FreeSpace { free: 50, total: 1000, used: 50 });
        assert_eq!(band.w, 300.0, "50 free of 100 -> half of the width");
        assert_eq!(split.tree.w, 300.0);
        assert_eq!(s.volume_space(), Some(SpaceInfo { total: 1000, free: 50 }));
        s.execute("treemap.free_space", &serde_json::json!({"enabled": false})).unwrap();
        assert_eq!(s.free_space_split(bounds()).band, None);
    }

    #[test]
    fn a_shown_snapshot_has_no_free_space_band() {
        let (_dir, mut s, _fake) = session_with_space(50);
        s.execute("treemap.free_space", &serde_json::json!({"enabled": true})).unwrap();
        assert!(settle(&mut s).band.is_some(), "the live tree has a band");
        let out = tempfile::tempdir().unwrap();
        let file = out.path().join("a.dirsnap").to_string_lossy().into_owned();
        s.execute("snapshot.save", &serde_json::json!({ "path": file, "allow_inside_scan": true })).unwrap();
        s.execute("snapshot.load", &serde_json::json!({ "path": file })).unwrap();
        assert!(s.snapshot_view().is_some());
        let split = s.free_space_split(bounds());
        assert_eq!((split.tree, split.band, split.pending), (bounds(), None, false));
    }

    #[test]
    fn a_new_tree_asks_again_and_a_non_local_root_has_no_band() {
        let (_dir, mut s, fake) = session_with_space(50);
        s.execute("treemap.free_space", &serde_json::json!({"enabled": true})).unwrap();
        assert!(settle(&mut s).band.is_some());
        fake.set_space(SpaceInfo { total: 1000, free: 900 });
        assert_eq!(s.volume_space().map(|v| v.free), Some(50), "answered once per tree");
        let mut tree = Tree::new("demo");
        let root = tree.root();
        tree.add_child(root, "f", NodeKind::File, 10).unwrap();
        tree.aggregate();
        s.replace_tree(tree);
        let split = settle(&mut s);
        assert_eq!(split.band, None, "a synthetic root is not on a local drive");
        assert_eq!(split.tree, bounds());
        assert_eq!(s.volume_space(), None);
    }

    #[test]
    fn a_failing_volume_gives_no_band_and_is_not_retried() {
        let (_dir, mut s, fake) = session_with_space(50);
        fake.set_failure(Some(dirustat_platform::PlatformError::Failed { op: "free_space", message: "x".to_owned() }));
        s.execute("treemap.free_space", &serde_json::json!({"enabled": true})).unwrap();
        assert_eq!(settle(&mut s).band, None);
        assert_eq!(settle(&mut s).band, None, "no retry for this tree");
    }
}
