//! Hand-written argument parser. Pure: no I/O, no panics, every hostile input is a [`UsageError`].

use dirustat_model::SizeMode;
use serde_json::Value;
use std::ffi::OsString;
use std::fmt;

pub const USAGE: &str = "\
dirustat-cli: headless diRUSTat

USAGE:
    dirustat-cli commands [--json]
    dirustat-cli scan <folder> [--json <out.json>] [--size-mode logical|allocated] [--top <N>] [--timeout <secs>]
    dirustat-cli info <folder>
    dirustat-cli run [--scan <folder>] --cmd 'id={json params}'... [--confirm] [--timeout <secs>]
    dirustat-cli mcp [--connect <127.0.0.1:port>]
    dirustat-cli --help

EXIT CODES:
    0 ok, 1 command error or scan timeout (stable error code on stderr), 2 usage error

--timeout bounds the total time spent waiting for scans; on expiry the scan is cancelled and the
exit code is 1.
";

/// Upper bound for `--top`; larger values are clamped.
pub const MAX_TOP: usize = 100_000;
/// Upper bound for `--timeout`, in seconds (one week).
pub const MAX_TIMEOUT_SECS: u64 = 7 * 24 * 3600;
/// Default for `--top`.
pub const DEFAULT_TOP: usize = 20;

/// A parsed command line.
#[derive(Clone, Debug, PartialEq)]
pub enum Cli {
    Help,
    Commands { json: bool },
    Scan { folder: String, json_out: Option<String>, size_mode: SizeMode, top: usize, timeout: Option<u64> },
    Info { folder: String },
    Run { scan: Option<String>, cmds: Vec<CmdCall>, confirm: bool, timeout: Option<u64> },
    Mcp { connect: Option<String> },
}

/// One `--cmd id={json}` argument.
#[derive(Clone, Debug, PartialEq)]
pub struct CmdCall {
    pub id: String,
    pub params: Value,
}

/// Why the command line could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

fn usage<T>(msg: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(msg.into()))
}

/// Parses `args` (without the program name).
pub fn parse(args: &[OsString]) -> Result<Cli, UsageError> {
    let mut words = Vec::with_capacity(args.len());
    for a in args {
        match a.to_str() {
            Some(s) => words.push(s),
            None => return usage("arguments must be valid UTF-8"),
        }
    }
    let mut it = words.into_iter();
    let Some(first) = it.next() else { return usage("missing subcommand") };
    let rest: Vec<&str> = it.collect();
    match first {
        "-h" | "--help" | "help" => Ok(Cli::Help),
        "commands" => parse_commands(&rest),
        "scan" => parse_scan(&rest),
        "info" => parse_info(&rest),
        "run" => parse_run(&rest),
        "mcp" => parse_mcp(&rest),
        other => usage(format!("unknown subcommand: {other}")),
    }
}

fn parse_mcp(rest: &[&str]) -> Result<Cli, UsageError> {
    match rest {
        [] => Ok(Cli::Mcp { connect: None }),
        ["--connect", addr] if !addr.is_empty() => Ok(Cli::Mcp { connect: Some((*addr).to_owned()) }),
        [one] if one.strip_prefix("--connect=").is_some_and(|a| !a.is_empty()) => {
            Ok(Cli::Mcp { connect: one.strip_prefix("--connect=").map(str::to_owned) })
        }
        [extra, ..] => usage(format!("unexpected argument for mcp: {extra}")),
    }
}

type Words<'a> = std::slice::Iter<'a, &'a str>;

/// Splits `--opt=value` into `("--opt", Some("value"))`.
fn split_opt(arg: &str) -> (&str, Option<&str>) {
    if arg.starts_with("--")
        && let Some((name, value)) = arg.split_once('=')
    {
        return (name, Some(value));
    }
    (arg, None)
}

/// Value of an option: inline (`--opt=v`) or the next word.
fn value<'a>(name: &str, inline: Option<&'a str>, rest: &mut Words<'a>) -> Result<&'a str, UsageError> {
    if let Some(v) = inline {
        return Ok(v);
    }
    match rest.next() {
        Some(v) => Ok(v),
        None => usage(format!("option {name} needs a value")),
    }
}

fn non_empty<'a>(name: &str, v: &'a str) -> Result<&'a str, UsageError> {
    if v.is_empty() { usage(format!("{name} value must not be empty")) } else { Ok(v) }
}

/// Parses a `--timeout` value: whole seconds, 1..=[`MAX_TIMEOUT_SECS`].
fn parse_timeout(v: &str) -> Result<u64, UsageError> {
    match v.parse::<u64>() {
        Ok(n) if (1..=MAX_TIMEOUT_SECS).contains(&n) => Ok(n),
        _ => usage(format!("--timeout needs whole seconds in 1..={MAX_TIMEOUT_SECS}, got {v:?}")),
    }
}

