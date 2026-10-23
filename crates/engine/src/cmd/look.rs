//! `treemap.look`, `treemap.grid` and `treemap.free_space`: the cushion constants, the tile grid and the
//! free-space band (G-018). Like
//! `treemap.cushion` they change settings only; the shell reads the settings when it paints.

use super::{CommandSpec, always, parse_params};
use crate::look::LookPatch;
use crate::{CommandError, Session};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "treemap.look",
            label: "Cushion look",
            menu: "",
            shortcut: None,
            params_doc: "{\"height\": 0..=4 (root ridge height, default 0.22), \"scale\": 0..=1 (height factor per \
                nesting level, 0.85), \"ambient\": 0..=1 (0.4), \"diffuse\": 0..=1 (0.6), \"light\": [x, y, z] (direction \
                towards the light, x right, y down, z towards the viewer; |x|,|y| <= 100, z in 0..=100, length >= 0.001; \
                default [-1, -1, 2.5]), \"reset\": bool (restore the defaults first)}, every field optional; an \
                empty object only reports the look. A value out of range is invalid_params and changes nothing \
                (settings files are clamped instead). Returns {look} (setting treemap.look).",
            destructive: false,
            enabled: always,
            run: run_look,
        },
        CommandSpec {
            id: "treemap.grid",
            label: "Grid lines",
            menu: "View/Grid lines",
            shortcut: None,
            params_doc: "{\"enabled\": bool (optional, omitted = toggle)} 1-pixel dark lines along the right and \
                bottom edge of every file tile (setting treemap.grid, default off). Returns {enabled}.",
            destructive: false,
            enabled: always,
            run: run_grid,
        },
        CommandSpec {
            id: "treemap.free_space",
            label: "Free space",
            menu: "View/Free space",
            shortcut: None,
            params_doc: "{\"enabled\": bool (optional, omitted = toggle)} shows the free space of the scanned \
                volume as a band at the right edge of the treemap, its width proportional to free / (scanned + free) \
                (setting treemap.show_free_space, default off). The volume is asked in the background; nothing is \
                shown for a root that is not on a local drive or when the query fails. Returns {enabled}.",
            destructive: false,
            enabled: always,
            run: run_free_space,
        },
    ]
}

