//! The command registry (ADR 0003). Every user-visible action is a [`CommandSpec`]; UI, CLI,
//! control channel and MCP call them by id through [`Session::execute`].
//!
//! Each area lives in its own module with a `specs()` function; [`registry`] concatenates them.
//! Rules for `run`: never open a dialog, never panic (the guard is a safety net only), reject
//! unknown params fields with `InvalidParams` (use [`parse_params`] / [`no_params`]).

mod cleanup;
mod detached;
mod file;
mod list;
mod platform;
mod privileges;
mod scan;
mod search;
mod select;
mod session;
mod settings;
#[cfg(test)]
mod testing;
mod tree;
mod view;

pub use detached::{DetachedJob, JobOutcome, Prepared, execute_unlocked};
pub use privileges::{FastPathStatus, fast_path_status, volume_root_of};

use crate::{CommandError, Session, guard};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Static description of a command plus its entry points.
#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    /// Stable id, `group.action` (`scan.start`).
    pub id: &'static str,
    pub label: &'static str,
    /// Menu path (`File/Scan folder`); empty for commands without a menu entry.
    pub menu: &'static str,
    pub shortcut: Option<&'static str>,
    /// Human- and agent-readable description of the params object, every field documented.
    pub params_doc: &'static str,
    /// Destructive commands require `confirm: true` and follow `docs/safety.md`.
    pub destructive: bool,
    /// Whether the command can run in the current session state.
    pub enabled: fn(&Session) -> bool,
    pub run: fn(&mut Session, &Value) -> Result<Value, CommandError>,
}

/// Serializable description of a command for `engine.commands` and MCP.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static str,
    pub shortcut: Option<&'static str>,
    pub params_doc: &'static str,
    pub destructive: bool,
    /// `enabled` evaluated against the session at the time of the call.
    pub enabled: bool,
}

/// `enabled` predicate for commands that are always available.
pub(crate) fn always(_: &Session) -> bool {
    true
}

/// All registered commands.
pub(crate) fn registry() -> &'static [CommandSpec] {
    static REGISTRY: OnceLock<Vec<CommandSpec>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut all = Vec::new();
        all.extend(scan::specs());
        all.extend(select::specs());
        all.extend(view::specs());
        all.extend(session::specs());
        all.extend(settings::specs());
        all.extend(tree::specs());
        all.extend(list::specs());
        all.extend(privileges::specs());
        all.extend(search::specs());
        all.extend(platform::specs());
        all.extend(file::specs());
        all.extend(cleanup::specs());
        #[cfg(test)]
        all.extend(testing::specs());
        all
    })
}

/// Strict params parsing: the value must match `T`, and `T` should use `deny_unknown_fields`.
pub(crate) fn parse_params<T: DeserializeOwned>(params: &Value) -> Result<T, CommandError> {
    T::deserialize(params).map_err(|e| CommandError::InvalidParams(e.to_string()))
}

/// For commands without params: accepts only an empty object.
pub(crate) fn no_params(params: &Value) -> Result<(), CommandError> {
    match params {
        Value::Object(map) if map.is_empty() => Ok(()),
        _ => Err(CommandError::InvalidParams("this command takes no params".to_owned())),
    }
}

/// Looks `id` up in `specs` and runs it under the panic guard: enabled check, `confirm` handling
/// for destructive commands, then `run`. `null` params mean an empty object.
pub(crate) fn execute_in(
    session: &mut Session,
    specs: &[CommandSpec],
    id: &str,
    params: &Value,
) -> Result<Value, CommandError> {
    let spec = specs.iter().find(|s| s.id == id).ok_or_else(|| CommandError::UnknownCommand(id.to_owned()))?;
    guard::guarded(id, || {
        let params = admit(session, spec, params)?;
        (spec.run)(session, &params)
    })
}

/// The checks every command passes before `run`: `enabled`, the busy gate (no destructive command
/// and no scan while a detached permanent deletion runs), and `confirm` for destructive commands.
/// Returns the params `run` gets (`null` becomes `{}`, `confirm` is stripped).
pub(crate) fn admit(session: &Session, spec: &CommandSpec, params: &Value) -> Result<Value, CommandError> {
    if !(spec.enabled)(session) {
        return Err(CommandError::NotEnabled(spec.id.to_owned()));
    }
    if session.removal_in_progress() && (spec.destructive || spec.id.starts_with("scan.")) {
        return Err(CommandError::NotEnabled(format!(
            "{}: a deletion is still running; wait for it to finish",
            spec.id
        )));
    }
    let empty = Value::Object(Map::new());
    let params = if params.is_null() { &empty } else { params };
    if !spec.destructive {
        return Ok(params.clone());
    }
    let Value::Object(map) = params else {
        return Err(CommandError::ConfirmationRequired(spec.id.to_owned()));
    };
    if map.get("confirm") != Some(&Value::Bool(true)) {
        return Err(CommandError::ConfirmationRequired(spec.id.to_owned()));
    }
    let mut rest = map.clone();
    rest.remove("confirm");
    Ok(Value::Object(rest))
}