fn parse_commands(args: &[&str]) -> Result<Cli, UsageError> {
    let mut json = false;
    for a in args {
        match *a {
            "--json" => json = true,
            other => return usage(format!("unexpected argument for commands: {other}")),
        }
    }
    Ok(Cli::Commands { json })
}

fn parse_folder(folder: &mut Option<String>, arg: &str) -> Result<(), UsageError> {
    if arg.starts_with('-') {
        return usage(format!("unknown option: {arg}"));
    }
    if folder.is_some() {
        return usage(format!("unexpected extra argument: {arg}"));
    }
    *folder = Some(non_empty("folder", arg)?.to_owned());
    Ok(())
}

fn parse_scan(args: &[&str]) -> Result<Cli, UsageError> {
    let mut folder = None;
    let mut json_out = None;
    let mut size_mode = SizeMode::default();
    let mut top = DEFAULT_TOP;
    let mut timeout = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = split_opt(arg);
        match name {
            "--json" => json_out = Some(non_empty(name, value(name, inline, &mut it)?)?.to_owned()),
            "--size-mode" => {
                size_mode = match value(name, inline, &mut it)? {
                    "logical" => SizeMode::Logical,
                    "allocated" => SizeMode::Allocated,
                    other => return usage(format!("--size-mode must be logical or allocated, got {other:?}")),
                };
            }
            "--top" => {
                let v = value(name, inline, &mut it)?;
                top = match v.parse::<usize>() {
                    Ok(n) => n.min(MAX_TOP),
                    Err(_) => return usage(format!("--top needs a non-negative integer, got {v:?}")),
                };
            }
            "--timeout" => timeout = Some(parse_timeout(value(name, inline, &mut it)?)?),
            _ => parse_folder(&mut folder, arg)?,
        }
    }
    match folder {
        Some(folder) => Ok(Cli::Scan { folder, json_out, size_mode, top, timeout }),
        None => usage("scan needs a folder"),
    }
}

fn parse_info(args: &[&str]) -> Result<Cli, UsageError> {
    let mut folder = None;
    for arg in args {
        parse_folder(&mut folder, arg)?;
    }
    match folder {
        Some(folder) => Ok(Cli::Info { folder }),
        None => usage("info needs a folder"),
    }
}

fn parse_cmd(text: &str) -> Result<CmdCall, UsageError> {
    let (id, params) = match text.split_once('=') {
        Some((id, json)) => {
            let params = serde_json::from_str(json)
                .map_err(|e| UsageError(format!("--cmd {id}: params are not valid JSON: {e}")))?;
            (id, params)
        }
        None => (text, Value::Null),
    };
    if id.is_empty() {
        return usage("--cmd needs a command id");
    }
    Ok(CmdCall { id: id.to_owned(), params })
}