fn run_look(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let patch: LookPatch = parse_params(params)?;
    let mut next = session.settings().clone();
    next.treemap.look = patch.apply(next.treemap.look).map_err(CommandError::InvalidParams)?;
    session.apply_settings(next);
    Ok(json!({ "look": session.settings().treemap.look }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GridParams {
    #[serde(default)]
    enabled: Option<bool>,
}

fn run_grid(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: GridParams = parse_params(params)?;
    let mut next = session.settings().clone();
    let enabled = p.enabled.unwrap_or(!next.treemap.grid);
    next.treemap.grid = enabled;
    session.apply_settings(next);
    Ok(json!({ "enabled": enabled }))
}

fn run_free_space(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: GridParams = parse_params(params)?;
    let mut next = session.settings().clone();
    let enabled = p.enabled.unwrap_or(!next.treemap.show_free_space);
    next.treemap.show_free_space = enabled;
    session.apply_settings(next);
    Ok(json!({ "enabled": enabled }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CushionLook;

    #[test]
    fn look_reports_patches_resets_and_persists_in_settings() {
        let mut s = Session::new();
        let out = s.execute("treemap.look", &json!({})).unwrap();
        assert_eq!(out["look"]["height"], json!(0.22), "shortest decimal, not the f32 widening");
        let out = s.execute("treemap.look", &json!({"height": 1.5, "light": [1, 0, 1]})).unwrap();
        assert_eq!(out["look"]["light"], json!([1.0, 0.0, 1.0]));
        assert_eq!(s.settings().treemap.look.height, 1.5);
        assert_eq!(s.settings().treemap.look.scale, CushionLook::default().scale, "untouched field");
        s.execute("treemap.look", &json!({"reset": true, "ambient": 0.25})).unwrap();
        let look = s.settings().treemap.look;
        assert_eq!((look.height, look.ambient), (CushionLook::default().height, 0.25));
        // The look reaches settings.get and survives a settings.set of another key.
        s.execute("settings.set", &json!({"key": "treemap.grid", "value": true})).unwrap();
        let got = s.execute("settings.get", &json!({})).unwrap();
        assert_eq!(got["settings"]["treemap"]["look"]["ambient"], json!(0.25));
    }

    #[test]
    fn look_rejects_hostile_values_and_changes_nothing() {
        let mut s = Session::new();
        s.execute("treemap.look", &json!({"height": 2.0})).unwrap();
        let before = s.settings().clone();
        for bad in [
            json!({"height": 5}),
            json!({"height": -1}),
            json!({"scale": 1.01}),
            json!({"ambient": "x"}),
            json!({"diffuse": null, "scale": 7}),
            json!({"height": 1e39}),
            json!({"light": [0, 0, 0]}),
            json!({"light": [0, 0, -1]}),
            json!({"light": [1, 2]}),
            json!({"light": [1e9, 0, 1]}),
            json!({"unknown": 1}),
            json!({"height": 1.0, "light": [0, 0, 0]}),
            json!([1]),
            json!("x"),
        ] {
            assert_eq!(s.execute("treemap.look", &bad).unwrap_err().code(), "invalid_params", "{bad}");
        }
        assert_eq!(s.settings(), &before, "a refused call leaves every setting alone");
    }

    #[test]
    fn settings_set_accepts_the_look_patch_with_the_same_rules() {
        let mut s = Session::new();
        s.execute("settings.set", &json!({"key": "treemap.look", "value": {"scale": 0.5}})).unwrap();
        assert_eq!(s.settings().treemap.look.scale, 0.5);
        let bad = json!({"key": "treemap.look", "value": {"scale": 9}});
        assert_eq!(s.execute("settings.set", &bad).unwrap_err().code(), "invalid_params");
        assert_eq!(s.settings().treemap.look.scale, 0.5);
        s.execute("settings.reset", &json!({})).unwrap();
        assert_eq!(s.settings().treemap.look, CushionLook::default());
    }

    #[test]
    fn grid_defaults_off_toggles_sets_and_matches_settings_set() {
        let mut s = Session::new();
        assert!(!s.settings().treemap.grid);
        assert_eq!(s.execute("treemap.grid", &json!({})).unwrap(), json!({"enabled": true}));
        assert!(s.settings().treemap.grid);
        s.execute("treemap.grid", &json!({"enabled": true})).unwrap();
        assert!(s.settings().treemap.grid, "explicit value is idempotent");
        assert_eq!(s.execute("treemap.grid", &json!(null)).unwrap(), json!({"enabled": false}));
        s.execute("settings.set", &json!({"key": "treemap.grid", "value": true})).unwrap();
        assert_eq!(s.execute("treemap.grid", &json!({})).unwrap(), json!({"enabled": false}));
        for bad in [json!({"enabled": "yes"}), json!({"x": 1}), json!("on"), json!(3)] {
            assert_eq!(s.execute("treemap.grid", &bad).unwrap_err().code(), "invalid_params", "{bad}");
        }
    }

    #[test]
    fn a_hostile_settings_file_is_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let json = r#"{"treemap":{"look":{"height":900,"scale":-2,"ambient":5,"diffuse":0.3,"light":[0,0,0]}}}"#;
        std::fs::write(&path, json).unwrap();
        let out = crate::Settings::load_from(&path);
        assert_eq!(out.warning, None);
        let look = out.settings.treemap.look;
        assert_eq!((look.height, look.scale, look.ambient, look.diffuse), (4.0, 0.0, 1.0, 0.3));
        assert_eq!(look.light, CushionLook::default().light);
        let cfg = look.config();
        assert!(cfg.height.is_finite() && cfg.light.iter().all(|v| v.is_finite()));
    }
}