impl Session {
    /// Runs the command `id` with JSON `params` (`null` = no params) under the last-resort panic
    /// guard. Long commands (`scan.start`) return at once.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value, CommandError> {
        execute_in(self, registry(), id, params)
    }

    /// Describes every command, with `enabled` evaluated for the current state.
    pub fn commands(&self) -> Vec<CommandInfo> {
        registry()
            .iter()
            .map(|s| CommandInfo {
                id: s.id,
                label: s.label,
                menu: s.menu,
                shortcut: s.shortcut,
                params_doc: s.params_doc,
                destructive: s.destructive,
                enabled: (s.enabled)(self),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn every_spec_is_well_formed_and_ids_are_unique() {
        let mut seen = HashSet::new();
        for s in registry() {
            assert!(!s.label.is_empty() && !s.params_doc.is_empty(), "{}", s.id);
            assert!(seen.insert(s.id), "duplicate id {}", s.id);
            let (group, action) = s.id.split_once('.').unwrap_or(("", ""));
            let ok =
                |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            assert!(ok(group) && ok(action), "id {} is not group.action", s.id);
        }
    }

    #[test]
    fn commands_are_serializable_and_report_enabled() {
        let s = Session::new();
        let infos = s.commands();
        assert_eq!(infos.len(), registry().len());
        let json = serde_json::to_value(&infos).unwrap();
        assert!(json.is_array());
        let cancel = infos.iter().find(|i| i.id == "scan.cancel").unwrap();
        assert!(!cancel.enabled);
        let start = infos.iter().find(|i| i.id == "scan.start").unwrap();
        assert!(start.enabled);
    }

    #[test]
    fn error_codes_match_the_protocol() {
        let codes = [
            (CommandError::UnknownCommand(String::new()), "unknown_command"),
            (CommandError::InvalidParams(String::new()), "invalid_params"),
            (CommandError::Unsupported(String::new()), "unsupported"),
            (CommandError::ConfirmationRequired(String::new()), "confirmation_required"),
            (CommandError::NotEnabled(String::new()), "not_enabled"),
            (CommandError::Scan(String::new()), "internal"),
            (CommandError::Platform(String::new()), "internal"),
            (CommandError::ProtectedPath(String::new()), "protected_path"),
            (CommandError::StaleTree(String::new()), "stale_tree"),
            (CommandError::Panicked { what: String::new(), message: String::new() }, "internal"),
        ];
        for (err, code) in codes {
            assert_eq!(err.code(), code);
        }
    }

    #[test]
    fn invalid_json_text_is_invalid_params() {
        let mut s = Session::new();
        let err = s.run("session.state", "{not json").unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        assert!(s.run("session.state", "").is_ok());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut s = Session::new();
        let err = s.execute("select.node", &json!({"id": 0, "bogus": 1})).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        let err = s.execute("session.state", &json!({"bogus": 1})).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
    }

    fn echo_params(_: &mut Session, params: &Value) -> Result<Value, CommandError> {
        Ok(params.clone())
    }

    #[test]
    fn destructive_commands_need_confirm_and_it_is_stripped() {
        let specs = [CommandSpec {
            id: "file.fake",
            label: "Fake",
            menu: "",
            shortcut: None,
            params_doc: "x",
            destructive: true,
            enabled: always,
            run: echo_params,
        }];
        let mut s = Session::new();
        for p in [json!(null), json!({}), json!({"confirm": false}), json!({"confirm": "yes"}), json!([true])] {
            let err = execute_in(&mut s, &specs, "file.fake", &p).unwrap_err();
            assert_eq!(err.code(), "confirmation_required");
        }
        let ok = execute_in(&mut s, &specs, "file.fake", &json!({"confirm": true, "a": 1})).unwrap();
        assert_eq!(ok, json!({"a": 1}));
    }

    #[test]
    fn hostile_params_never_panic() {
        let long = "x".repeat(100_000);
        let mut nested = json!(1);
        for _ in 0..100 {
            nested = json!({ "a": [nested] });
        }
        let mut corpus = vec![
            json!(null),
            json!(true),
            json!(0),
            json!(-1),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(1e308),
            json!(-0.0),
            json!(f64::NAN),
            json!(f64::INFINITY),
            json!(""),
            json!(long),
            json!("\u{0}"),
            json!("\u{202e}\u{fffd}\u{65e5}\u{672c}\u{1f980}"),
            json!("../../../../no_such_dir_dirustat_hostile/../.."),
            json!("..\\..\\..\\no_such_dir_dirustat_hostile\\..\\.."),
            json!("\\\\?\\GLOBALROOT\\no_such_dir_dirustat_hostile"),
            json!("no_such_dir_dirustat_hostile/\u{0}/x"),
            json!([]),
            json!([1, "a", null]),
            json!({}),
            json!({"unknown_field": 1}),
            json!({"confirm": true}),
            json!({"": ""}),
            nested,
        ];
        let seeds = corpus.clone();
        for key in ["path", "id", "mode", "confirm", "node", "limit"] {
            for v in &seeds {
                corpus.push(json!({ key: v }));
            }
        }
        let mut s = Session::new();
        for spec in registry().iter().filter(|c| !c.id.starts_with("test.")) {
            for params in &corpus {
                if let Err(CommandError::Panicked { what, message }) = s.execute(spec.id, params) {
                    panic!("{} panicked ({what}): {message}", spec.id);
                }
            }
            // The session stays usable after every command.
            assert!(s.execute("session.state", &json!({})).is_ok(), "after {}", spec.id);
        }
        assert_eq!(s.tree().len(), 1);
    }
}
