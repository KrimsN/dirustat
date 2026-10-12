//! L3: control channel (JSON over TCP on 127.0.0.1 with a key) and MCP server.
//!
//! Status: skeleton (milestone M2). Protocol: `docs/control-protocol.md`. Every action available to
//! a human in the UI must be available to an agent via `engine.execute`; window screenshots via `ui.screenshot`.

use dirustat_engine::{CommandError, Session};

/// Control channel protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Handler stub: for now it only runs the command in the session.
pub fn execute(session: &mut Session, command: &str, params: &str) -> Result<(), CommandError> {
    session.run(command, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_surfaces_error() {
        let mut s = Session::new();
        assert!(execute(&mut s, "missing", "{}").is_err());
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
