//! Pure path validation shared by every implementation (no I/O except [`require_exists`]).

use crate::error::{PlatformError, Result};
use std::path::Path;

/// Longest accepted path in UTF-16 code units, the Win32 extended-path limit.
pub const MAX_PATH_UNITS: usize = 32_767;

/// What kind of place a path string names. This is a positive classification: only [`Self::Local`]
/// is ever trusted, everything else is spelled out so each caller can decide what it tolerates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathClass {
    /// A path on a lettered local volume (`X:\...`, `X:/...`, `\\?\X:\...`) or, off Windows, an
    /// absolute path.
    Local,
    /// A network share: `\\server\share`, `//server/share`, `\\?\UNC\...`, `\??\UNC\...`.
    Network,
    /// Any other Win32 or NT namespace: `\\.\...`, `\\?\GLOBALROOT\...`, `\\?\Volume{...}\...`, `\??\...`.
    Device,
    /// Relative, drive-relative (`C:x`), rooted without a drive (`\x`), empty or otherwise unknown.
    Other,
}

/// [`classify_path`] for an explicit platform flavour (testable on every OS).
pub fn classify_path_for(text: &str, windows: bool) -> PathClass {
    if !windows {
        return if text.starts_with('/') { PathClass::Local } else { PathClass::Other };
    }
    let unified = text.replace('/', "\\");
    let drive = |s: &str| {
        let b = s.as_bytes();
        b.len() >= 3
            && b.first().is_some_and(u8::is_ascii_alphabetic)
            && b.get(1) == Some(&b':')
            && b.get(2) == Some(&b'\\')
    };
    for prefix in [r"\\?\", r"\??\", r"\\.\"] {
        let Some(rest) = unified.strip_prefix(prefix) else { continue };
        let is_unc = rest.get(..3).is_some_and(|s| s.eq_ignore_ascii_case("UNC"))
            && rest.get(3..).is_none_or(|t| t.is_empty() || t.starts_with('\\'));
        return if prefix == r"\\.\" {
            PathClass::Device
        } else if is_unc {
            PathClass::Network
        } else if drive(rest) {
            PathClass::Local
        } else {
            PathClass::Device
        };
    }
    if unified.starts_with(r"\\") {
        PathClass::Network
    } else if drive(&unified) {
        PathClass::Local
    } else {
        PathClass::Other
    }
}

/// Classifies `text` for the current OS. On Windows a lettered path is also checked against the
/// real drive type (QA-145): a mapped network drive (`Z:\` -> `\\server\share`) is
/// [`PathClass::Network`], a letter that is not a mounted volume is [`PathClass::Other`]. Only a
/// path whose drive is a fixed, removable, optical or RAM disk is [`PathClass::Local`].
pub fn classify_path(text: &str) -> PathClass {
    classify_path_with(text, cfg!(windows), &live_drive_type)
}

/// [`classify_path_for`] plus the drive-type check of [`classify_path`], with the
/// `GetDriveTypeW` lookup supplied by the caller (`root` is `X:\`; `None` = the query failed).
/// Lexical `Network`, `Device` and `Other` answers are final and never reach `drive_type`.
pub fn classify_path_with(text: &str, windows: bool, drive_type: &dyn Fn(&str) -> Option<u32>) -> PathClass {
    let lexical = classify_path_for(text, windows);
    if !windows || lexical != PathClass::Local {
        return lexical;
    }
    let Some(root) = drive_root(text) else { return PathClass::Other };
    match drive_type(&root) {
        Some(kind) if drive_type_is_local(kind) => PathClass::Local,
        Some(DRIVE_TYPE_REMOTE) => PathClass::Network,
        _ => PathClass::Other,
    }
}

#[cfg(windows)]
fn live_drive_type(root: &str) -> Option<u32> {
    crate::win32::drive_type(root)
}

#[cfg(not(windows))]
fn live_drive_type(_root: &str) -> Option<u32> {
    None
}

/// [`validate_absolute`] that also demands a [`PathClass::Local`] path: slow or remote namespaces
/// (network shares, device paths) are refused before any I/O happens.
pub fn validate_local(path: &Path) -> Result<&str> {
    let text = validate_absolute(path)?;
    if classify_path(text) == PathClass::Local {
        Ok(text)
    } else {
        Err(PlatformError::InvalidPath("not a local drive path".to_owned()))
    }
}

