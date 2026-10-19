//! Pure path helpers shared by the control channel and the UI (no I/O).

pub use dirustat_platform::{PathClass, classify_path};

/// True for a path on a lettered local volume (`X:\...`, `\\?\X:\...`; off Windows, any absolute
/// path). This is the positive test behind every guarded path parameter: network shares, device
/// namespaces (`\\.\`, `\\?\GLOBALROOT\`, `\\?\Volume{`) and relative paths are not local.
pub fn is_local_path(path: &str) -> bool {
    classify_path(path) == PathClass::Local
}

/// True for network paths in any spelling: `\\server\share`, `//server/share`, mixed separators,
/// `\\?\UNC\...`, the NT form `\??\UNC\...` and device paths `\\.\...`, case-insensitive.
/// Extended local paths such as `\\?\C:\...` are not UNC.
pub fn is_unc_path(path: &str) -> bool {
    let norm: String = path.trim_start().chars().take(16).map(|c| if c == '/' { '\\' } else { c }).collect();
    // `\\?\` is the Win32 extended prefix, `\??\` the NT namespace spelling of the same thing.
    for prefix in [r"\\?\", r"\??\"] {
        if let Some(rest) = norm.strip_prefix(prefix) {
            return rest.get(..3).is_some_and(|s| s.eq_ignore_ascii_case("UNC"))
                && rest.get(3..).is_none_or(|t| t.is_empty() || t.starts_with('\\'));
        }
    }
    norm.starts_with(r"\\")
}

/// Lexically normalizes a scan root for the current platform: one separator style, no `.` or
/// redundant separators, `..` resolved against the preceding component (never above the root),
/// no trailing separator except on a root, and no `\\?\` prefix in front of a drive path.
/// Network (`\\server\share`) and other `\\?\` / `\??\` paths only get one separator style.
/// No I/O: symlinks are not resolved.
pub fn normalize_scan_root(path: &str) -> String {
    normalize_root_for(path, cfg!(windows))
}

pub(crate) fn normalize_root_for(path: &str, windows: bool) -> String {
    let sep = if windows { '\\' } else { '/' };
    let mut s = if windows { path.replace('/', "\\") } else { path.to_owned() };
    if windows {
        // `\\?\C:\x` is the same local path as `C:\x`.
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
            && rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        {
            s = rest.to_owned();
        } else if s.starts_with(r"\\") {
            return s;
        }
    }
    let (prefix, rest) = match s.as_bytes() {
        [d, b':', ..] if windows && d.is_ascii_alphabetic() => s.split_at(2),
        _ => ("", s.as_str()),
    };
    let rooted = rest.starts_with(sep);
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split(sep) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    let mut out = String::from(prefix);
    if rooted {
        out.push(sep);
    }
    out.push_str(&parts.join(&sep.to_string()));
    if out.is_empty() { ".".to_owned() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_are_normalized() {
        let cases_win = [
            ("E:/a/b", r"E:\a\b"),
            (r"E:\a\..\a\.\b\", r"E:\a\b"),
            (r"E:/x/../../y", r"E:\y"),
            (r"\\?\E:\a\b", r"E:\a\b"),
            ("C:", "C:"),
            ("C:/", r"C:\"),
            (r"C:\", r"C:\"),
            (r"\\server\share/x", r"\\server\share\x"),
            (r"\\?\UNC\s\x", r"\\?\UNC\s\x"),
            ("rel/../a//b", r"a\b"),
            ("../a", r"..\a"),
            (".", "."),
        ];
        for (input, want) in cases_win {
            assert_eq!(normalize_root_for(input, true), want, "{input}");
        }
        let cases_unix = [("/a/b/../c/", "/a/c"), ("/..", "/"), ("a//b/./c", "a/b/c"), ("/", "/"), ("x/..", ".")];
        for (input, want) in cases_unix {
            assert_eq!(normalize_root_for(input, false), want, "{input}");
        }
    }

    #[test]
    fn unc_spellings() {
        for p in [
            r"\\server\share",
            "//server/share",
            r"/\server",
            r"\\?\UNC\server\share",
            r"\??\UNC\server\share",
            "/??/unc/server/share",
            r"\\.\C:",
            r"  \\host\x",
        ] {
            assert!(is_unc_path(p), "{p}");
        }
        for p in [r"C:\Users", r"\\?\C:\Users", r"\??\C:\x", r"\\?\UNCLE\x", "/home/user", ""] {
            assert!(!is_unc_path(p), "{p}");
        }
    }
}