fn parse_run(args: &[&str]) -> Result<Cli, UsageError> {
    let mut scan = None;
    let mut cmds = Vec::new();
    let mut confirm = false;
    let mut timeout = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = split_opt(arg);
        match name {
            "--scan" => scan = Some(non_empty(name, value(name, inline, &mut it)?)?.to_owned()),
            "--cmd" => cmds.push(parse_cmd(value(name, inline, &mut it)?)?),
            "--confirm" if inline.is_none() => confirm = true,
            "--timeout" => timeout = Some(parse_timeout(value(name, inline, &mut it)?)?),
            _ => return usage(format!("unexpected argument for run: {arg}")),
        }
    }
    if cmds.is_empty() && scan.is_none() {
        return usage("run needs at least one --cmd (or --scan)");
    }
    Ok(Cli::Run { scan, cmds, confirm, timeout })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(words: &[&str]) -> Result<Cli, UsageError> {
        let args: Vec<OsString> = words.iter().map(OsString::from).collect();
        parse(&args)
    }

    #[test]
    fn help_and_missing() {
        assert_eq!(p(&["--help"]), Ok(Cli::Help));
        assert_eq!(p(&["-h"]), Ok(Cli::Help));
        assert!(p(&[]).is_err());
        assert!(p(&["bogus"]).is_err());
        assert!(p(&[""]).is_err());
    }

    #[test]
    fn commands_flags() {
        assert_eq!(p(&["commands"]), Ok(Cli::Commands { json: false }));
        assert_eq!(p(&["commands", "--json"]), Ok(Cli::Commands { json: true }));
        assert!(p(&["commands", "--x"]).is_err());
    }

    #[test]
    fn scan_full() {
        let cli = p(&["scan", "/tmp/x", "--json", "o.json", "--size-mode=logical", "--top", "5"]).unwrap();
        assert_eq!(
            cli,
            Cli::Scan {
                folder: "/tmp/x".into(),
                json_out: Some("o.json".into()),
                size_mode: SizeMode::Logical,
                top: 5,
                timeout: None
            }
        );
    }

    #[test]
    fn scan_defaults_and_clamp() {
        assert!(matches!(
            p(&["scan", "d"]),
            Ok(Cli::Scan { top: DEFAULT_TOP, size_mode: SizeMode::Allocated, json_out: None, .. })
        ));
        assert!(matches!(p(&["scan", "d", "--top", "18446744073709551615"]), Ok(Cli::Scan { top: MAX_TOP, .. })));
    }

    #[test]
    fn scan_bad_args() {
        assert!(p(&["scan"]).is_err());
        assert!(p(&["scan", ""]).is_err());
        assert!(p(&["scan", "a", "b"]).is_err());
        assert!(p(&["scan", "a", "--top"]).is_err());
        assert!(p(&["scan", "a", "--top", "-1"]).is_err());
        assert!(p(&["scan", "a", "--top", "99999999999999999999999"]).is_err());
        assert!(p(&["scan", "a", "--size-mode", "x"]).is_err());
        assert!(p(&["scan", "a", "--json"]).is_err());
        assert!(p(&["scan", "a", "--json="]).is_err());
        assert!(p(&["scan", "a", "--nope"]).is_err());
    }

    #[test]
    fn timeout_option_is_parsed_and_bounded() {
        assert!(matches!(p(&["scan", "d", "--timeout", "30"]), Ok(Cli::Scan { timeout: Some(30), .. })));
        assert!(matches!(p(&["scan", "d", "--timeout=1"]), Ok(Cli::Scan { timeout: Some(1), .. })));
        assert!(matches!(p(&["run", "--scan", "d", "--timeout", "5"]), Ok(Cli::Run { timeout: Some(5), .. })));
        assert!(matches!(p(&["scan", "d"]), Ok(Cli::Scan { timeout: None, .. })));
        for bad in ["0", "-1", "x", "", "1.5", "604801", "99999999999999999999"] {
            assert!(p(&["scan", "d", "--timeout", bad]).is_err(), "{bad:?}");
            assert!(p(&["run", "--scan", "d", "--timeout", bad]).is_err(), "{bad:?}");
        }
        assert!(p(&["scan", "d", "--timeout"]).is_err());
        assert!(p(&["info", "d", "--timeout", "5"]).is_err());
    }

    #[test]
    fn info_needs_one_folder() {
        assert_eq!(p(&["info", "d"]), Ok(Cli::Info { folder: "d".into() }));
        assert!(p(&["info"]).is_err());
        assert!(p(&["info", "a", "b"]).is_err());
    }

    #[test]
    fn run_cmds() {
        let cli =
            p(&["run", "--scan", "d", "--cmd", "select.node={\"id\":1}", "--cmd=session.state", "--confirm"]).unwrap();
        assert_eq!(
            cli,
            Cli::Run {
                scan: Some("d".into()),
                cmds: vec![
                    CmdCall { id: "select.node".into(), params: json!({"id": 1}) },
                    CmdCall { id: "session.state".into(), params: Value::Null },
                ],
                confirm: true,
                timeout: None
            }
        );
    }

    #[test]
    fn run_bad_args() {
        assert!(p(&["run"]).is_err());
        assert!(p(&["run", "--cmd"]).is_err());
        assert!(p(&["run", "--cmd", "=1"]).is_err());
        assert!(p(&["run", "--cmd", ""]).is_err());
        assert!(p(&["run", "--cmd", "a.b={not json"]).is_err());
        assert!(p(&["run", "--scan", ""]).is_err());
        assert!(p(&["run", "--cmd", "a.b", "extra"]).is_err());
    }

    #[test]
    fn mcp_takes_no_args() {
        assert_eq!(p(&["mcp"]), Ok(Cli::Mcp { connect: None }));
        assert_eq!(p(&["mcp", "--connect=127.0.0.1:1"]), Ok(Cli::Mcp { connect: Some("127.0.0.1:1".into()) }));
        assert!(p(&["mcp", "x"]).is_err());
    }

    #[test]
    fn invalid_utf8_is_a_usage_error() {
        #[cfg(windows)]
        let bad = {
            use std::os::windows::ffi::OsStringExt;
            OsString::from_wide(&[0x66, 0xd800, 0x6f])
        };
        #[cfg(unix)]
        let bad = {
            use std::os::unix::ffi::OsStringExt;
            OsString::from_vec(vec![0x66, 0xff, 0x6f])
        };
        assert!(parse(&[OsString::from("scan"), bad]).is_err());
    }
}
