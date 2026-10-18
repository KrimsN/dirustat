//! A transient notice in the bottom-right corner: the outcome of a delete, a cleanup run or a
//! refused action. It stays for [`TOAST_SECONDS`] or until dismissed. The text never contains more
//! than the counts, sizes and names the user just confirmed.

use crate::state::UiState;
use crate::theme::Tokens;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// How long a toast stays on screen.
pub const TOAST_SECONDS: u64 = 12;
/// Longest toast text kept (characters); the rest is cut so a hostile message cannot flood the UI.
const MAX_TEXT_CHARS: usize = 600;

/// Severity of a toast; picks the accent colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToastKind {
    Success,
    Info,
    Error,
}

/// The notice itself (serializable so `ui.state` shows it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
}

/// Shows `text` as the current toast (replacing an older one).
pub fn push(state: &mut UiState, kind: ToastKind, text: impl Into<String>) {
    let text: String = text.into().chars().take(MAX_TEXT_CHARS).collect();
    state.toast = Some(Toast { kind, text });
    state.toast_since = Some(Instant::now());
}

/// Draws the toast and expires it. Returns nothing: the only interaction is the close button.
pub fn show(ctx: &egui::Context, state: &mut UiState, tokens: &Tokens) {
    let Some(toast) = state.toast.clone() else { return };
    let since = *state.toast_since.get_or_insert_with(Instant::now);
    let lifetime = Duration::from_secs(TOAST_SECONDS);
    let elapsed = since.elapsed();
    if elapsed >= lifetime {
        state.toast = None;
        state.toast_since = None;
        return;
    }
    ctx.request_repaint_after(lifetime.saturating_sub(elapsed));
    let accent = Tokens::color(match toast.kind {
        ToastKind::Success => tokens.success,
        ToastKind::Info => tokens.accent,
        ToastKind::Error => tokens.error,
    });
    let mut dismissed = false;
    egui::Area::new(egui::Id::new("dirustat_toast"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -40.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(Tokens::color(tokens.surface))
                .stroke(egui::Stroke::new(1.5, accent))
                .corner_radius(egui::CornerRadius::same(tokens.radius_medium))
                .show(ui, |ui| {
                    ui.set_max_width(380.0);
                    ui.horizontal_top(|ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&toast.text).color(Tokens::color(tokens.text))).wrap(),
                        );
                        if ui.small_button("\u{d7}").on_hover_text("Dismiss").clicked() {
                            dismissed = true;
                        }
                    });
                });
        });
    if dismissed {
        state.toast = None;
        state.toast_since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_replaces_and_caps_the_text() {
        let mut s = UiState::default();
        push(&mut s, ToastKind::Info, "one");
        push(&mut s, ToastKind::Error, "x".repeat(MAX_TEXT_CHARS * 2));
        let toast = s.toast.clone().unwrap_or(Toast { kind: ToastKind::Info, text: String::new() });
        assert_eq!(toast.kind, ToastKind::Error);
        assert_eq!(toast.text.chars().count(), MAX_TEXT_CHARS);
        assert!(s.toast_since.is_some());
    }

    #[test]
    fn expired_toast_disappears_and_all_kinds_draw() {
        let ctx = egui::Context::default();
        let mut s = UiState::default();
        for kind in [ToastKind::Success, ToastKind::Info, ToastKind::Error] {
            push(&mut s, kind, "Moved 3 items to the Recycle Bin");
            let out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui.ctx(), &mut s, &Tokens::dark()));
            out.drop_without_applying_deltas();
            assert!(s.toast.is_some());
        }
        s.toast_since = Instant::now().checked_sub(Duration::from_secs(TOAST_SECONDS + 1));
        let out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui.ctx(), &mut s, &Tokens::light()));
        out.drop_without_applying_deltas();
        assert!(s.toast.is_none());
    }
}
