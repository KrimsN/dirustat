//! `view.*` commands that change how the session presents data.

use super::{CommandSpec, always, parse_params};
use crate::{CommandError, Session};
use dirustat_model::SizeMode;
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "view.size_mode",
        label: "Size mode",
        menu: "View/Size mode",
        shortcut: None,
        params_doc: "{\"mode\": \"logical\" | \"allocated\"} sizes shown on the map and in statistics.",
        destructive: false,
        enabled: always,
        run: run_size_mode,
    }]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeModeParams {
    mode: ModeName,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModeName {
    Logical,
    Allocated,
}

fn run_size_mode(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: SizeModeParams = parse_params(params)?;
    let (mode, name) = match p.mode {
        ModeName::Logical => (SizeMode::Logical, "logical"),
        ModeName::Allocated => (SizeMode::Allocated, "allocated"),
    };
    session.set_map_size_mode(mode);
    Ok(json!({ "mode": name }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_mode_switches_and_validates() {
        let mut s = Session::new();
        assert_eq!(s.map_size_mode(), SizeMode::Allocated);
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        assert_eq!(s.map_size_mode(), SizeMode::Logical);
        assert_eq!(s.state_json()["size_mode"], "logical");
        for bad in [json!({}), json!({"mode": "bogus"}), json!({"mode": 1}), json!({"mode": "logical", "x": 1})] {
            let err = s.execute("view.size_mode", &bad).unwrap_err();
            assert_eq!(err.code(), "invalid_params");
        }
        assert_eq!(s.map_size_mode(), SizeMode::Logical);
    }
}
