//! `settings.*` commands and the session's settings accessors. Persistence to disk is done by the
//! shell with [`Settings::load_from`] / [`Settings::save_to`]; commands never touch the disk.

use super::{CommandSpec, always, no_params, parse_params};
use crate::settings::{MAX_PATH_LEN, MAX_RECENT_PATHS};
use crate::{CommandError, Session, Settings};
use serde::Deserialize;
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "settings.get",
            label: "Get settings",
            menu: "",
            shortcut: None,
            params_doc: "none. Returns {settings}: version, size_mode, ext_sort, tree_sort, treemap, recent_paths, prefer_fast_path, exclude, cleanups (edit them with cleanup.save / cleanup.remove).",
            destructive: false,
            enabled: always,
            run: run_get,
        },
        CommandSpec {
            id: "settings.set",
            label: "Change setting",
            menu: "",
            shortcut: None,
            params_doc: "{\"key\": string, \"value\": any} keys: size_mode (\"logical\"|\"allocated\"), \
                ext_sort / tree_sort ({\"key\": \"size\"|\"name\"|\"count\", \"descending\": bool}), \
                treemap.cushion / treemap.grid / treemap.show_free_space (bool), \
                scan.prefer_fast_path (bool, default true), \
                scan.exclude (array of at most 256 non-empty wildcard patterns, default []), \
                recent_paths (array of at most 20 non-empty strings). Unknown key is invalid_params.",
            destructive: false,
            enabled: always,
            run: run_set,
        },
        CommandSpec {
            id: "settings.reset",
            label: "Reset settings",
            menu: "",
            shortcut: None,
            params_doc: "none. Restores every setting to its default; returns {settings}.",
            destructive: false,
            enabled: always,
            run: run_reset,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetParams {
    key: String,
    value: Value,
}

fn invalid(what: String) -> CommandError {
    CommandError::InvalidParams(what)
}

fn typed<T: serde::de::DeserializeOwned>(key: &str, value: &Value) -> Result<T, CommandError> {
    T::deserialize(value).map_err(|e| invalid(format!("bad value for {key}: {e}")))
}

fn paths(value: &Value) -> Result<Vec<String>, CommandError> {
    let list: Vec<String> = typed("recent_paths", value)?;
    if list.len() > MAX_RECENT_PATHS {
        return Err(invalid(format!("recent_paths holds at most {MAX_RECENT_PATHS} entries")));
    }
    if list.iter().any(|p| p.is_empty() || p.len() > MAX_PATH_LEN) {
        return Err(invalid(format!("recent_paths entries must be 1..={MAX_PATH_LEN} bytes")));
    }
    Ok(list)
}

fn apply_key(s: &mut Settings, key: &str, value: &Value) -> Result<(), CommandError> {
    match key {
        "size_mode" => s.size_mode = typed(key, value)?,
        "ext_sort" => s.ext_sort = typed(key, value)?,
        "tree_sort" => s.tree_sort = typed(key, value)?,
        "treemap.cushion" => s.treemap.cushion = typed(key, value)?,
        "treemap.grid" => s.treemap.grid = typed(key, value)?,
        "treemap.show_free_space" => s.treemap.show_free_space = typed(key, value)?,
        "scan.prefer_fast_path" => s.prefer_fast_path = typed(key, value)?,
        "scan.exclude" => s.exclude = super::scan::checked_excludes(typed(key, value)?)?,
        "recent_paths" => s.recent_paths = paths(value)?,
        _ => return Err(invalid(format!("unknown settings key: {}", key.chars().take(100).collect::<String>()))),
    }
    Ok(())
}

fn settings_json(session: &Session) -> Value {
    json!({ "settings": session.settings() })
}

fn run_get(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    Ok(settings_json(session))
}

fn run_set(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: SetParams = parse_params(params)?;
    let mut next = session.settings().clone();
    apply_key(&mut next, &p.key, &p.value)?;
    session.apply_settings(next);
    Ok(settings_json(session))
}

fn run_reset(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    session.apply_settings(Settings::default());
    Ok(settings_json(session))
}

impl Session {
    /// Current settings (kept in sync with the map size mode).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Replaces the settings (sanitized) and applies the ones the session acts on.
    pub fn apply_settings(&mut self, mut settings: Settings) {
        settings.sanitize();
        let mode = settings.size_mode.mode();
        self.settings = settings;
        self.set_map_size_mode(mode);
    }

    /// Remembers a scanned path at the front of the recent list.
    pub fn remember_path(&mut self, path: &str) {
        self.settings.push_recent(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_model::SizeMode;

    #[test]
    fn get_set_reset_round_trip() {
        let mut s = Session::new();
        let got = s.execute("settings.get", &json!({})).unwrap();
        assert_eq!(got["settings"]["size_mode"], "allocated");
        assert_eq!(got["settings"]["version"], 1);
        s.execute("settings.set", &json!({"key": "size_mode", "value": "logical"})).unwrap();
        assert_eq!(s.map_size_mode(), SizeMode::Logical);
        s.execute("settings.set", &json!({"key": "treemap.grid", "value": true})).unwrap();
        s.execute("settings.set", &json!({"key": "ext_sort", "value": {"key": "name", "descending": false}})).unwrap();
        let out = s.execute("settings.set", &json!({"key": "recent_paths", "value": ["a", "b"]})).unwrap();
        assert_eq!(out["settings"]["recent_paths"], json!(["a", "b"]));
        assert_eq!(out["settings"]["treemap"]["grid"], true);
        assert_eq!(out["settings"]["ext_sort"]["key"], "name");
        let out = s.execute("settings.reset", &json!({})).unwrap();
        assert_eq!(out["settings"]["size_mode"], "allocated");
        assert_eq!(s.map_size_mode(), SizeMode::Allocated);
        assert_eq!(s.settings(), &Settings::default());
    }

    #[test]
    fn prefer_fast_path_defaults_on_and_old_files_load() {
        let mut s = Session::new();
        assert!(s.settings().prefer_fast_path);
        let out = s.execute("settings.set", &json!({"key": "scan.prefer_fast_path", "value": false})).unwrap();
        assert_eq!(out["settings"]["prefer_fast_path"], false);
        let bad = json!({"key": "scan.prefer_fast_path", "value": "no"});
        assert_eq!(s.execute("settings.set", &bad).unwrap_err().code(), "invalid_params");
        let old: Settings = serde_json::from_str(r#"{"version":1,"size_mode":"logical"}"#).unwrap();
        assert!(old.prefer_fast_path);
    }

    #[test]
    fn view_size_mode_keeps_settings_in_sync() {
        let mut s = Session::new();
        s.execute("view.size_mode", &json!({"mode": "logical"})).unwrap();
        assert_eq!(s.execute("settings.get", &json!({})).unwrap()["settings"]["size_mode"], "logical");
    }

    #[test]
    fn invalid_params_leave_settings_untouched() {
        let mut s = Session::new();
        let too_many: Vec<String> = (0..=MAX_RECENT_PATHS).map(|i| i.to_string()).collect();
        let bad = [
            json!({}),
            json!({"key": "size_mode"}),
            json!({"key": "nope", "value": 1}),
            json!({"key": "size_mode", "value": "huge"}),
            json!({"key": "size_mode", "value": 1}),
            json!({"key": "treemap.grid", "value": "yes"}),
            json!({"key": "ext_sort", "value": {"key": "size"}}),
            json!({"key": "ext_sort", "value": {"key": "size", "descending": true, "x": 1}}),
            json!({"key": "recent_paths", "value": too_many}),
            json!({"key": "recent_paths", "value": [""]}),
            json!({"key": "recent_paths", "value": [1]}),
            json!({"key": "size_mode", "value": "logical", "extra": 1}),
        ];
        for p in bad {
            assert_eq!(s.execute("settings.set", &p).unwrap_err().code(), "invalid_params", "{p}");
        }
        assert_eq!(s.settings(), &Settings::default());
        assert_eq!(s.execute("settings.get", &json!({"x": 1})).unwrap_err().code(), "invalid_params");
        assert_eq!(s.execute("settings.reset", &json!({"x": 1})).unwrap_err().code(), "invalid_params");
    }

    #[test]
    fn remember_path_is_bounded() {
        let mut s = Session::new();
        for i in 0..50 {
            s.remember_path(&format!("p{i}"));
        }
        assert_eq!(s.settings().recent_paths.len(), MAX_RECENT_PATHS);
    }
}
