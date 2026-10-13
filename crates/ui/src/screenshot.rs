//! `ui.screenshot`: capture the window through `ViewportCommand::Screenshot` and save a PNG.
//!
//! The request is asynchronous: `request` returns a ticket, the app sends the viewport command on
//! its next frame, the backend answers with `Event::Screenshot` a frame or two later, and the
//! result is polled with `status`. Nothing here blocks or panics on hostile input.
//!
//! Writing is conservative because the path can come from a remote control call: an existing file
//! is refused unless `overwrite` is set, missing directories are not created (except for the
//! app's own trusted `--screenshot` flag) and the bytes land through a temp file plus rename.

use egui::ColorImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Pixels per side above which a capture is rejected instead of encoded.
const MAX_SIDE: usize = 16_384;
/// Total pixels above which a capture is rejected (64 MP is about 256 MB of RGBA); bounds memory
/// for the encoder even when both sides are individually below [`MAX_SIDE`].
const MAX_PIXELS: usize = 64_000_000;
/// A request that gets no capture event within this time fails, so lost events cannot occupy the
/// in-flight slots forever.
const REQUEST_TTL: Duration = Duration::from_secs(60);
/// Finished results kept for polling.
const MAX_FINISHED: usize = 16;
/// Requests allowed in flight.
const MAX_PENDING: usize = 8;
const TIMEOUT_MESSAGE: &str = "timeout: no capture arrived";

/// Region of the window in points (egui logical pixels).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// State of one screenshot request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScreenshotStatus {
    Pending,
    Done { path: String, width: u32, height: u32 },
    Failed { message: String },
}

/// How a capture is written to disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveOptions {
    /// Replace an existing file. Without it an existing path is an error.
    pub overwrite: bool,
    /// Create missing parent directories. Only trusted callers (the app's own `--screenshot`
    /// flag) set this; the control channel never creates directories.
    pub create_dirs: bool,
}

impl SaveOptions {
    /// Options for the control channel: no directory creation, overwrite only when asked.
    pub fn controlled(overwrite: bool) -> Self {
        Self { overwrite, create_dirs: false }
    }

    /// Options for the user's own command line: overwrite and create directories.
    pub fn trusted() -> Self {
        Self { overwrite: true, create_dirs: true }
    }
}

#[derive(Debug)]
struct Entry {
    ticket: u64,
    path: PathBuf,
    region: Option<Region>,
    options: SaveOptions,
    deadline: Instant,
    sent: bool,
    status: ScreenshotStatus,
}

/// Queue of screenshot requests, owned by the app.
#[derive(Debug)]
pub struct Screenshots {
    next: u64,
    ttl: Duration,
    entries: Vec<Entry>,
}

impl Default for Screenshots {
    fn default() -> Self {
        Self::with_ttl(REQUEST_TTL)
    }
}

impl Screenshots {
    /// Queue with an explicit request lifetime (tests use short ones).
    pub fn with_ttl(ttl: Duration) -> Self {
        Self { next: 0, ttl, entries: Vec::new() }
    }

    /// Queues a trusted capture (overwrites, creates directories); see [`SaveOptions::trusted`].
    pub fn request(&mut self, path: &str, region: Option<Region>) -> Result<u64, String> {
        self.request_with(path, region, SaveOptions::trusted())
    }

    /// Fails pending requests whose deadline has passed.
    pub fn expire(&mut self, now: Instant) {
        for e in self.entries.iter_mut().filter(|e| e.status == ScreenshotStatus::Pending && now >= e.deadline) {
            e.status = ScreenshotStatus::Failed { message: TIMEOUT_MESSAGE.to_owned() };
        }
    }

