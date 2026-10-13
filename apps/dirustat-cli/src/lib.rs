//! Testable core of `dirustat-cli`: `run(args, out, err) -> exit code`.
//!
//! Exit codes: 0 ok, 1 command or runtime error, 2 usage error.

mod args;
mod cmds;

use std::ffi::OsString;
use std::io::Write;

pub use args::{Cli, CmdCall, UsageError, parse};

/// Exit code for success.
pub const EXIT_OK: i32 = 0;
/// Exit code for a command or runtime error.
pub const EXIT_ERROR: i32 = 1;
/// Exit code for a usage error.
pub const EXIT_USAGE: i32 = 2;

/// Runs the CLI with `args` (without the program name). Never panics; I/O errors on `out`/`err`
/// are ignored (a closed pipe must not crash the tool).
pub fn run(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match parse(args) {
        Ok(Cli::Help) => {
            let _ = out.write_all(args::USAGE.as_bytes());
            EXIT_OK
        }
        Ok(cli) => cmds::execute(cli, out, err),
        Err(e) => {
            let _ = writeln!(err, "error: {e}\n\n{}", args::USAGE);
            EXIT_USAGE
        }
    }
}
