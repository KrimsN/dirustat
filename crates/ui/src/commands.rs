//! Shell-only commands (`ui.*`): view state that belongs to the window, not to the session.
//!
//! They mirror the engine registry: an id, label, menu path, shortcut and a pure state change.
//! The menus, shortcuts and the command palette list them next to `Session::commands()`.

use crate::state::{MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT, UiState, ZOOM_STEP};
use crate::theme::ThemeKind;

/// Side effect a UI command asks the window to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEffect {
    None,
    Quit,
}

/// A shell-only command.
#[derive(Clone, Copy, Debug)]
pub struct UiCommand {
    /// Stable id, `ui.action`.
    pub id: &'static str,
    pub label: &'static str,
    /// Menu path (`View/Treemap`); empty for commands without a menu entry.
    pub menu: &'static str,
    pub shortcut: Option<&'static str>,
    pub params_doc: &'static str,
    /// Check mark for toggles.
    pub checked: Option<fn(&UiState) -> bool>,
    pub run: fn(&mut UiState) -> UiEffect,
}

fn toggle_tree(s: &mut UiState) -> UiEffect {
    s.panels.tree = !s.panels.tree;
    UiEffect::None
}
fn toggle_extensions(s: &mut UiState) -> UiEffect {
    s.panels.extensions = !s.panels.extensions;
    UiEffect::None
}
fn toggle_map(s: &mut UiState) -> UiEffect {
    s.panels.map = !s.panels.map;
    UiEffect::None
}
fn toggle_status(s: &mut UiState) -> UiEffect {
    s.panels.status_bar = !s.panels.status_bar;
    UiEffect::None
}
fn theme_dark(s: &mut UiState) -> UiEffect {
    s.theme = ThemeKind::Dark;
    UiEffect::None
}
fn theme_light(s: &mut UiState) -> UiEffect {
    s.theme = ThemeKind::Light;
    UiEffect::None
}
fn zoom_in(s: &mut UiState) -> UiEffect {
    s.zoom_percent = s.zoom_percent.saturating_add(ZOOM_STEP).min(MAX_ZOOM_PERCENT);
    UiEffect::None
}
fn zoom_out(s: &mut UiState) -> UiEffect {
    s.zoom_percent = s.zoom_percent.saturating_sub(ZOOM_STEP).max(MIN_ZOOM_PERCENT);
    UiEffect::None
}
fn zoom_reset(s: &mut UiState) -> UiEffect {
    s.zoom_percent = 100;
    UiEffect::None
}
fn palette(s: &mut UiState) -> UiEffect {
    s.palette_open = !s.palette_open;
    s.palette_query.clear();
    s.palette_index = 0;
    UiEffect::None
}
fn quit(_: &mut UiState) -> UiEffect {
    UiEffect::Quit
}

const NONE: &str = "none";

/// All shell-only commands.
pub const UI_COMMANDS: &[UiCommand] = &[
    UiCommand {
        id: "ui.quit",
        label: "Quit",
        menu: "File/Quit",
        shortcut: Some("Ctrl+Q"),
        params_doc: NONE,
        checked: None,
        run: quit,
    },
    UiCommand {
        id: "ui.toggle_tree",
        label: "Directory tree",
        menu: "View/Directory tree",
        shortcut: None,
        params_doc: NONE,
        checked: Some(|s| s.panels.tree),
        run: toggle_tree,
    },
    UiCommand {
        id: "ui.toggle_extensions",
        label: "File types",
        menu: "View/File types",
        shortcut: None,
        params_doc: NONE,
        checked: Some(|s| s.panels.extensions),
        run: toggle_extensions,
    },
    UiCommand {
        id: "ui.toggle_map",
        label: "Treemap",
        menu: "View/Treemap",
        shortcut: None,
        params_doc: NONE,
        checked: Some(|s| s.panels.map),
        run: toggle_map,
    },
    UiCommand {
        id: "ui.toggle_status_bar",
        label: "Status bar",
        menu: "View/Status bar",
        shortcut: None,
        params_doc: NONE,
        checked: Some(|s| s.panels.status_bar),
        run: toggle_status,
    },
    UiCommand {
        id: "ui.theme_dark",
        label: "Dark theme",
        menu: "View/Dark theme",
        shortcut: None,
        params_doc: NONE,
        checked: Some(|s| s.theme == ThemeKind::Dark),
        run: theme_dark,
    },
    UiCommand {
        id: "ui.theme_light",
        label: "Light theme",
        menu: "View/Light theme",
        shortcut: None,
        params_doc: NONE,
        checked: Some(|s| s.theme == ThemeKind::Light),
        run: theme_light,
    },
    UiCommand {
        id: "ui.zoom_in",
        label: "Zoom in",
        menu: "View/Zoom in",
        shortcut: Some("Ctrl+="),
        params_doc: NONE,
        checked: None,
        run: zoom_in,
    },
    UiCommand {
        id: "ui.zoom_out",
        label: "Zoom out",
        menu: "View/Zoom out",
        shortcut: Some("Ctrl+-"),
        params_doc: NONE,
        checked: None,
        run: zoom_out,
    },
    UiCommand {
        id: "ui.zoom_reset",
        label: "Reset zoom",
        menu: "View/Reset zoom",
        shortcut: Some("Ctrl+0"),
        params_doc: NONE,
        checked: None,
        run: zoom_reset,
    },
    UiCommand {
        id: "ui.command_palette",
        label: "Command palette",
        menu: "View/Command palette",
        shortcut: Some("Ctrl+Shift+P"),
        params_doc: NONE,
        checked: None,
        run: palette,
    },
];

