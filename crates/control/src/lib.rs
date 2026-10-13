//! L3: control channel (JSON lines over TCP on 127.0.0.1 with a key) and MCP server.
//!
//! Protocol: `docs/control-protocol.md`. Every action available to a human in the UI is available
//! to an agent via `engine.execute`; UI-only methods go through a [`UiBridge`] implemented by the
//! shell. The [`Dispatcher`] is transport-agnostic; [`ControlServer`] serves it over loopback TCP
//! and [`serve_mcp`] serves it as MCP over stdio.

mod bridge;
mod client;
mod dispatcher;
pub mod error;
mod key;
mod line;
mod mcp;
mod server;
mod tree;

pub use bridge::{NullBridge, Region, ScreenshotRequest, UiBridge};
pub use client::ControlClient;
pub use dispatcher::{
    Backend, Dispatcher, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, SharedSession, constant_time_eq, lock_session, share,
};
pub use error::ControlError;
pub use key::{default_key_dir, generate_key, key_file_name, read_key_file, write_key_file};
pub use mcp::{serve_mcp, serve_mcp_connect, serve_mcp_with};
pub use server::{ControlServer, MAX_CONNECTIONS, RATE_LIMIT};
pub use tree::{MAX_DEPTH, MAX_LIMIT};

/// Control channel protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_stable() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
