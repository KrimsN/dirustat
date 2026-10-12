//! `session.*` commands and the serializable session state.

use super::{CommandSpec, always, no_params};
use crate::{CommandError, Session};
use serde_json::{Value, json};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "session.state",
            label: "Session state",
            menu: "",
            shortcut: None,
            params_doc: "none. Returns revision, node count, scanning flag, progress, root, selection and size mode.",
            destructive: false,
            enabled: always,
            run: run_state,
        },
        CommandSpec {
            id: "session.poll",
            label: "Poll scan",
            menu: "",
            shortcut: None,
            params_doc: "none. Finishes a completed scan without blocking; returns {replaced, scanning}.",
            destructive: false,
            enabled: always,
            run: run_poll,
        },
    ]
}

fn run_state(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    Ok(session.state_json())
}

fn run_poll(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    let replaced = session.poll_scan()?;
    Ok(json!({ "replaced": replaced, "scanning": session.is_scanning() }))
}

impl Session {
    /// Session state for `session.state` and the control channel.
    pub fn state_json(&self) -> Value {
        let progress = self.scan.as_ref().map(|p| {
            let pr = p.handle.progress();
            json!({ "dirs": pr.dirs, "files": pr.files, "bytes": pr.bytes, "errors": pr.errors })
        });
        let scan_root = self.scan.as_ref().map(|p| p.root.as_str());
        let view = self.map_state_view();
        json!({
            "revision": self.revision(),
            "nodes": self.tree().len(),
            "root": self.tree().get(self.tree().root()).map(|n| n.name()),
            "scanning": self.is_scanning(),
            "scan_root": scan_root,
            "progress": progress,
            "selection": view.selected,
            "selection_name": view.selected_name,
            "size_mode": view.size_mode,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_session_state() {
        let mut s = Session::new();
        let v = s.execute("session.state", &json!(null)).unwrap();
        assert_eq!(v["nodes"], 1);
        assert_eq!(v["scanning"], false);
        assert_eq!(v["revision"], 0);
        assert!(v["progress"].is_null());
        assert_eq!(v["size_mode"], "allocated");
    }

    #[test]
    fn poll_without_scan_does_nothing() {
        let mut s = Session::new();
        let v = s.execute("session.poll", &json!({})).unwrap();
        assert_eq!(v, json!({"replaced": false, "scanning": false}));
    }

    #[test]
    fn panic_command_is_guarded() {
        let mut s = Session::new();
        let err = s.run("test.panic", "{}").unwrap_err();
        assert!(matches!(&err, CommandError::Panicked { what, .. } if what == "test.panic"));
        assert_eq!(err.code(), "internal");
    }
}
