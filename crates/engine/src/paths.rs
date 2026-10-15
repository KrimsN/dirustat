//! Pure path helpers shared by the control channel and the UI (no I/O).

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

#[cfg(test)]
mod tests {
    use super::*;

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
