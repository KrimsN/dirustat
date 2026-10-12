//! Headless CLI. Subcommands (M2): `scan`, `info`, `run`, `commands`, `mcp`. No egui, no UI crates.
#![allow(clippy::print_stdout)]

use dirustat_engine::Session;

fn main() {
    println!("dirustat-cli: {} nodes in empty session", Session::new().tree().len());
}
