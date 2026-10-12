//! `cargo xtask <command>` — single entry point for checks and automation.
//!
//! Commands: `layers` (dependency layer check), `ci` (the whole gate), `fixtures` (synthetic trees), `help`.
//! Planned: `parity`, `scorecard`, `perf`, `version` (see docs/development.md).
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod fixtures;
mod layers;

use std::path::{Path, PathBuf};
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

/// `cargo xtask fixtures [--size small|100k|1m|5m|<nodes>] [--seed N] [--out DIR]`.
fn fixtures_command(root: &Path, args: &[String]) -> bool {
    let mut size = "100k".to_owned();
    let mut seed = 1u64;
    let mut out: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match (flag.as_str(), iter.next()) {
            ("--size", Some(v)) => size = v.clone(),
            ("--seed", Some(v)) => match v.parse() {
                Ok(n) => seed = n,
                Err(_) => {
                    eprintln!("invalid --seed: {v}");
                    return false;
                }
            },
            ("--out", Some(v)) => out = Some(PathBuf::from(v)),
            _ => {
                eprintln!("usage: cargo xtask fixtures [--size small|100k|1m|5m|<nodes>] [--seed N] [--out DIR]");
                return false;
            }
        }
    }
    let Some(nodes) = fixtures::preset(&size).or_else(|| size.parse().ok()) else {
        eprintln!("invalid --size: {size}");
        return false;
    };
    let out = out.unwrap_or_else(|| root.join("fixtures-local").join(format!("{size}-seed{seed}")));
    println!("==> fixtures: {nodes} nodes, seed {seed} -> {}", out.display());
    match fixtures::create(&out, nodes, seed) {
        Ok(summary) => {
            let skipped = if summary.skipped.is_empty() { "none".to_owned() } else { summary.skipped.join("; ") };
            println!(
                "done: {} dirs, {} files, {} bytes logical; skipped: {skipped}",
                summary.dirs, summary.files, summary.bytes
            );
            true
        }
        Err(err) => {
            eprintln!("FAILED: fixtures: {err}");
            false
        }
    }
}

fn main() -> ExitCode {
    let root = workspace_root();
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let ok = match command.as_str() {
        "layers" => layers::check(&root),
        "ci" => ci(&root),
        "fixtures" => fixtures_command(&root, &std::env::args().skip(2).collect::<Vec<_>>()),
        _ => {
            println!("usage: cargo xtask <layers|ci|fixtures>");
            return if command == "help" { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
