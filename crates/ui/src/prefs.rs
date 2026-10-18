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
            ui.horizontal(|ui| {
                ui.monospace(pattern);
                if ui.small_button("Remove").clicked() {
                    action = Some(PrefsAction::Remove(i));
                }
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
