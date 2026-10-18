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

/// Win32 `GetDriveTypeW` code of a fixed (internal) disk, the only kind with a recycle bin that
/// this layer trusts.
pub const DRIVE_TYPE_FIXED: u32 = 3;

/// The volume root `X:\` of a path on a lettered volume (`C:\x`, `c:/x`, `\\?\C:\x`); `None` for
/// network (`\\server\share`), device, relative or otherwise unrecognized paths.
pub fn drive_root(text: &str) -> Option<String> {
    let unified = text.replace('/', "\\");
    let local = unified.strip_prefix(r"\\?\").unwrap_or(&unified);
    let mut chars = local.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') || chars.next() != Some('\\') {
        return None;
    }
    Some(format!("{}:\\", letter.to_ascii_uppercase()))
}

/// Whether a drive of type `kind` (`GetDriveTypeW`) has a recycle bin we can rely on: fixed disks
/// only. Network, removable, optical, RAM and unknown drives delete permanently (or fail) instead.
pub fn drive_type_has_recycle_bin(kind: u32) -> bool {
    kind == DRIVE_TYPE_FIXED
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
    fn drive_roots_and_recycle_bin_support() {
        for (input, want) in [
            (r"C:\x\y", Some(r"C:\")),
            ("d:/x", Some(r"D:\")),
            (r"\\?\E:\x", Some(r"E:\")),
            (r"\\server\share\x", None),
            (r"\\?\UNC\server\share", None),
            (r"\\.\C:", None),
            ("C:rel", None),
            ("rel", None),
            ("", None),
        ] {
            assert_eq!(drive_root(input).as_deref(), want, "{input}");
        }
        assert!(drive_type_has_recycle_bin(DRIVE_TYPE_FIXED));
        // Unknown(0), no root(1), removable(2), remote(4), cdrom(5), ramdisk(6).
        for kind in [0, 1, 2, 4, 5, 6, u32::MAX] {
            assert!(!drive_type_has_recycle_bin(kind), "{kind}");
        }
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
