//! The "Treemap" section of the Preferences window (G-018): cushion shading, grid lines and the
//! cushion constants with live preview. The treemap behind the window repaints as soon as a slider
//! moves, because every change is an ordinary command (`treemap.cushion`, `treemap.grid`,
//! `treemap.look`) and the painter reads the settings each frame.
//!
//! This module holds the pure mapping from a user edit to a command call plus the drawing; the
//! window itself lives in `prefs`.

use crate::theme::Tokens;
use dirustat_engine::{CushionLook, TreemapSettings};
use serde_json::{Value, json};

/// Slider range of the root ridge height (the engine accepts `0..=4`; beyond 1.5 the picture is
/// mostly noise, so the slider stops there and the command stays the way to go further).
const HEIGHT_SLIDER_MAX: f32 = 1.5;
/// Elevation below this leaves the light grazing the surface; the slider stops here.
const MIN_ELEVATION_DEG: f32 = 5.0;
/// Azimuth used when the light points straight at the viewer (no horizontal direction).
const FALLBACK_AZIMUTH_DEG: f32 = 225.0;

/// One edit of the section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LookAction {
    Cushion(bool),
    Grid(bool),
    FreeSpace(bool),
    Height(f32),
    Scale(f32),
    Ambient(f32),
    Diffuse(f32),
    /// Direction of the light in degrees, 0 = from the right, 90 = from below, 225 = from the upper left.
    Azimuth(f32),
    /// Height of the light above the surface in degrees (90 = straight at the viewer).
    Elevation(f32),
    Reset,
}

/// Azimuth (`0..360`) and elevation (`0..=90`) in degrees of a light vector (x right, y down, z
/// towards the viewer). A vector without a horizontal part gets [`FALLBACK_AZIMUTH_DEG`].
pub fn light_angles(light: [f32; 3]) -> (f32, f32) {
    let [x, y, z] = light;
    if !(x.is_finite() && y.is_finite() && z.is_finite()) {
        return (FALLBACK_AZIMUTH_DEG, 90.0);
    }
    let horizontal = x.hypot(y);
    let azimuth = if horizontal < 1e-6 { FALLBACK_AZIMUTH_DEG } else { y.atan2(x).to_degrees().rem_euclid(360.0) };
    let elevation = z.atan2(horizontal).to_degrees().clamp(0.0, 90.0);
    (azimuth, elevation)
}

/// The unit light vector for an azimuth and elevation in degrees (the elevation is limited to
/// `MIN_ELEVATION_DEG..=90`, non-finite angles fall back to the defaults).
pub fn light_from_angles(azimuth: f32, elevation: f32) -> [f32; 3] {
    let az = if azimuth.is_finite() { azimuth } else { FALLBACK_AZIMUTH_DEG }.to_radians();
    let el = if elevation.is_finite() { elevation.clamp(MIN_ELEVATION_DEG, 90.0) } else { 60.0 }.to_radians();
    [az.cos() * el.cos(), az.sin() * el.cos(), el.sin()]
}

/// The command (id and params) an edit stands for. Light edits keep the other angle of `current`.
pub fn command_for(action: LookAction, current: &CushionLook) -> (&'static str, Value) {
    let look = |params: Value| ("treemap.look", params);
    match action {
        LookAction::Cushion(on) => ("treemap.cushion", json!({ "enabled": on })),
        LookAction::Grid(on) => ("treemap.grid", json!({ "enabled": on })),
        LookAction::FreeSpace(on) => ("treemap.free_space", json!({ "enabled": on })),
        LookAction::Height(v) => look(json!({ "height": v })),
        LookAction::Scale(v) => look(json!({ "scale": v })),
        LookAction::Ambient(v) => look(json!({ "ambient": v })),
        LookAction::Diffuse(v) => look(json!({ "diffuse": v })),
        LookAction::Azimuth(az) => look(json!({ "light": light_from_angles(az, light_angles(current.light).1) })),
        LookAction::Elevation(el) => look(json!({ "light": light_from_angles(light_angles(current.light).0, el) })),
        LookAction::Reset => look(json!({ "reset": true })),
    }
}

fn slider(ui: &mut egui::Ui, value: f32, range: std::ops::RangeInclusive<f32>, label: &str) -> Option<f32> {
    let mut v = value;
    // The formatter only changes the displayed text: `fixed_decimals` would round the stored value and
    // report a change on the first frame.
    let slider = egui::Slider::new(&mut v, range).text(label).custom_formatter(|n, _| format!("{n:.2}"));
    let response = ui.add(slider);
    response.changed().then_some(v)
}

