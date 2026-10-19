//! Colour of a file extension in the treemap and the file-type list (QA-137).
//!
//! The engine's `ext_color` hashes into 16 colours, so common extensions collide. This mapping
//! gives every extension of a curated list of common types its own slot (12 hues x 5 lightness and
//! saturation variants, ordered so neighbours in the list get far-apart hues) and hashes any other
//! extension into the whole hue circle. Colours are mid-tone, so tiles stand out against both
//! themes. Stable across runs and platforms; case-insensitive; no extension is grey.

use dirustat_engine::NO_EXT_COLOR;

const HUES: usize = 12;
/// (saturation, lightness) of the five variants.
const VARIANTS: [(f32, f32); 5] = [(0.75, 0.50), (0.75, 0.66), (0.75, 0.36), (0.40, 0.55), (0.95, 0.78)];

/// Common types, in groups (video, audio, images, documents, code, data, archives, binaries,
/// temporary). Position decides the colour, so only append at the end.
const CURATED: [&str; 60] = [
    "mp4", "mkv", "avi", "mov", "wmv", "webm", "mp3", "flac", "wav", "ogg", "m4a", "aac", "jpg", "jpeg", "png", "gif",
    "bmp", "webp", "svg", "psd", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv", "json", "xml",
    "html", "css", "js", "ts", "rs", "py", "java", "c", "cpp", "h", "cs", "go", "zip", "7z", "rar", "gz", "tar", "iso",
    "exe", "dll", "sys", "msi", "bin", "dat", "tmp", "log", "pdb", "bak",
];

const FALLBACK_VARIANT: (f32, f32) = (0.75, 0.50);

/// RGB colour of `ext` (without the dot).
pub fn ext_rgb(ext: &str) -> [u8; 3] {
    if ext.is_empty() {
        return NO_EXT_COLOR;
    }
    let lower = ext.to_lowercase();
    if let Some(slot) = CURATED.iter().position(|e| *e == lower) {
        let hue_index = slot.saturating_mul(5) % HUES;
        let variant = slot / HUES;
        // Hues are 30 degrees apart; the hue index is below 12 and the variant below 5.
        let hue = f32::from(u16::try_from(hue_index).unwrap_or(0)) * 30.0;
        let (s, l) = VARIANTS.get(variant).copied().unwrap_or(FALLBACK_VARIANT);
        return hsl_to_rgb(hue, s, l);
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in lower.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let hue = f32::from(u16::try_from(hash % 360).unwrap_or(0));
    let variant = usize::try_from((hash >> 24) % 5).unwrap_or(0);
    let (s, l) = VARIANTS.get(variant).copied().unwrap_or(FALLBACK_VARIANT);
    hsl_to_rgb(hue, s, l)
}

/// HSL (hue in degrees, saturation and lightness in `0..=1`) to RGB bytes.
pub fn hsl_to_rgb(hue: f32, s: f32, l: f32) -> [u8; 3] {
    let h = if hue.is_finite() { hue.rem_euclid(360.0) } else { 0.0 };
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| {
        // The value is clamped to 0..=255 before the conversion.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        b
    };
    [byte(r), byte(g), byte(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: [u8; 3], b: [u8; 3]) -> f32 {
        let d = |x: u8, y: u8| f32::from(x) - f32::from(y);
        (d(a[0], b[0]).powi(2) + d(a[1], b[1]).powi(2) + d(a[2], b[2]).powi(2)).sqrt()
    }

    #[test]
    fn curated_extensions_never_share_a_colour_and_stay_apart() {
        let colours: Vec<[u8; 3]> = CURATED.iter().map(|e| ext_rgb(e)).collect();
        for (i, a) in colours.iter().enumerate() {
            for (j, b) in colours.iter().enumerate().skip(i + 1) {
                assert_ne!(a, b, "{} and {} share a colour", CURATED[i], CURATED[j]);
                assert!(distance(*a, *b) >= 20.0, "{} / {} are too close", CURATED[i], CURATED[j]);
            }
        }
        assert!(!colours.contains(&NO_EXT_COLOR));
    }

    #[test]
    fn the_reported_collisions_are_gone() {
        for group in [["mp4", "zip", "7z", "md"], ["bin", "tmp", "log", "mp3"]] {
            let colours: Vec<[u8; 3]> = group.iter().map(|e| ext_rgb(e)).collect();
            for (i, a) in colours.iter().enumerate() {
                for b in colours.iter().skip(i + 1) {
                    assert!(distance(*a, *b) >= 40.0, "{group:?}");
                }
            }
        }
    }

    #[test]
    fn mapping_is_stable_case_insensitive_and_total() {
        assert_eq!(ext_rgb("MP4"), ext_rgb("mp4"));
        assert_eq!(ext_rgb("zzfoo"), ext_rgb("ZZFOO"));
        assert_eq!(ext_rgb(""), NO_EXT_COLOR);
        assert_ne!(ext_rgb("zzfoo"), ext_rgb("zzbar"));
        // Unknown and hostile extensions still map to a colour.
        let _ = ext_rgb(&"\u{1f600}".repeat(1000));
    }

    #[test]
    fn hsl_conversion_handles_edges() {
        assert_eq!(hsl_to_rgb(0.0, 1.0, 0.5), [255, 0, 0]);
        assert_eq!(hsl_to_rgb(120.0, 1.0, 0.5), [0, 255, 0]);
        assert_eq!(hsl_to_rgb(240.0, 1.0, 0.5), [0, 0, 255]);
        assert_eq!(hsl_to_rgb(f32::NAN, 5.0, -3.0), [0, 0, 0]);
        assert_eq!(hsl_to_rgb(-60.0, 1.0, 0.5), hsl_to_rgb(300.0, 1.0, 0.5));
    }
}
