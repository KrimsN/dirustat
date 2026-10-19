//! Preferences window (`ui.preferences`): the scan exclusion list. The list itself lives in the
//! session settings (`scan.exclude`, changed with `settings.set`); this module holds the pure list
//! edits and the window drawing.

use crate::theme::Tokens;

/// Most patterns the list accepts (the engine enforces the same limit).
pub const MAX_PATTERNS: usize = 256;

/// What the user did in the window this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefsAction {
    Add(String),
    Remove(usize),
}

/// The list with `input` appended. Empty input and duplicates are reported, not added.
pub fn with_pattern(list: &[String], input: &str) -> Result<Vec<String>, String> {
    let pattern = input.trim();
    if pattern.is_empty() {
        return Err("Type a pattern first, for example node_modules or *.tmp".to_owned());
    }
    if list.iter().any(|p| p == pattern) {
        return Err("This pattern is already in the list".to_owned());
    }
    if list.len() >= MAX_PATTERNS {
        return Err(format!("At most {MAX_PATTERNS} patterns are allowed"));
    }
    let mut next = list.to_vec();
    next.push(pattern.to_owned());
    Ok(next)
}

/// The list without the entry at `index` (an index out of range changes nothing).
pub fn without_pattern(list: &[String], index: usize) -> Vec<String> {
    list.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, p)| p.clone()).collect()
}

/// Draws the window. `open` is cleared by the close button.
pub fn show(
    ctx: &egui::Context,
    open: &mut bool,
    input: &mut String,
    error: Option<&str>,
    exclude: &[String],
    tokens: &Tokens,
) -> Option<PrefsAction> {
    let mut action = None;
    egui::Window::new("Preferences").open(open).collapsible(false).default_width(420.0).show(ctx, |ui| {
        ui.heading("Scan exclusions");
        ui.colored_label(
            Tokens::color(tokens.text_muted),
            "Files and folders matching a pattern are skipped by the next scan. Use * and ?. A pattern \
             without a slash is matched against names (node_modules, *.tmp), one with a slash against \
             the whole path.",
        );
        ui.add_space(tokens.spacing);
        if exclude.is_empty() {
            ui.colored_label(Tokens::color(tokens.text_muted), "No exclusions");
        }
        for (i, pattern) in exclude.iter().enumerate() {
            // Remove stays pinned at the right edge; a long pattern is cut at the end and
            // shown in full by its tooltip (QA-133).
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        action = Some(PrefsAction::Remove(i));
                    }
                    ui.add(egui::Label::new(egui::RichText::new(pattern).monospace()).truncate())
                        .on_hover_text(pattern);
                });
            });
        }
        ui.add_space(tokens.spacing);
        ui.horizontal(|ui| {
            let edit = ui.add(egui::TextEdit::singleline(input).desired_width(220.0).hint_text("pattern"));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Add").clicked() || enter {
                action = Some(PrefsAction::Add(input.clone()));
            }
        });
        if let Some(message) = error {
            ui.colored_label(Tokens::color(tokens.error), message);
        }
        if !exclude.is_empty() {
            ui.colored_label(
                Tokens::color(tokens.warning),
                "A non-empty list disables the MFT fast path: scans use the slower portable walk.",
            );
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_very_long_pattern_keeps_the_window_and_remove_in_view() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let long = format!("C:/very/long/{}", "p".repeat(1024));
        let exclude = vec!["*.tmp".to_owned(), long];
        for tokens in [Tokens::dark(), Tokens::light()] {
            let mut open = true;
            let mut input = String::new();
            // The first frame sizes the window, the second shows its final layout.
            for _ in 0..2 {
                let raw = egui::RawInput { screen_rect: Some(screen), ..egui::RawInput::default() };
                let out = ctx.run_ui(raw, |ui| {
                    let _ = show(ui.ctx(), &mut open, &mut input, None, &exclude, &tokens);
                });
                out.drop_without_applying_deltas();
            }
            let rect = ctx.memory(|m| m.area_rect(egui::Id::new("Preferences")));
            let rect = rect.unwrap_or(egui::Rect::NOTHING);
            assert!(rect.width() < 700.0, "window width {} stays bounded", rect.width());
            assert!(rect.right() <= screen.right(), "window {rect:?} stays on screen");
        }
    }

    #[test]
    fn patterns_are_added_trimmed_and_deduplicated() {
        let list = with_pattern(&[], "  node_modules ").unwrap_or_default();
        assert_eq!(list, ["node_modules"]);
        assert!(with_pattern(&list, "node_modules").is_err());
        assert!(with_pattern(&list, "   ").is_err());
        let full: Vec<String> = (0..MAX_PATTERNS).map(|i| format!("p{i}")).collect();
        assert!(with_pattern(&full, "x").is_err());
    }

    #[test]
    fn removal_is_bounds_safe() {
        let list: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(without_pattern(&list, 1), ["a", "c"]);
        assert_eq!(without_pattern(&list, 99), list);
        assert!(without_pattern(&[], 0).is_empty());
    }
}
