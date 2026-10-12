//! Test-only commands.

use super::{CommandSpec, always};
use crate::{CommandError, Session};
use serde_json::Value;

fn run_panic(_: &mut Session, _: &Value) -> Result<Value, CommandError> {
    panic!("deliberate test panic")
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "test.panic",
        label: "Test panic",
        menu: "",
        shortcut: None,
        params_doc: "none; always panics (tests only)",
        destructive: false,
        enabled: always,
        run: run_panic,
    }]
}