/// Draws the section and returns the edit made this frame, if any.
pub fn show(ui: &mut egui::Ui, treemap: &TreemapSettings, tokens: &Tokens) -> Option<LookAction> {
    let mut action = None;
    ui.heading("Treemap");
    let mut cushion = treemap.cushion;
    if ui.checkbox(&mut cushion, "Cushion shading").changed() {
        action = Some(LookAction::Cushion(cushion));
    }
    let mut grid = treemap.grid;
    if ui.checkbox(&mut grid, "Grid lines").changed() {
        action = Some(LookAction::Grid(grid));
    }
    let mut free = treemap.show_free_space;
    if ui.checkbox(&mut free, "Free space band").changed() {
        action = Some(LookAction::FreeSpace(free));
    }
    ui.add_enabled_ui(treemap.cushion, |ui| {
        let look = treemap.look.sanitized();
        let (azimuth, elevation) = light_angles(look.light);
        ui.colored_label(Tokens::color(tokens.text_muted), "Cushion look (applied to the treemap as you drag)");
        if let Some(v) = slider(ui, look.height, 0.0..=HEIGHT_SLIDER_MAX, "Height") {
            action = Some(LookAction::Height(v));
        }
        if let Some(v) = slider(ui, look.scale, 0.0..=1.0, "Scale per level") {
            action = Some(LookAction::Scale(v));
        }
        if let Some(v) = slider(ui, look.ambient, 0.0..=1.0, "Ambient light") {
            action = Some(LookAction::Ambient(v));
        }
        if let Some(v) = slider(ui, look.diffuse, 0.0..=1.0, "Diffuse light") {
            action = Some(LookAction::Diffuse(v));
        }
        if let Some(v) = slider(ui, azimuth, 0.0..=359.0, "Light direction") {
            action = Some(LookAction::Azimuth(v));
        }
        if let Some(v) = slider(ui, elevation, MIN_ELEVATION_DEG..=90.0, "Light elevation") {
            action = Some(LookAction::Elevation(v));
        }
        if ui.button("Reset cushion look").clicked() {
            action = Some(LookAction::Reset);
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_round_trip_and_cover_the_default_light() {
        let d = CushionLook::default();
        let (az, el) = light_angles(d.light);
        assert!((az - 225.0).abs() < 0.01, "default light comes from the upper left, got {az}");
        assert!((el - 60.5).abs() < 0.1, "got {el}");
        for (a, e) in [(0.0, 45.0), (90.0, 10.0), (225.0, 60.0), (359.0, 90.0)] {
            let v = light_from_angles(a, e);
            let (a2, e2) = light_angles(v);
            let da = ((a2 - a + 540.0).rem_euclid(360.0) - 180.0).abs();
            assert!(da < 0.01 || e >= 89.9, "azimuth {a} came back as {a2}");
            assert!((e2 - e).abs() < 0.01, "elevation {e} came back as {e2}");
            assert!((v[0] * v[0] + v[1] * v[1] + v[2] * v[2] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn hostile_angles_and_vectors_give_usable_results() {
        assert_eq!(light_angles([0.0, 0.0, 1.0]), (FALLBACK_AZIMUTH_DEG, 90.0));
        assert_eq!(light_angles([f32::NAN, 0.0, 1.0]), (FALLBACK_AZIMUTH_DEG, 90.0));
        for (a, e) in [(f32::NAN, f32::NAN), (1e30, -1e30), (f32::INFINITY, 1e30), (-720.0, 0.0)] {
            let v = light_from_angles(a, e);
            assert!(v.iter().all(|c| c.is_finite()) && v[2] > 0.0, "{a} {e} -> {v:?}");
        }
        // Whatever the slider produces, the engine accepts the light.
        let mut s = dirustat_engine::Session::new();
        let (id, params) = command_for(LookAction::Elevation(f32::NAN), &CushionLook::default());
        assert!(s.execute(id, &params).is_ok(), "{params}");
    }

    #[test]
    fn every_edit_maps_to_a_command_the_engine_accepts() {
        let mut s = dirustat_engine::Session::new();
        let actions = [
            LookAction::Cushion(false),
            LookAction::Cushion(true),
            LookAction::Grid(true),
            LookAction::FreeSpace(true),
            LookAction::Height(0.7),
            LookAction::Scale(0.5),
            LookAction::Ambient(0.3),
            LookAction::Diffuse(0.8),
            LookAction::Azimuth(10.0),
            LookAction::Elevation(40.0),
        ];
        for a in actions {
            let current = s.settings().treemap.look;
            let (id, params) = command_for(a, &current);
            assert!(s.execute(id, &params).is_ok(), "{a:?}: {params}");
        }
        let t = s.settings().treemap;
        assert!(t.cushion && t.grid);
        assert_eq!((t.look.height, t.look.scale, t.look.ambient, t.look.diffuse), (0.7, 0.5, 0.3, 0.8));
        let (az, el) = light_angles(t.look.light);
        assert!((az - 10.0).abs() < 0.01 && (el - 40.0).abs() < 0.01, "{az} {el}");
        let (id, params) = command_for(LookAction::Reset, &t.look);
        assert!(s.execute(id, &params).is_ok());
        assert_eq!(s.settings().treemap.look, CushionLook::default());
    }

    #[test]
    fn the_section_draws_in_both_themes_with_hostile_stored_values() {
        let ctx = egui::Context::default();
        let hostile = TreemapSettings {
            look: CushionLook { height: f32::NAN, scale: 1e30, ambient: -5.0, diffuse: f32::INFINITY, light: [0.0; 3] },
            ..TreemapSettings::default()
        };
        for tokens in [Tokens::dark(), Tokens::light()] {
            for settings in [TreemapSettings::default(), hostile, TreemapSettings { cushion: false, ..hostile }] {
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 600.0))),
                    ..egui::RawInput::default()
                };
                let out = ctx.run_ui(raw, |ui| {
                    assert_eq!(show(ui, &settings, &tokens), None, "nothing changes without input");
                });
                assert!(!out.shapes.is_empty());
                out.drop_without_applying_deltas();
            }
        }
    }
}