/// Checks that `path` is a non-empty, absolute, NUL-free path of sane length and returns it as text.
/// Non-UTF-8 paths are rejected: the command layer only ever hands over JSON strings. Device
/// namespaces (`\\.\`, `\\?\GLOBALROOT\`, `\\?\Volume{`) are never accepted.
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
    if classify_path(text) == PathClass::Device {
        return Err(PlatformError::InvalidPath("device namespace paths are not supported".to_owned()));
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
/// `GetDriveTypeW` code of a network (mapped) drive.
pub const DRIVE_TYPE_REMOTE: u32 = 4;

/// Whether a `GetDriveTypeW` code names a volume attached to this machine: removable (2), fixed
/// (3), optical (5) or RAM disk (6). Unknown (0), no root directory (1), remote (4) and any code
/// this build does not know are not local.
pub fn drive_type_is_local(kind: u32) -> bool {
    matches!(kind, 2 | 3 | 5 | 6)
}

/// The volume root `X:\` of a path on a lettered volume (`C:\x`, `c:/x`, `\\?\C:\x`, `\??\C:\x`);
/// `None` for network (`\\server\share`), device, relative or otherwise unrecognized paths.
pub fn drive_root(text: &str) -> Option<String> {
    let unified = text.replace('/', "\\");
    let local = unified.strip_prefix(r"\\?\").or_else(|| unified.strip_prefix(r"\??\")).unwrap_or(&unified);
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

    #[test]
    fn classification_is_positive() {
        use PathClass::{Device, Local, Network, Other};
        let win = [
            (r"C:\x", Local),
            ("c:/x/y", Local),
            (r"\\?\C:\x", Local),
            (r"\\?\c:/x", Local),
            (r"\\server\share", Network),
            ("//server/share", Network),
            (r"\\?\UNC\s\x", Network),
            (r"\??\unc\s\x", Network),
            ("/??/UNC/s/x", Network),
            (r"\\?\GLOBALROOT\Device\Mup\1.2.3.4\s\x", Device),
            (r"\\?\globalroot\device\mup\x", Device),
            (r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\x", Device),
            (r"\\?\UNCLE\x", Device),
            (r"\\.\C:", Device),
            (r"\\.\pipe\x", Device),
            (r"\??\C:\x", Local),
            (r"\??\GLOBALROOT\x", Device),
            (r"\\?\C:", Device),
            ("C:rel", Other),
            (r"\rooted", Other),
            (" C:\\x", Other),
            ("rel", Other),
            ("", Other),
        ];
        for (text, want) in win {
            assert_eq!(classify_path_for(text, true), want, "{text:?}");
        }
        assert_eq!(classify_path_for("/home/x", false), Local);
        assert_eq!(classify_path_for(r"\\?\GLOBALROOT\x", false), Other);
        assert_eq!(classify_path_for("rel", false), Other);
    }

    /// QA-145: a lettered path is local only when its drive is a local disk; the lookup is a fake.
    #[test]
    fn drive_type_decides_whether_a_lettered_path_is_local() {
        use PathClass::{Device, Local, Network, Other};
        let provider = |root: &str| match root {
            r"C:\" => Some(3),
            r"D:\" => Some(2),
            r"E:\" => Some(5),
            r"F:\" => Some(6),
            r"Z:\" => Some(4),
            r"Y:\" => Some(1),
            r"X:\" => Some(0),
            r"W:\" => Some(77),
            _ => None,
        };
        let cases = [
            (r"C:\x", Local),
            ("c:/x/y", Local),
            (r"\\?\C:\x", Local),
            (r"\??\C:\x", Local),
            (r"D:\x", Local),
            (r"E:\x", Local),
            (r"F:\x", Local),
            (r"Z:\data", Network),
            (r"\\?\Z:\data", Network),
            ("z:/data", Network),
            (r"Y:\x", Other),
            (r"X:\x", Other),
            (r"W:\x", Other),
            (r"V:\x", Other),
            // The lexical answers are final.
            (r"\\server\share", Network),
            (r"\\?\UNC\s\x", Network),
            (r"\\.\C:", Device),
            (r"\\?\GLOBALROOT\x", Device),
            ("C:rel", Other),
            ("rel", Other),
        ];
        for (text, want) in cases {
            assert_eq!(classify_path_with(text, true, &provider), want, "{text:?}");
        }
        // Off Windows no drive type is consulted at all.
        let never = |_: &str| -> Option<u32> { panic!("the drive type must not be queried off Windows") };
        assert_eq!(classify_path_with("/home/x", false, &never), Local);
        assert_eq!(classify_path_with("rel", false, &never), Other);
        // The pure helper: removable, fixed, optical and RAM disks are local; nothing else is.
        for kind in [2, 3, 5, 6] {
            assert!(drive_type_is_local(kind), "{kind}");
        }
        for kind in [0, 1, 4, 7, 99, u32::MAX] {
            assert!(!drive_type_is_local(kind), "{kind}");
        }
    }

    #[test]
    fn drive_root_understands_the_nt_prefix() {
        assert_eq!(drive_root(r"\??\c:\x").as_deref(), Some(r"C:\"));
        assert_eq!(drive_root(r"\??\UNC\s\x"), None);
    }

    #[cfg(windows)]
    #[test]
    fn live_classification_trusts_real_local_drives_only() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().to_str().unwrap();
        assert_eq!(classify_path(text), PathClass::Local, "{text}");
        assert!(validate_local(dir.path()).is_ok());
        // A letter with no volume behind it is not trusted as local.
        let free = ('D'..='Z').find(|c| !Path::new(&format!("{c}:\\")).exists()).unwrap();
        let path = format!("{free}:\\data");
        assert_eq!(classify_path(&path), PathClass::Other, "{path}");
        assert!(matches!(validate_local(Path::new(&path)), Err(PlatformError::InvalidPath(_))));
    }

    #[cfg(windows)]
    #[test]
    fn device_paths_never_validate() {
        for bad in [r"\\?\GLOBALROOT\Device\Mup\x", r"\\.\C:", r"\\?\Volume{1}\x"] {
            assert!(matches!(validate_absolute(Path::new(bad)), Err(PlatformError::InvalidPath(_))), "{bad}");
        }
        // Network paths validate (the engine decides) but are not local.
        assert!(validate_absolute(Path::new(r"\\server\share\x")).is_ok());
        assert!(matches!(validate_local(Path::new(r"\\server\share\x")), Err(PlatformError::InvalidPath(_))));
        assert!(validate_local(Path::new(r"C:\Windows")).is_ok());
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
