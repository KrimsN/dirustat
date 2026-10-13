//! Launch arguments of the desktop app as a pure parser (no I/O, no process exit).

use dirustat_model::SizeMode;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;

/// Usage text for `--help` and usage errors.
pub const USAGE: &str = "\
Usage: dirustat [OPTIONS]

Options:
  --scan <path>            scan this folder at startup
  --size-mode <mode>       logical | allocated
  --control <port>         start the control channel on this TCP port (0 = any free port)
  --screenshot <path>      save a PNG screenshot of the main window
  --exit-after-screenshot  quit after the screenshot (requires --screenshot)
  --demo                   load synthetic demo data
  --size <W>x<H>           initial window size in logical pixels (200..=16384 each)
  --help                   print this help
";

/// Parsed launch arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    pub scan: Option<PathBuf>,
    pub size_mode: Option<SizeMode>,
    pub control: Option<u16>,
    pub screenshot: Option<PathBuf>,
    pub demo: bool,
    /// Initial window size `(width, height)`.
    pub size: Option<(u16, u16)>,
    pub exit_after_screenshot: bool,
    pub help: bool,
}

/// A command-line usage error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    UnknownArgument(String),
    MissingValue(&'static str),
    InvalidValue {
        option: &'static str,
        value: String,
    },
    Duplicate(&'static str),
    /// `--exit-after-screenshot` without `--screenshot`.
    ExitWithoutScreenshot,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArgument(a) => write!(f, "unknown argument: {a}"),
            Self::MissingValue(o) => write!(f, "{o} needs a value"),
            Self::InvalidValue { option, value } => write!(f, "invalid value for {option}: {value}"),
            Self::Duplicate(o) => write!(f, "{o} given more than once"),
            Self::ExitWithoutScreenshot => write!(f, "--exit-after-screenshot requires --screenshot"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Longest argument text echoed back in an error message.
const ECHO_LIMIT: usize = 200;

fn echo(s: &OsStr) -> String {
    s.to_string_lossy().chars().take(ECHO_LIMIT).collect()
}

fn set_once<T>(slot: &mut Option<T>, option: &'static str, value: T) -> Result<(), LaunchError> {
    if slot.is_some() {
        return Err(LaunchError::Duplicate(option));
    }
    *slot = Some(value);
    Ok(())
}

fn non_empty(option: &'static str, v: OsString) -> Result<PathBuf, LaunchError> {
    if v.is_empty() {
        return Err(LaunchError::InvalidValue { option, value: String::new() });
    }
    Ok(PathBuf::from(v))
}

/// Smallest and largest accepted window edge, in logical pixels.
const SIZE_RANGE: std::ops::RangeInclusive<u16> = 200..=16384;

/// Parses `WxH` (decimal digits only) into an in-range size.
fn parse_size(text: &str) -> Option<(u16, u16)> {
    let (w, h) = text.split_once('x')?;
    let dim = |t: &str| {
        let ok = !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
        t.parse::<u16>().ok().filter(|n| ok && SIZE_RANGE.contains(n))
    };
    Some((dim(w)?, dim(h)?))
}

impl LaunchArgs {
    /// Parses the arguments after the program name. `--option=value` is accepted as well as
    /// `--option value`. `--help` stops parsing and wins over later errors.
    pub fn parse<I>(args: I) -> Result<Self, LaunchError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut out = Self::default();
        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            let Some(text) = arg.to_str() else {
                return Err(LaunchError::UnknownArgument(echo(&arg)));
            };
            let (name, inline) = match text.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n, Some(OsString::from(v))),
                _ => (text, None),
            };
            let mut value = |option: &'static str| -> Result<OsString, LaunchError> {
                inline.clone().or_else(|| iter.next()).ok_or(LaunchError::MissingValue(option))
            };
            match name {
                "--help" | "-h" => {
                    out.help = true;
                    return Ok(out);
                }
                "--demo" if inline.is_none() => out.demo = true,
                "--exit-after-screenshot" if inline.is_none() => out.exit_after_screenshot = true,
                "--scan" => {
                    let v = non_empty("--scan", value("--scan")?)?;
                    set_once(&mut out.scan, "--scan", v)?;
                }
                "--screenshot" => {
                    let v = non_empty("--screenshot", value("--screenshot")?)?;
                    set_once(&mut out.screenshot, "--screenshot", v)?;
                }
                "--size-mode" => {
                    let v = value("--size-mode")?;
                    let mode = match v.to_str() {
                        Some("logical") => SizeMode::Logical,
                        Some("allocated") => SizeMode::Allocated,
                        _ => return Err(LaunchError::InvalidValue { option: "--size-mode", value: echo(&v) }),
                    };
                    set_once(&mut out.size_mode, "--size-mode", mode)?;
                }
                "--size" => {
                    let v = value("--size")?;
                    let size = v.to_str().and_then(parse_size);
                    let size = size.ok_or_else(|| LaunchError::InvalidValue { option: "--size", value: echo(&v) })?;
                    set_once(&mut out.size, "--size", size)?;
                }
                "--control" => {
                    let v = value("--control")?;
                    let port = v.to_str().and_then(|s| s.parse::<u16>().ok());
                    let port =
                        port.ok_or_else(|| LaunchError::InvalidValue { option: "--control", value: echo(&v) })?;
                    set_once(&mut out.control, "--control", port)?;
                }
                _ => return Err(LaunchError::UnknownArgument(echo(&arg))),
            }
        }
        if out.exit_after_screenshot && out.screenshot.is_none() {
            return Err(LaunchError::ExitWithoutScreenshot);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<LaunchArgs, LaunchError> {
        LaunchArgs::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn empty_is_default() {
        assert_eq!(parse(&[]), Ok(LaunchArgs::default()));
    }

    #[test]
    fn all_options() {
        let a = parse(&[
            "--scan",
            "C:\\x",
            "--size-mode",
            "logical",
            "--control",
            "7000",
            "--screenshot=a.png",
            "--demo",
            "--exit-after-screenshot",
        ])
        .unwrap();
        assert_eq!(a.scan, Some(PathBuf::from("C:\\x")));
        assert_eq!(a.size_mode, Some(SizeMode::Logical));
        assert_eq!(a.control, Some(7000));
        assert_eq!(a.screenshot, Some(PathBuf::from("a.png")));
        assert!(a.demo && a.exit_after_screenshot && !a.help);
    }

    #[test]
    fn size_parses_and_rejects_hostile_values() {
        assert_eq!(parse(&["--size", "800x600"]).unwrap().size, Some((800, 600)));
        assert_eq!(parse(&["--size=1200x800"]).unwrap().size, Some((1200, 800)));
        for bad in ["10x10", "800", "axb", "800x", "x600", "99999x600", "-5x600", "+800x600", "800x600x1", "0x0", ""] {
            assert!(matches!(parse(&["--size", bad]), Err(LaunchError::InvalidValue { .. })), "{bad:?}");
        }
        assert_eq!(parse(&["--size"]), Err(LaunchError::MissingValue("--size")));
        assert_eq!(parse(&["--size", "300x300", "--size", "400x400"]), Err(LaunchError::Duplicate("--size")));
    }

    #[test]
    fn help_wins() {
        assert!(parse(&["--help", "--bogus"]).unwrap().help);
        assert!(parse(&["-h"]).unwrap().help);
        assert!(USAGE.contains("--scan"));
    }

    #[test]
    fn usage_errors() {
        assert_eq!(parse(&["--scan"]), Err(LaunchError::MissingValue("--scan")));
        assert_eq!(parse(&["--scan", "a", "--scan", "b"]), Err(LaunchError::Duplicate("--scan")));
        assert_eq!(parse(&["--exit-after-screenshot"]), Err(LaunchError::ExitWithoutScreenshot));
        for bad in
            [["--control", "70000"], ["--control", "-1"], ["--control", ""], ["--size-mode", "huge"], ["--scan", ""]]
        {
            assert!(matches!(parse(&bad), Err(LaunchError::InvalidValue { .. })), "{bad:?}");
        }
        assert!(matches!(parse(&["nope"]), Err(LaunchError::UnknownArgument(_))));
        assert!(matches!(parse(&["--demo=1"]), Err(LaunchError::UnknownArgument(_))));
    }

    #[test]
    fn hostile_input_never_panics() {
        let long = "x".repeat(100_000);
        let junk = ["", "-", "--", "=", "--=", "--scan=", "\u{0}", "\u{202e}", long.as_str(), "--control=\u{0}"];
        for a in junk {
            for b in junk {
                let _ = parse(&[a, b]);
                let _ = parse(&["--scan", a, b]);
            }
        }
        let err = parse(&[long.as_str()]).unwrap_err();
        assert!(err.to_string().len() < 1000);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let bad = OsString::from_wide(&[0xD800, 0x41]);
            assert!(LaunchArgs::parse([bad]).is_err());
        }
    }
}
