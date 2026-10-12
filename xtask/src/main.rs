//! `cargo xtask <command>` — single entry point for checks and automation.
//!
//! Commands: `layers` (dependency layer check), `ci` (the whole gate), `help`.
//! Planned: `parity`, `scorecard`, `perf`, `fixtures`, `version` (see docs/development.md).
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod layers;

use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn workspace_root() -> PathBuf {
    // xtask lives in <root>/xtask; cargo sets CARGO_MANIFEST_DIR for `cargo xtask`.
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_default();
    manifest.parent().map(PathBuf::from).unwrap_or_default()
}

fn run_step(root: &PathBuf, name: &str, program: &str, args: &[&str]) -> bool {
    println!("==> {name}: {program} {}", args.join(" "));
    match Command::new(program).args(args).current_dir(root).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("FAILED: {name} ({status})");
            false
        }
        Err(err) => {
            eprintln!("FAILED: {name}: cannot start {program}: {err}");
            false
        }
    }
}

fn ci(root: &PathBuf) -> bool {
    let steps: [(&str, &str, &[&str]); 4] = [
        ("fmt", "cargo", &["fmt", "--all", "--", "--check"]),
        ("clippy", "cargo", &["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]),
        ("test", "cargo", &["test", "--workspace", "--locked"]),
        (
            "wasm-core",
            "cargo",
            &["check", "-p", "dirustat-model", "-p", "dirustat-treemap", "--target", "wasm32-unknown-unknown"],
        ),
    ];
    let mut ok = layers::check(root);
    for (name, program, args) in steps {
        // The wasm target may be missing locally: skip with a warning; CI always has it.
        if name == "wasm-core" && !wasm_target_installed() {
            eprintln!("SKIP: wasm-core (rustup target add wasm32-unknown-unknown)");
            continue;
        }
        ok &= run_step(root, name, program, args);
    }
    ok
}

fn wasm_target_installed() -> bool {
    Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wasm32-unknown-unknown"))
        .unwrap_or(false)
}

fn main() -> ExitCode {
    let root = workspace_root();
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let ok = match command.as_str() {
        "layers" => layers::check(&root),
        "ci" => ci(&root),
        _ => {
            println!("usage: cargo xtask <layers|ci>");
            return if command == "help" { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