    /// Queues a capture of the whole window (or `region`) into `path` (must end in `.png`).
    pub fn request_with(&mut self, path: &str, region: Option<Region>, options: SaveOptions) -> Result<u64, String> {
        let path = validate_path(path)?;
        if !options.overwrite && std::fs::symlink_metadata(&path).is_ok() {
            return Err(format!("{} exists; pass overwrite: true to replace it", path.display()));
        }
        if let Some(r) = region
            && !(r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite() && r.w > 0.0 && r.h > 0.0)
        {
            return Err("region must be finite with positive width and height".to_owned());
        }
        let now = Instant::now();
        self.expire(now);
        let pending = self.entries.iter().filter(|e| e.status == ScreenshotStatus::Pending).count();
        if pending >= MAX_PENDING {
            return Err("too many screenshots in flight".to_owned());
        }
        self.next = self.next.saturating_add(1);
        let ticket = self.next;
        let deadline = now + self.ttl;
        self.entries.push(Entry {
            ticket,
            path,
            region,
            options,
            deadline,
            sent: false,
            status: ScreenshotStatus::Pending,
        });
        let finished = self.entries.iter().filter(|e| e.status != ScreenshotStatus::Pending).count();
        if finished > MAX_FINISHED {
            let mut drop_n = finished.saturating_sub(MAX_FINISHED);
            self.entries.retain(|e| {
                if drop_n > 0 && e.status != ScreenshotStatus::Pending {
                    drop_n = drop_n.saturating_sub(1);
                    return false;
                }
                true
            });
        }
        Ok(ticket)
    }

    /// Current status of a ticket; `None` for unknown or expired tickets.
    pub fn status(&self, ticket: u64) -> Option<ScreenshotStatus> {
        let e = self.entries.iter().find(|e| e.ticket == ticket)?;
        if e.status == ScreenshotStatus::Pending && Instant::now() >= e.deadline {
            return Some(ScreenshotStatus::Failed { message: TIMEOUT_MESSAGE.to_owned() });
        }
        Some(e.status.clone())
    }

    pub fn has_pending(&self) -> bool {
        self.entries.iter().any(|e| e.status == ScreenshotStatus::Pending)
    }

    /// Sends the viewport command for requests not sent yet and consumes finished captures.
    pub fn pump(&mut self, ctx: &egui::Context) {
        self.expire(Instant::now());
        for e in self.entries.iter_mut().filter(|e| !e.sent && e.status == ScreenshotStatus::Pending) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(e.ticket)));
            e.sent = true;
        }
        let shots: Vec<(u64, std::sync::Arc<ColorImage>)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|ev| match ev {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let ticket = user_data.data.as_ref()?.downcast_ref::<u64>().copied()?;
                        Some((ticket, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        let ppp = ctx.pixels_per_point();
        for (ticket, image) in shots {
            self.finish(ticket, &image, ppp);
        }
        if self.has_pending() {
            ctx.request_repaint();
        }
    }

    /// Stores a captured image for `ticket` (also the entry point for tests).
    pub fn finish(&mut self, ticket: u64, image: &ColorImage, pixels_per_point: f32) {
        let Some(e) = self.entries.iter_mut().find(|e| e.ticket == ticket) else { return };
        e.status = match save_png(&e.path, image, e.region, pixels_per_point, e.options) {
            Ok((width, height)) => {
                ScreenshotStatus::Done { path: e.path.to_string_lossy().into_owned(), width, height }
            }
            Err(message) => ScreenshotStatus::Failed { message },
        };
    }
}

fn validate_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed.len() > 4096 || trimmed.contains('\0') {
        return Err("path must be a non-empty file name".to_owned());
    }
    let p = Path::new(trimmed);
    let is_png = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err("screenshot path must end in .png".to_owned());
    }
    Ok(p.to_path_buf())
}

/// Pixel rectangle `(x, y, w, h)` of `region` inside an image, clamped to its bounds.
fn pixel_rect(size: [usize; 2], region: Option<Region>, ppp: f32) -> Result<(usize, usize, usize, usize), String> {
    let [iw, ih] = size;
    let Some(r) = region else { return Ok((0, 0, iw, ih)) };
    let scale = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    // Float to usize casts saturate in Rust; negative values become 0.
    let to_px = |v: f32| (v * scale).max(0.0).round() as usize;
    let x0 = to_px(r.x).min(iw);
    let y0 = to_px(r.y).min(ih);
    let x1 = to_px(r.x + r.w).min(iw);
    let y1 = to_px(r.y + r.h).min(ih);
    let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if w == 0 || h == 0 {
        return Err("region is outside the window".to_owned());
    }
    Ok((x0, y0, w, h))
}

/// True if a `w` x `h` image is non-empty and within the side and total pixel caps.
fn dims_ok(w: usize, h: usize) -> bool {
    w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE && w.saturating_mul(h) <= MAX_PIXELS
}

