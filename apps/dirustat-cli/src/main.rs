//! Headless CLI. Подкоманды (M2): `scan`, `info`, `run`, `commands`, `mcp`. Без egui и без UI-крейтов.
#![allow(clippy::print_stdout)]

use dirustat_engine::Session;

fn main() {
    println!("dirustat-cli: {} nodes in empty session", Session::new().tree().len());
}
