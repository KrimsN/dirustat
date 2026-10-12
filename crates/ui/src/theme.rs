//! Design tokens: every colour, radius, spacing and font size used by the shell comes from here.
//! Widgets never hard-code colours (`docs/ui-design.md`).

use serde::{Deserialize, Serialize};

/// Which token set is active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeKind {
    #[default]
    Dark,
    Light,
}

/// RGBA colour as plain bytes, so tokens stay serializable without egui types.
pub type Rgba = [u8; 4];

/// Colour, radius, spacing and font-size tokens for one theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub kind: ThemeKind,
    pub background: Rgba,
    pub panel: Rgba,
    pub surface: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub success: Rgba,
    pub border: Rgba,
    pub radius_small: u8,
    pub radius_medium: u8,
    pub spacing: f32,
    pub font_body: f32,
    pub font_small: f32,
}

impl Tokens {
    pub fn dark() -> Self {
        Self {
            kind: ThemeKind::Dark,
            background: [24, 26, 31, 255],
            panel: [32, 35, 42, 255],
            surface: [43, 47, 56, 255],
            text: [230, 233, 240, 255],
            text_muted: [150, 156, 170, 255],
            accent: [86, 156, 245, 255],
            error: [240, 100, 100, 255],
            warning: [235, 180, 70, 255],
            success: [110, 200, 130, 255],
            border: [62, 67, 80, 255],
            radius_small: 3,
            radius_medium: 6,
            spacing: 6.0,
            font_body: 14.0,
            font_small: 12.0,
        }
    }

    pub fn light() -> Self {
        Self {
            kind: ThemeKind::Light,
            background: [246, 247, 249, 255],
            panel: [236, 238, 242, 255],
            surface: [255, 255, 255, 255],
            text: [28, 31, 38, 255],
            text_muted: [96, 102, 116, 255],
            accent: [24, 100, 210, 255],
            error: [190, 40, 40, 255],
            warning: [160, 100, 0, 255],
            success: [30, 130, 60, 255],
            border: [200, 204, 214, 255],
            radius_small: 3,
            radius_medium: 6,
            spacing: 6.0,
            font_body: 14.0,
            font_small: 12.0,
        }
    }

    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Dark => Self::dark(),
            ThemeKind::Light => Self::light(),
        }
    }

    /// Converts a token colour to an egui colour.
    pub fn color(c: Rgba) -> egui::Color32 {
        let [r, g, b, a] = c;
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    }

    /// Applies the tokens to an egui context (visuals and spacing).
    pub fn apply(&self, ctx: &egui::Context) {
        let (theme, preference, mut visuals) = match self.kind {
            ThemeKind::Dark => (egui::Theme::Dark, egui::ThemePreference::Dark, egui::Visuals::dark()),
            ThemeKind::Light => (egui::Theme::Light, egui::ThemePreference::Light, egui::Visuals::light()),
        };
        visuals.panel_fill = Self::color(self.panel);
        visuals.window_fill = Self::color(self.surface);
        visuals.extreme_bg_color = Self::color(self.background);
        visuals.override_text_color = Some(Self::color(self.text));
        visuals.error_fg_color = Self::color(self.error);
        visuals.warn_fg_color = Self::color(self.warning);
        visuals.selection.bg_fill = Self::color(self.accent);
        visuals.hyperlink_color = Self::color(self.accent);
        let radius = egui::CornerRadius::same(self.radius_small);
        visuals.widgets.noninteractive.corner_radius = radius;
        visuals.widgets.inactive.corner_radius = radius;
        visuals.widgets.hovered.corner_radius = radius;
        visuals.widgets.active.corner_radius = radius;
        visuals.window_corner_radius = egui::CornerRadius::same(self.radius_medium);
        ctx.set_visuals_of(theme, visuals);
        ctx.set_theme(preference);
        let spacing = self.spacing;
        ctx.global_style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(spacing, spacing);
        });
    }
}

impl Default for Tokens {
    fn default() -> Self {
        Self::dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_through_json() {
        for tokens in [Tokens::dark(), Tokens::light()] {
            let json = serde_json::to_string(&tokens).unwrap_or_default();
            let back: Result<Tokens, _> = serde_json::from_str(&json);
            assert_eq!(back.ok(), Some(tokens));
        }
    }

    #[test]
    fn themes_differ_and_text_contrasts_with_background() {
        let (d, l) = (Tokens::dark(), Tokens::light());
        assert_ne!(d, l);
        assert!(d.text > d.background);
        assert!(l.text < l.background);
    }
}
