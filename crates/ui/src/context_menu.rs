//! Context menu of tree rows and treemap tiles. Every entry maps to a command id. The two delete
//! entries only open the confirmation dialog (`delete_dialog`); they never delete from the click.

use crate::delete_dialog::{self, DeleteMode};
use crate::state::UiState;
use crate::toast::{self, ToastKind};
use dirustat_engine::Session;
use dirustat_model::NodeId;
use serde_json::json;

/// An entry of the node context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Reveal,
    Properties,
    CopyPath,
    /// Opens the Recycle Bin confirmation dialog.
    Recycle,
    /// Opens the permanent-delete confirmation dialog.
    DeletePermanent,
}

impl MenuAction {
    pub const ALL: [Self; 5] = [Self::Reveal, Self::Properties, Self::CopyPath, Self::Recycle, Self::DeletePermanent];

    /// The command that implements the entry.
    pub fn command_id(self) -> &'static str {
        match self {
            Self::Reveal => "platform.reveal",
            Self::Properties => "platform.properties",
            Self::CopyPath => "ui.copy_path",
            Self::Recycle => "file.delete",
            Self::DeletePermanent => "file.delete_permanent",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Reveal => "Reveal in file manager",
            Self::Properties => "Properties",
            Self::CopyPath => "Copy path",
            Self::Recycle => "Move to Recycle Bin",
            Self::DeletePermanent => "Delete permanently...",
        }
    }

    fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::Recycle => Some("Delete"),
            Self::DeletePermanent => Some("Shift+Delete"),
            _ => None,
        }
    }

    fn destructive(self) -> bool {
        matches!(self, Self::Recycle | Self::DeletePermanent)
    }
}

/// Draws the entries; returns the clicked one (the menu closes). `delete_block` is the reason the
/// delete entries are off (network scan root, scan root itself, running scan); they stay visible
/// but disabled with that reason as the tooltip.
pub fn menu_ui(ui: &mut egui::Ui, delete_block: Option<&str>) -> Option<MenuAction> {
    let mut chosen = None;
    for action in MenuAction::ALL {
        if action == MenuAction::Recycle {
            ui.separator();
        }
        let mut button = egui::Button::new(action.label());
        if let Some(sc) = action.shortcut() {
            button = button.shortcut_text(sc);
        }
        let blocked = action.destructive().then_some(delete_block).flatten();
        let response = ui.add_enabled(blocked.is_none(), button);
        let response = match blocked {
            Some(reason) => response.on_disabled_hover_text(reason),
            None => response,
        };
        if response.clicked() {
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
        MenuAction::Recycle | MenuAction::DeletePermanent => {
            let mode = if action == MenuAction::Recycle { DeleteMode::Recycle } else { DeleteMode::Permanent };
            // Only the read-only preview runs here; the dialog sends the delete after a click.
            if let Err(message) = delete_dialog::open_for_node(session, state, node, mode) {
                toast::push(state, ToastKind::Error, message);
            }
        }
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
        assert_eq!(
            ids,
            ["platform.reveal", "platform.properties", "ui.copy_path", "file.delete", "file.delete_permanent"]
        );
        // Only the two delete entries are destructive, and they carry the registry shortcuts.
        let destructive: Vec<&str> = MenuAction::ALL.iter().filter(|a| a.destructive()).map(|a| a.label()).collect();
        assert_eq!(destructive, ["Move to Recycle Bin", "Delete permanently..."]);
        assert_eq!(MenuAction::Recycle.shortcut(), Some("Delete"));
        assert_eq!(MenuAction::DeletePermanent.shortcut(), Some("Shift+Delete"));
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