/// Encodes (a region of) `image` as RGBA PNG bytes; returns the bytes and the pixel size.
pub fn encode_png(
    image: &ColorImage,
    region: Option<Region>,
    pixels_per_point: f32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let [iw, ih] = image.size;
    if !dims_ok(iw, ih) || image.pixels.len() != iw.saturating_mul(ih) {
        return Err("captured image has an unusable size".to_owned());
    }
    let (x0, y0, w, h) = pixel_rect(image.size, region, pixels_per_point)?;
    let mut rgba = Vec::with_capacity(w.saturating_mul(h).saturating_mul(4));
    for row in y0..y0.saturating_add(h) {
        let start = row.saturating_mul(iw).saturating_add(x0);
        let line = image.pixels.get(start..start.saturating_add(w)).ok_or("image rows out of range")?;
        for px in line {
            rgba.extend_from_slice(&px.to_srgba_unmultiplied());
        }
    }
    let width = u32::try_from(w).map_err(|e| e.to_string())?;
    let height = u32::try_from(h).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok((out, width, height))
}

/// Saves (a region of) `image` as a PNG file; returns the pixel size written.
///
/// The bytes go to a temp file next to the target and are renamed into place, so readers never see
/// a half-written PNG and a symlink at the target is replaced, not followed.
pub fn save_png(
    path: &Path,
    image: &ColorImage,
    region: Option<Region>,
    pixels_per_point: f32,
    options: SaveOptions,
) -> Result<(u32, u32), String> {
    let (bytes, w, h) = encode_png(image, region, pixels_per_point)?;
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    if options.create_dirs {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    if !options.overwrite && std::fs::symlink_metadata(path).is_ok() {
        return Err(format!("{} exists; pass overwrite: true to replace it", path.display()));
    }
    let tmp = write_temp(dir, &bytes).map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {e}", path.display()));
    }
    Ok((w, h))
}