/// Looks a shell command up by id.
pub fn find(id: &str) -> Option<&'static UiCommand> {
    UI_COMMANDS.iter().find(|c| c.id == id)
}

/// Runs the shell command `id`; `None` when no such command exists.
pub fn run(state: &mut UiState, id: &str) -> Option<UiEffect> {
    find(id).map(|c| (c.run)(state))
}

/// Parses a shortcut such as `Ctrl+Shift+P`, `Escape` or `Ctrl+=`. Unknown text gives `None`.
pub fn parse_shortcut(text: &str) -> Option<egui::KeyboardShortcut> {
    let mut mods = egui::Modifiers::NONE;
    let mut key = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" => mods.command = true,
            "shift" => mods.shift = true,
            "alt" => mods.alt = true,
            _ => key = egui::Key::from_name(part).or_else(|| egui::Key::from_name(&part.to_ascii_uppercase())),
        }
    }
    key.map(|k| egui::KeyboardShortcut::new(mods, k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_ui_commands_with_parsable_shortcuts() {
        let mut seen = HashSet::new();
        for c in UI_COMMANDS {
            assert!(seen.insert(c.id));
            assert!(c.id.starts_with("ui."), "{}", c.id);
            assert!(!c.label.is_empty() && !c.params_doc.is_empty());
            if let Some(sc) = c.shortcut {
                assert!(parse_shortcut(sc).is_some(), "{sc}");
            }
        }
    }

    #[test]
    fn shortcuts_parse() {
        let p = parse_shortcut("Ctrl+Shift+P");
        assert_eq!(
            p.map(|s| (s.logical_key, s.modifiers.shift, s.modifiers.command)),
            Some((egui::Key::P, true, true))
        );
        assert_eq!(parse_shortcut("Escape").map(|s| s.logical_key), Some(egui::Key::Escape));
        assert_eq!(parse_shortcut("Ctrl+-").map(|s| s.logical_key), Some(egui::Key::Minus));
        assert_eq!(parse_shortcut("Ctrl+=").map(|s| s.logical_key), Some(egui::Key::Equals));
        assert_eq!(parse_shortcut("Ctrl+0").map(|s| s.logical_key), Some(egui::Key::Num0));
        assert!(parse_shortcut("Ctrl+Bogus").is_none());
        assert!(parse_shortcut("").is_none());
    }

    #[test]
    fn toggles_and_zoom_stay_in_range() {
        let mut s = UiState::default();
        assert_eq!(run(&mut s, "ui.toggle_map"), Some(UiEffect::None));
        assert!(!s.panels.map);
        run(&mut s, "ui.toggle_map");
        assert!(s.panels.map);
        for _ in 0..100 {
            run(&mut s, "ui.zoom_in");
        }
        assert_eq!(s.zoom_percent, MAX_ZOOM_PERCENT);
        for _ in 0..100 {
            run(&mut s, "ui.zoom_out");
        }
        assert_eq!(s.zoom_percent, MIN_ZOOM_PERCENT);
        run(&mut s, "ui.zoom_reset");
        assert_eq!(s.zoom_percent, 100);
        assert_eq!(run(&mut s, "ui.quit"), Some(UiEffect::Quit));
        assert_eq!(run(&mut s, "ui.nope"), None);
    }
}
