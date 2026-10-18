//! Pure path validation shared by every implementation (no I/O except [`require_exists`]).

use crate::error::{PlatformError, Result};
use std::path::Path;

/// Longest accepted path in UTF-16 code units, the Win32 extended-path limit.
pub const MAX_PATH_UNITS: usize = 32_767;

/// Checks that `path` is a non-empty, absolute, NUL-free path of sane length and returns it as text.
/// Non-UTF-8 paths are rejected: the command layer only ever hands over JSON strings.
pub fn validate_absolute(path: &Path) -> Result<&str> {
    let text = path.to_str().ok_or_else(|| PlatformError::InvalidPath("not valid UTF-8".to_owned()))?;
    if text.is_empty() {
        return Err(PlatformError::InvalidPath("empty".to_owned()));
    }
    if text.contains('\0') {
        return Err(PlatformError::InvalidPath("contains NUL".to_owned()));
    }
    if text.encode_utf16().count() > MAX_PATH_UNITS {
        return Err(PlatformError::InvalidPath("too long".to_owned()));
    }
    if !path.is_absolute() {
        return Err(PlatformError::InvalidPath("not absolute".to_owned()));
    }
    Ok(text)
}

/// `NotFound` unless something (even a dangling link) exists at `path`.
pub fn require_exists(path: &Path, text: &str) -> Result<()> {
    match path.symlink_metadata() {
        Ok(_) => Ok(()),
        Err(err) => Err(PlatformError::from_io("stat", text, &err)),
    }
}

/// Builds the single raw argument `/select,"<path>"` for `explorer.exe`. Explorer parses its own
/// command line and needs backslashes and the quotes around the path, so the argument is passed
/// without the usual argv quoting (and without any shell). A `"` can never occur in a Windows path,
/// so rejecting it also rules out argument injection.
pub fn explorer_select_arg(text: &str) -> Result<String> {
    if text.contains('"') {
        return Err(PlatformError::InvalidPath("contains a quote".to_owned()));
    }
    Ok(format!("/select,\"{}\"", text.replace('/', "\\")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_hostile_paths() {
        let long = "x".repeat(MAX_PATH_UNITS + 1);
        for bad in ["", "a\0b", "relative/dir", r"..\x", long.as_str()] {
            assert!(matches!(validate_absolute(Path::new(bad)), Err(PlatformError::InvalidPath(_))), "{bad:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn accepts_absolute_windows_paths() {
        for ok in [r"C:\Windows", r"\\server\share\x", r"\\?\C:\x"] {
            assert_eq!(validate_absolute(Path::new(ok)), Ok(ok));
        }
        assert!(validate_absolute(Path::new("C:rel")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_absolute_unix_paths() {
        assert_eq!(validate_absolute(Path::new("/tmp/x")), Ok("/tmp/x"));
    }

    #[test]
    fn explorer_arg_is_quoted_and_rejects_quotes() {
        assert_eq!(explorer_select_arg("C:/a b/c.txt").as_deref(), Ok(r#"/select,"C:\a b\c.txt""#));
        assert!(explorer_select_arg(r#"C:\a" /root,"D:\"#).is_err());
    }

    #[test]
    fn missing_path_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let text = missing.to_str().unwrap();
        assert!(matches!(require_exists(&missing, text), Err(PlatformError::NotFound(_))));
        assert!(require_exists(dir.path(), "dir").is_ok());
    }
}