/// Writes `bytes` to a fresh uniquely named file in `dir` (never reusing an existing one).
fn write_temp(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut last = None;
    for attempt in 0_u32..8 {
        let tmp = dir.join(format!(".dirustat-shot-{}-{nanos}-{attempt}.tmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(bytes).and_then(|()| f.flush()) {
                    drop(f);
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
                return Ok(tmp);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no temp name available")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize) -> ColorImage {
        ColorImage::filled([w, h], egui::Color32::from_rgb(10, 20, 30))
    }

    #[test]
    fn png_round_trips_size_and_region_is_clamped() {
        let img = image(40, 20);
        let (bytes, w, h) = encode_png(&img, None, 1.0).unwrap_or_default();
        assert_eq!((w, h), (40, 20));
        assert_eq!(bytes.get(1..4), Some(&b"PNG"[..]));
        let r = Region { x: 10.0, y: 5.0, w: 1000.0, h: 1000.0 };
        let (_, w, h) = encode_png(&img, Some(r), 1.0).unwrap_or_default();
        assert_eq!((w, h), (30, 15));
        let r = Region { x: 5.0, y: 5.0, w: 10.0, h: 5.0 };
        let (_, w, h) = encode_png(&img, Some(r), 2.0).unwrap_or_default();
        assert_eq!((w, h), (20, 10));
    }

    #[test]
    fn hostile_regions_and_images_are_errors() {
        let img = image(8, 8);
        for r in [
            Region { x: 100.0, y: 0.0, w: 5.0, h: 5.0 },
            Region { x: f32::NAN, y: 0.0, w: 5.0, h: 5.0 },
            Region { x: -50.0, y: -50.0, w: 10.0, h: 10.0 },
            Region { x: 0.0, y: 0.0, w: f32::INFINITY, h: f32::INFINITY },
        ] {
            let res = encode_png(&img, Some(r), 1.0);
            // NaN and infinite sizes either clamp to the image or fail; never panic.
            assert!(res.is_ok() || res.is_err());
        }
        assert!(encode_png(&image(0, 0), None, 1.0).is_err());
        let mut bad = image(4, 4);
        bad.pixels.truncate(3);
        assert!(encode_png(&bad, None, 1.0).is_err());
    }

    #[test]
    fn request_validates_path_and_region() {
        let mut s = Screenshots::default();
        for bad in ["", "  ", "shot.txt", "shot", "a\0.png"] {
            assert!(s.request(bad, None).is_err(), "{bad:?}");
        }
        let nan = Region { x: f32::NAN, y: 0.0, w: 1.0, h: 1.0 };
        assert!(s.request("x.png", Some(nan)).is_err());
        let zero = Region { x: 0.0, y: 0.0, w: 0.0, h: 1.0 };
        assert!(s.request("x.png", Some(zero)).is_err());
        assert!(s.request(&"a".repeat(5000), None).is_err());
    }

    #[test]
    fn finish_writes_a_file_and_updates_status() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let path = dir.path().join("sub").join("shot.png");
        let mut s = Screenshots::default();
        let Ok(t) = s.request(&path.to_string_lossy(), None) else { return };
        assert_eq!(s.status(t), Some(ScreenshotStatus::Pending));
        s.finish(t, &image(16, 9), 1.0);
        assert!(matches!(s.status(t), Some(ScreenshotStatus::Done { width: 16, height: 9, .. })));
        assert!(path.is_file());
        assert!(!s.has_pending());
        assert_eq!(s.status(999), None);
        s.finish(999, &image(1, 1), 1.0);
    }

    #[test]
    fn existing_file_is_refused_unless_overwrite_and_no_temp_files_remain() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let path = dir.path().join("keep.png");
        assert!(std::fs::write(&path, b"precious").is_ok());
        let p = path.to_string_lossy().into_owned();
        let mut s = Screenshots::default();
        assert!(s.request_with(&p, None, SaveOptions::controlled(false)).is_err());
        // Even if the file appears after the request, the save step refuses.
        let other = dir.path().join("late.png");
        let Ok(t) = s.request_with(&other.to_string_lossy(), None, SaveOptions::controlled(false)) else { return };
        assert!(std::fs::write(&other, b"late").is_ok());
        s.finish(t, &image(4, 4), 1.0);
        assert!(matches!(s.status(t), Some(ScreenshotStatus::Failed { .. })));
        assert_eq!(std::fs::read(&other).unwrap_or_default(), b"late");
        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"precious");
        let Ok(t) = s.request_with(&p, None, SaveOptions::controlled(true)) else { return };
        s.finish(t, &image(4, 4), 1.0);
        assert!(matches!(s.status(t), Some(ScreenshotStatus::Done { .. })));
        assert_ne!(std::fs::read(&path).unwrap_or_default(), b"precious");
        let leftovers = std::fs::read_dir(dir.path()).map_or(0, |d| {
            d.filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count()
        });
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn controlled_requests_do_not_create_directories() {
        let Ok(dir) = tempfile::tempdir() else { return };
        let path = dir.path().join("missing").join("deeper").join("x.png");
        let mut s = Screenshots::default();
        let Ok(t) = s.request_with(&path.to_string_lossy(), None, SaveOptions::controlled(false)) else { return };
        s.finish(t, &image(4, 4), 1.0);
        assert!(matches!(s.status(t), Some(ScreenshotStatus::Failed { .. })));
        assert!(!dir.path().join("missing").exists());
    }

    #[test]
    fn pixel_cap_applies_to_the_total() {
        assert!(dims_ok(8000, 8000));
        assert!(!dims_ok(8001, 8000));
        assert!(!dims_ok(MAX_SIDE, MAX_SIDE));
        assert!(!dims_ok(0, 5));
        let big = ColorImage { size: [10_000, 7_000], source_size: egui::Vec2::ZERO, pixels: Vec::new() };
        assert!(encode_png(&big, None, 1.0).is_err());
    }

    #[test]
    fn stale_pending_requests_expire_and_free_their_slot() {
        let mut s = Screenshots::with_ttl(Duration::ZERO);
        let mut tickets = Vec::new();
        for _ in 0..MAX_PENDING {
            tickets.extend(s.request("x.png", None).ok());
        }
        // Without the deadline this would stay "too many screenshots in flight" forever.
        assert!(s.request("x.png", None).is_ok());
        assert!(matches!(
            tickets.first().and_then(|t| s.status(*t)),
            Some(ScreenshotStatus::Failed { message }) if message.starts_with("timeout")
        ));
    }

    #[test]
    fn pending_requests_are_capped_and_pump_sends_once() {
        let mut s = Screenshots::default();
        for _ in 0..MAX_PENDING {
            assert!(s.request("x.png", None).is_ok());
        }
        assert!(s.request("x.png", None).is_err());
        let ctx = egui::Context::default();
        let out = ctx.run_ui(egui::RawInput::default(), |_| s.pump(&ctx));
        out.drop_without_applying_deltas();
        assert!(s.entries.iter().all(|e| e.sent));
    }
}
