//! Runtime fallback font for CJK (and other scripts the bundled egui fonts lack), QA-126.
//!
//! No font file is committed (`AGENTS.md`). At startup the shell looks for a system font in the
//! OS font directory and registers it as the last fallback of the proportional and monospace
//! families. Everything is best effort: a missing directory, an unreadable or oversized file or
//! data that does not look like a font leaves the built-in fonts untouched and never panics.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Name of the fallback face in `FontDefinitions`.
pub const FONT_NAME: &str = "system-cjk-fallback";

/// Largest font file read (system CJK collections are 10-30 MiB).
pub const MAX_FONT_BYTES: u64 = 96 * 1024 * 1024;

/// System fonts that cover Chinese, Japanese and Korean, in order of preference.
pub fn candidate_paths() -> Vec<PathBuf> {
    candidates_in(&system_font_dirs())
}

fn system_font_dirs() -> Vec<PathBuf> {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")
            .or_else(|| std::env::var_os("windir"))
            .map_or_else(|| PathBuf::from("C:\\Windows"), PathBuf::from);
        vec![root.join("Fonts")]
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from("/System/Library/Fonts"), PathBuf::from("/Library/Fonts")]
    } else {
        vec![
            PathBuf::from("/usr/share/fonts/opentype/noto"),
            PathBuf::from("/usr/share/fonts/noto-cjk"),
            PathBuf::from("/usr/share/fonts/truetype/noto"),
            PathBuf::from("/usr/share/fonts/truetype/droid"),
            PathBuf::from("/usr/share/fonts/truetype/wqy"),
        ]
    }
}

const FILE_NAMES: &[&str] = &[
    // Windows: Simplified Chinese, Japanese, Korean, Traditional Chinese.
    "msyh.ttc",
    "YuGothR.ttc",
    "malgun.ttf",
    "msjh.ttc",
    "meiryo.ttc",
    "simsun.ttc",
    // macOS.
    "PingFang.ttc",
    "Hiragino Sans GB.ttc",
    "AppleSDGothicNeo.ttc",
    // Linux.
    "NotoSansCJK-Regular.ttc",
    "DroidSansFallbackFull.ttf",
    "wqy-microhei.ttc",
];

/// Every `dir/name` combination, preferred files first.
pub fn candidates_in(dirs: &[PathBuf]) -> Vec<PathBuf> {
    FILE_NAMES.iter().flat_map(|name| dirs.iter().map(move |dir| dir.join(name))).collect()
}

/// True when `bytes` start with a TrueType/OpenType/collection signature.
pub fn looks_like_font(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0, 1, 0, 0] | b"OTTO" | b"ttcf" | b"true"))
}

/// Reads one font file; `None` for a missing, oversized, unreadable or non-font file.
pub fn read_font(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FONT_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    looks_like_font(&bytes).then_some(bytes)
}

/// The first readable font of `paths` and its path.
pub fn first_available(paths: &[PathBuf]) -> Option<(PathBuf, Vec<u8>)> {
    paths.iter().find_map(|p| read_font(p).map(|bytes| (p.clone(), bytes)))
}

/// `base` with `bytes` registered as the last fallback of the proportional and monospace families.
pub fn definitions_with_fallback(mut base: egui::FontDefinitions, bytes: Vec<u8>) -> egui::FontDefinitions {
    base.font_data.insert(FONT_NAME.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let names = base.families.entry(family).or_default();
        if !names.iter().any(|n| n == FONT_NAME) {
            names.push(FONT_NAME.to_owned());
        }
    }
    base
}

/// Installs the first available system CJK font as a fallback. Returns the file used, or `None`
/// when the system has none (the built-in fonts then show boxes for those characters).
pub fn install_cjk_fallback(ctx: &egui::Context) -> Option<PathBuf> {
    install_from(ctx, &candidate_paths())
}

/// Like [`install_cjk_fallback`] with explicit candidates (tests, custom setups).
pub fn install_from(ctx: &egui::Context, paths: &[PathBuf]) -> Option<PathBuf> {
    let (path, bytes) = first_available(paths)?;
    ctx.set_fonts(definitions_with_fallback(egui::FontDefinitions::default(), bytes));
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_bogus_files_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.ttc");
        assert!(read_font(&missing).is_none());
        assert!(read_font(dir.path()).is_none(), "a directory is not a font");
        let empty = dir.path().join("empty.ttf");
        std::fs::write(&empty, b"").unwrap();
        assert!(read_font(&empty).is_none());
        let text = dir.path().join("text.ttf");
        std::fs::write(&text, b"this is not a font").unwrap();
        assert!(read_font(&text).is_none());
        assert!(first_available(&[missing, empty, text]).is_none());
        assert!(first_available(&[]).is_none());
    }

    #[test]
    fn install_with_nothing_available_changes_nothing() {
        let ctx = egui::Context::default();
        let missing = PathBuf::from("Z:\\definitely\\not\\here\\msyh.ttc");
        assert!(install_from(&ctx, &[missing]).is_none());
        assert!(install_from(&ctx, &[]).is_none());
    }

    #[test]
    fn first_readable_candidate_wins_and_registers_as_last_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("b.ttc");
        let mut data = b"ttcf".to_vec();
        data.extend_from_slice(&[0; 32]);
        std::fs::write(&good, &data).unwrap();
        let paths = vec![dir.path().join("a.ttc"), good.clone(), dir.path().join("c.ttc")];
        let (found, bytes) = first_available(&paths).unwrap();
        assert_eq!(found, good);
        let defs = definitions_with_fallback(egui::FontDefinitions::default(), bytes);
        assert!(defs.font_data.contains_key(FONT_NAME));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            assert_eq!(defs.families.get(&family).and_then(|v| v.last()).map(String::as_str), Some(FONT_NAME));
        }
        // Registering twice does not duplicate the entry.
        let again = definitions_with_fallback(defs, vec![0, 1, 0, 0]);
        let count = again
            .families
            .get(&egui::FontFamily::Proportional)
            .map_or(0, |v| v.iter().filter(|n| *n == FONT_NAME).count());
        assert_eq!(count, 1);
    }

    #[test]
    fn candidates_cover_every_directory_and_signatures_are_checked() {
        let dirs = vec![PathBuf::from("a"), PathBuf::from("b")];
        let all = candidates_in(&dirs);
        assert_eq!(all.len(), FILE_NAMES.len() * 2);
        assert!(all.first().is_some_and(|p| p.ends_with("msyh.ttc")));
        assert!(!candidate_paths().is_empty());
        assert!(looks_like_font(b"OTTO\0\0"));
        assert!(looks_like_font(&[0, 1, 0, 0, 9]));
        assert!(!looks_like_font(b"ab"));
        assert!(!looks_like_font(b"<html>"));
    }
}
