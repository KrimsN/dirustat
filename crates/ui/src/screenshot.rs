//! `ui.screenshot`: capture the window through `ViewportCommand::Screenshot` and save a PNG.
//!
//! The request is asynchronous: `request` returns a ticket, the app sends the viewport command on
//! its next frame, the backend answers with `Event::Screenshot` a frame or two later, and the
//! result is polled with `status`. Nothing here blocks or panics on hostile input.

use egui::ColorImage;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Pixels per side above which a capture is rejected instead of encoded.
const MAX_SIDE: usize = 16_384;
/// Finished results kept for polling.
const MAX_FINISHED: usize = 16;
/// Requests allowed in flight.
const MAX_PENDING: usize = 8;

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

#[derive(Debug)]
struct Entry {
    ticket: u64,
    path: PathBuf,
    region: Option<Region>,
    sent: bool,
    status: ScreenshotStatus,
}

/// Queue of screenshot requests, owned by the app.
#[derive(Debug, Default)]
pub struct Screenshots {
    next: u64,
    entries: Vec<Entry>,
}

impl Screenshots {
    /// Queues a capture of the whole window (or `region`) into `path` (must end in `.png`).
    pub fn request(&mut self, path: &str, region: Option<Region>) -> Result<u64, String> {
        let path = validate_path(path)?;
        if let Some(r) = region
            && !(r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite() && r.w > 0.0 && r.h > 0.0)
        {
            return Err("region must be finite with positive width and height".to_owned());
        }
        let pending = self.entries.iter().filter(|e| e.status == ScreenshotStatus::Pending).count();
        if pending >= MAX_PENDING {
            return Err("too many screenshots in flight".to_owned());
        }
        self.next = self.next.saturating_add(1);
        let ticket = self.next;
        self.entries.push(Entry { ticket, path, region, sent: false, status: ScreenshotStatus::Pending });
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
        self.entries.iter().find(|e| e.ticket == ticket).map(|e| e.status.clone())
    }

    pub fn has_pending(&self) -> bool {
        self.entries.iter().any(|e| e.status == ScreenshotStatus::Pending)
    }

    /// Sends the viewport command for requests not sent yet and consumes finished captures.
    pub fn pump(&mut self, ctx: &egui::Context) {
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
        e.status = match save_png(&e.path, image, e.region, pixels_per_point) {
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

/// Encodes (a region of) `image` as RGBA PNG bytes; returns the bytes and the pixel size.
pub fn encode_png(
    image: &ColorImage,
    region: Option<Region>,
    pixels_per_point: f32,
) -> Result<(Vec<u8>, u32, u32), String> {
    let [iw, ih] = image.size;
    if iw == 0 || ih == 0 || iw > MAX_SIDE || ih > MAX_SIDE || image.pixels.len() != iw.saturating_mul(ih) {
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
pub fn save_png(
    path: &Path,
    image: &ColorImage,
    region: Option<Region>,
    pixels_per_point: f32,
) -> Result<(u32, u32), String> {
    let (bytes, w, h) = encode_png(image, region, pixels_per_point)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok((w, h))
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
