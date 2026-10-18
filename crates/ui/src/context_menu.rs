//! Context menu of tree rows and treemap tiles. Every entry maps to a command id; there is no
//! delete entry yet (destructive commands follow `docs/safety.md`).

use crate::state::UiState;
use dirustat_engine::Session;
use dirustat_model::NodeId;
use serde_json::json;

/// An entry of the node context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Reveal,
    Properties,
    CopyPath,
}

impl MenuAction {
    pub const ALL: [Self; 3] = [Self::Reveal, Self::Properties, Self::CopyPath];

    /// The command that implements the entry.
    pub fn command_id(self) -> &'static str {
        match self {
            Self::Reveal => "platform.reveal",
            Self::Properties => "platform.properties",
            Self::CopyPath => "ui.copy_path",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Reveal => "Reveal in file manager",
            Self::Properties => "Properties",
            Self::CopyPath => "Copy path",
        }
    }
}

/// Draws the entries; returns the clicked one (the menu closes).
pub fn menu_ui(ui: &mut egui::Ui) -> Option<MenuAction> {
    let mut chosen = None;
    for action in MenuAction::ALL {
        if ui.button(action.label()).clicked() {
            chosen = Some(action);
            ui.close();
        }
    }
    chosen
}

/// Runs `action` for `node`. Failures become the status-bar notice, never a panic.
pub fn perform(ctx: &egui::Context, session: &mut Session, state: &mut UiState, node: NodeId, action: MenuAction) {
    match action {
        MenuAction::CopyPath => match session.node_path(node) {
            Some(path) => {
                ctx.copy_text(path);
                state.notice = None;
            }
            None => state.notice = Some("This item has no path".to_owned()),
        },
        MenuAction::Reveal | MenuAction::Properties => {
            match session.execute(action.command_id(), &json!({ "id": node.index() })) {
                Ok(_) => state.notice = None,
                Err(e) => {
                    let text = e.to_string();
                    state.notice = Some(text.strip_prefix("invalid params: ").unwrap_or(&text).to_owned());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_platform::{FakePlatform, PlatformCall};
    use std::sync::Arc;

    #[test]
    fn entries_map_to_commands() {
        let ids: Vec<&str> = MenuAction::ALL.iter().map(|a| a.command_id()).collect();
        assert_eq!(ids, ["platform.reveal", "platform.properties", "ui.copy_path"]);
        assert!(MenuAction::ALL.iter().all(|a| !a.label().to_lowercase().contains("delete")));
        assert!(crate::commands::find("ui.copy_path").is_some());
    }

    #[test]
    fn reveal_and_properties_go_through_the_platform_commands() {
        let Ok(dir) = tempfile::tempdir() else { return };
        assert!(std::fs::write(dir.path().join("a.txt"), b"x").is_ok());
        let mut session = Session::new();
        let fake = Arc::new(FakePlatform::new());
        session.set_platform(fake.clone());
        let Ok(handle) = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()) else { return };
        assert_eq!(session.finish_scan(handle), Ok(true));
        let node = session.tree().root();
        let path = session.node_path(node).unwrap_or_default();
        let mut state = UiState::default();
        let ctx = egui::Context::default();
        perform(&ctx, &mut session, &mut state, node, MenuAction::Reveal);
        perform(&ctx, &mut session, &mut state, node, MenuAction::Properties);
        assert_eq!(
            fake.calls(),
            vec![
                PlatformCall::Reveal(std::path::PathBuf::from(&path)),
                PlatformCall::Properties(std::path::PathBuf::from(&path))
            ]
        );
        assert_eq!(state.notice, None);
        // A platform failure is a notice, not a panic.
        fake.set_failure(Some(dirustat_platform::PlatformError::Unsupported("reveal")));
        perform(&ctx, &mut session, &mut state, node, MenuAction::Reveal);
        assert!(state.notice.is_some());
        // Copy path works without touching the platform.
        let before = fake.calls().len();
        perform(&ctx, &mut session, &mut state, node, MenuAction::CopyPath);
        assert_eq!(fake.calls().len(), before);
        assert_eq!(state.notice, None);
    }
}
