//! Fitting long paths into a column without hiding the file name (QA-125).
//!
//! egui can only cut text at the end, which in a list of paths with a common prefix leaves rows
//! that look identical. [`fit_middle`] keeps the start of the path and the whole last component
//! and drops the middle; [`path_label`] draws such a label and shows the full path in a tooltip.

/// Ellipsis inserted for the dropped middle (ASCII: every UI font has it).
pub const ELLIPSIS: &str = "...";
/// Longest text measured directly; longer paths are pre-shortened to this many characters.
const MAX_MEASURED_CHARS: usize = 512;
/// Narrowest label that still shows something useful.
const MIN_LABEL_WIDTH: f32 = 90.0;

/// `head + "..." + tail` of `chars` keeping `keep` characters, with at least the last component
/// (and its separator) in the tail.
fn build(chars: &[char], keep: usize, name_len: usize) -> String {
    let n = chars.len();
    let keep = keep.min(n);
    let tail_min = name_len.saturating_add(1).min(keep);
    let head = keep.saturating_sub(tail_min).min(keep / 3);
    let tail = keep.saturating_sub(head);
    let head_part: String = chars.get(..head).unwrap_or_default().iter().collect();
    let tail_part: String = chars.get(n.saturating_sub(tail)..).unwrap_or_default().iter().collect();
    format!("{head_part}{ELLIPSIS}{tail_part}")
}

/// Shortens `text` in the middle until `measure(result) <= max_width`. The file name (the text
/// after the last `\` or `/`) stays visible whenever the width allows. A text that already fits is
/// returned unchanged; `measure` gets the candidate text and returns its width in points.
pub fn fit_middle(text: &str, max_width: f32, measure: &dyn Fn(&str) -> f32) -> String {
    if !max_width.is_finite() {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    if chars.len() > MAX_MEASURED_CHARS {
        // Keep the beginning and the end; the middle of such a path never fits anyway.
        let half = MAX_MEASURED_CHARS / 2;
        let tail: Vec<char> = chars.get(chars.len().saturating_sub(half)..).unwrap_or_default().to_vec();
        chars.truncate(half);
        chars.extend(ELLIPSIS.chars());
        chars.extend(tail);
    } else if measure(text) <= max_width {
        return text.to_owned();
    }
    let n = chars.len();
    let name_len = chars.iter().rev().position(|c| matches!(c, '\\' | '/')).unwrap_or(n);
    // Largest `keep` whose candidate fits; the candidates grow with `keep`.
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = lo.saturating_add(hi.saturating_sub(lo).saturating_add(1) / 2);
        if measure(&build(&chars, mid, name_len)) <= max_width {
            lo = mid;
        } else {
            hi = mid.saturating_sub(1);
        }
    }
    build(&chars, lo.max(1), name_len)
}

/// Draws `path` as a monospace label that fills the available width. A path that does not fit is
/// shortened in the middle (the file name stays) and the full path is the hover text.
pub fn path_label(ui: &mut egui::Ui, path: &str, size: f32, color: Option<egui::Color32>) -> egui::Response {
    let font = egui::FontId::monospace(size);
    let width = ui.available_width().max(MIN_LABEL_WIDTH);
    let painter = ui.painter().clone();
    let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::PLACEHOLDER).size().x;
    let shown = fit_middle(path, width, &measure);
    let mut text = egui::RichText::new(&shown).font(font.clone());
    if let Some(color) = color {
        text = text.color(color);
    }
    let response = ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
    if shown == path { response } else { response.on_hover_text(path) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit per character.
    fn width(s: &str) -> f32 {
        f32::from(u16::try_from(s.chars().count()).unwrap_or(u16::MAX))
    }

    const PATH: &str = "C:\\Users\\someone\\AppData\\Local\\Temp\\fixture\\many\\file_017_with_a_long_name.txt";

    #[test]
    fn a_fitting_path_is_unchanged() {
        assert_eq!(fit_middle(PATH, 200.0, &width), PATH);
        assert_eq!(fit_middle("", 5.0, &width), "");
        assert_eq!(fit_middle(PATH, f32::NAN, &width), PATH);
    }

    #[test]
    fn the_file_name_survives_and_the_result_fits() {
        for max in [60.0, 45.0, 38.0] {
            let out = fit_middle(PATH, max, &width);
            assert!(width(&out) <= max, "{out} ({max})");
            assert!(out.ends_with("file_017_with_a_long_name.txt"), "{out}");
            assert!(out.contains(ELLIPSIS));
            assert!(out.starts_with("C:\\"), "the drive stays: {out}");
        }
        // The common prefix is gone, the distinguishing end is visible.
        let a = fit_middle(&PATH.replace("017", "001"), 45.0, &width);
        let b = fit_middle(&PATH.replace("017", "002"), 45.0, &width);
        assert_ne!(a, b);
    }

    #[test]
    fn a_name_wider_than_the_label_is_cut_at_its_start_not_dropped() {
        let out = fit_middle("C:\\dir\\a_really_long_file_name_here.txt", 14.0, &width);
        assert!(width(&out) <= 14.0, "{out}");
        assert!(out.ends_with(".txt"), "{out}");
        // Even a hopeless width yields a non-empty string and never panics.
        assert!(!fit_middle(PATH, 0.0, &width).is_empty());
        assert!(!fit_middle(PATH, -3.0, &width).is_empty());
    }

    #[test]
    fn huge_and_unicode_paths_are_bounded() {
        let huge = format!("C:\\{}\\last.txt", "d\\".repeat(20_000));
        let out = fit_middle(&huge, 40.0, &width);
        assert!(width(&out) <= 40.0 && out.ends_with("last.txt"), "{out}");
        let cjk = "C:\\\u{65e5}\u{672c}\u{8a9e}\\\u{30d5}\u{30a1}\u{30a4}\u{30eb}\u{540d}.txt";
        let out = fit_middle(cjk, 12.0, &width);
        assert!(width(&out) <= 12.0, "{out}");
        assert!(out.ends_with(".txt"));
    }

    #[test]
    fn the_label_draws_headless_and_tooltips_only_when_cut() {
        let ctx = egui::Context::default();
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_width(150.0);
            let _ = path_label(ui, PATH, 12.0, None);
            let _ = path_label(ui, "short.txt", 12.0, Some(egui::Color32::RED));
        });
        out.drop_without_applying_deltas();
    }
}
