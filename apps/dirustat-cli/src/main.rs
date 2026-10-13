//! Headless CLI. Subcommands: `scan`, `info`, `run`, `commands`, `mcp`. No egui, no UI crates.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let code = dirustat_cli::run(&args, &mut out, &mut err);
    let _ = out.flush();
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
