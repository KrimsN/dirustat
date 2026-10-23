//! `treemap.*` commands: how the treemap is drawn. They change settings only; the shell reads
//! the settings when it paints.

use super::{CommandSpec, always, parse_params};
use crate::{CommandError, Session};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "treemap.cushion",
        label: "Cushion shading",
        menu: "View/Cushion shading",
        shortcut: None,
        params_doc: "{\"enabled\": bool (optional, omitted = toggle)} cushion shading of the treemap \
            (setting treemap.cushion, default on); off draws flat tiles. Returns {enabled}.",
        destructive: false,
        enabled: always,
        run: run_cushion,
    }]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CushionParams {
    #[serde(default)]
    enabled: Option<bool>,
}

fn run_cushion(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: CushionParams = parse_params(params)?;
    let mut next = session.settings().clone();
    let enabled = p.enabled.unwrap_or(!next.treemap.cushion);
    next.treemap.cushion = enabled;
    session.apply_settings(next);
    Ok(json!({ "enabled": enabled }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cushion_defaults_on_toggles_and_sets() {
        let mut s = Session::new();
        assert!(s.settings().treemap.cushion);
        let out = s.execute("treemap.cushion", &json!({})).unwrap();
        assert_eq!(out["enabled"], false);
        assert!(!s.settings().treemap.cushion);
        s.execute("treemap.cushion", &json!({"enabled": true})).unwrap();
        assert!(s.settings().treemap.cushion);
        s.execute("treemap.cushion", &json!({"enabled": true})).unwrap();
        assert!(s.settings().treemap.cushion, "an explicit value is idempotent");
        s.execute("treemap.cushion", &json!(null)).unwrap();
        assert!(!s.settings().treemap.cushion, "null params toggle too");
    }

    #[test]
    fn cushion_rejects_bad_params_and_stays_in_sync_with_settings_set() {
        let mut s = Session::new();
        for bad in [json!({"enabled": "yes"}), json!({"enabled": 1}), json!({"x": 1}), json!("on")] {
            assert_eq!(s.execute("treemap.cushion", &bad).unwrap_err().code(), "invalid_params", "{bad}");
        }
        assert!(s.settings().treemap.cushion);
        s.execute("settings.set", &json!({"key": "treemap.cushion", "value": false})).unwrap();
        let out = s.execute("treemap.cushion", &json!({})).unwrap();
        assert_eq!(out["enabled"], true);
    }
}
