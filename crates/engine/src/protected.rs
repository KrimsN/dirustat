//! Protected paths for destructive commands (`docs/safety.md`, principle 2).
//!
//! A pure, lexical check (no I/O): the volume root, the scan root, the system directories
//! (`Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`, `Users`), an entire user
//! profile (`C:\Users\<name>`) and everything below `Windows`. Paths are normalized first, so
//! case, trailing separators, `\\?\` prefixes, forward slashes, `.`/`..` components and 8.3 short
//! names of the top-level directories cannot be used to slip past the list. The check runs on
//! the lexical path and again on the canonical path resolved from disk (see `cmd::file`), which
//! covers system directories reached through a reparse point.

use crate::paths::normalize_root_for;

/// A normalized absolute path: a root marker and lower-cased components.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    /// `c:` for a drive, `\\` for a network path, `/` for a Unix path.
    pub root: String,
    pub comps: Vec<String>,
}

impl Key {
    /// True when `self` is `other` or lies below it.
    pub fn starts_with(&self, other: &Key) -> bool {
        self.root == other.root && self.comps.starts_with(&other.comps)
    }
}

/// Normalizes `path`; `None` when it is not an absolute path of the supported forms (relative,
/// drive-relative, device paths such as `\\.\` and `\\?\GLOBALROOT`).
pub(crate) fn key_of(path: &str, windows: bool) -> Option<Key> {
    if path.contains('\0') {
        return None;
    }
    let mut p = path.trim().to_owned();
    if windows {
        let unified = p.replace('/', "\\");
        // `\\?\UNC\server\share` is the network path `\\server\share`.
        let lower = unified.get(..8).map(str::to_ascii_lowercase);
        if lower.as_deref() == Some(r"\\?\unc\") {
            p = format!(r"\\{}", unified.get(8..)?);
        } else if unified.starts_with(r"\??\") || unified.starts_with(r"\\.\") {
            return None;
        } else {
            p = unified;
        }
    }
    let n = normalize_root_for(&p, windows);
    let fold = |s: &str| if windows { s.to_lowercase() } else { s.to_owned() };
    let sep = if windows { '\\' } else { '/' };
    if windows {
        let b = n.as_bytes();
        if b.len() >= 3
            && b.first().is_some_and(u8::is_ascii_alphabetic)
            && b.get(1) == Some(&b':')
            && b.get(2) == Some(&b'\\')
        {
            let comps = n.get(3..)?.split(sep).filter(|c| !c.is_empty()).map(fold).collect();
            return Some(Key { root: fold(n.get(..2)?), comps });
        }
        if let Some(rest) = n.strip_prefix(r"\\") {
            if rest.starts_with(['?', '.']) {
                return None;
            }
            let comps = rest.split(sep).filter(|c| !c.is_empty()).map(fold).collect();
            return Some(Key { root: r"\\".to_owned(), comps });
        }
        None
    } else {
        let rest = n.strip_prefix('/')?;
        Some(Key { root: "/".to_owned(), comps: rest.split(sep).filter(|c| !c.is_empty()).map(fold).collect() })
    }
}

/// Top-level names that are protected on any drive (lower case).
const WIN_TOP_EXACT: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "users",
    "documents and settings",
    "recovery",
    "boot",
    "efi",
    "$recycle.bin",
    "system volume information",
];
/// Top-level names whose whole subtree is protected.
const WIN_TOP_SUBTREE: &[&str] = &["windows", "system volume information"];
/// Names used to recognize 8.3 short forms (`PROGRA~1`), spaces and parentheses removed.
const WIN_SHORT_BASES: &[&str] =
    &["programfiles", "programfiles(x86)", "programdata", "windows", "users", "documentsandsettings"];
const UNIX_TOP: &[&str] = &[
    "bin",
    "boot",
    "dev",
    "etc",
    "home",
    "lib",
    "lib64",
    "proc",
    "root",
    "run",
    "sbin",
    "sys",
    "usr",
    "var",
    "system",
    "library",
    "users",
    "applications",
    "volumes",
    "private",
];

/// The set of protected locations, built once per check from the environment.
#[derive(Clone, Debug)]
pub(crate) struct ProtectedPaths {
    windows: bool,
    exact: Vec<Key>,
    subtree: Vec<Key>,
}

impl ProtectedPaths {
    /// Built from the process environment (`SystemRoot`, `ProgramFiles`, `USERPROFILE`, ...).
    pub fn from_env() -> Self {
        Self::build(cfg!(windows), &|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
    }

    pub fn build(windows: bool, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut exact = Vec::new();
        let mut subtree = Vec::new();
        let add = |list: &mut Vec<Key>, value: Option<String>| {
            if let Some(k) = value.and_then(|v| key_of(&v, windows)) {
                list.push(k);
            }
        };
        if windows {
            for name in ["SystemRoot", "windir"] {
                add(&mut subtree, env(name));
            }
            for name in
                ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "PUBLIC", "ALLUSERSPROFILE"]
            {
                add(&mut exact, env(name));
            }
        }
        let home = env(if windows { "USERPROFILE" } else { "HOME" });
        if let Some(k) = home.as_deref().and_then(|v| key_of(v, windows)) {
            // The profile itself and the directory that holds all profiles.
            let mut parent = k.clone();
            parent.comps.pop();
            exact.push(k);
            exact.push(parent);
        }
        Self { windows, exact, subtree }
    }

    /// `Err(reason)` when `path` must never be deleted or moved by a command.
    pub fn check(&self, path: &str, scan_root: &str) -> Result<(), String> {
        let key = key_of(path, self.windows).ok_or_else(|| "path is not an absolute local path".to_owned())?;
        if let Some(root) = key_of(scan_root, self.windows)
            && root == key
        {
            return Err("the scan root itself".to_owned());
        }
        if Self::is_volume_root(&key) {
            return Err("a volume root".to_owned());
        }
        if self.exact.contains(&key) {
            return Err("a system or user-profile directory".to_owned());
        }
        if self.subtree.iter().any(|s| key.starts_with(s)) {
            return Err("inside the system directory".to_owned());
        }
        if self.windows { self.check_windows(&key) } else { Self::check_unix(&key) }
    }

    fn is_volume_root(key: &Key) -> bool {
        if key.root == r"\\" { key.comps.len() <= 2 } else { key.comps.is_empty() }
    }

    fn check_windows(&self, key: &Key) -> Result<(), String> {
        if key.root == r"\\" {
            return Ok(());
        }
        let Some(top) = key.comps.first() else { return Ok(()) };
        if WIN_TOP_SUBTREE.contains(&top.as_str()) {
            return Err("inside the system directory".to_owned());
        }
        if key.comps.len() == 1 && (WIN_TOP_EXACT.contains(&top.as_str()) || is_short_form_of_system(top)) {
            return Err("a system directory".to_owned());
        }
        // `C:\Users\<name>` and `C:\Documents and Settings\<name>`: an entire user profile.
        let profile_holder = matches!(top.as_str(), "users" | "documents and settings") || is_short_form_of_system(top);
        if key.comps.len() == 2 && profile_holder {
            return Err("an entire user profile".to_owned());
        }
        Ok(())
    }

    fn check_unix(key: &Key) -> Result<(), String> {
        let Some(top) = key.comps.first() else { return Ok(()) };
        if key.comps.len() == 1 && UNIX_TOP.contains(&top.as_str()) {
            return Err("a system directory".to_owned());
        }
        if key.comps.len() == 2 && matches!(top.as_str(), "home" | "users") {
            return Err("an entire user profile".to_owned());
        }
        Ok(())
    }
}

/// `PROGRA~1`-style 8.3 name whose stem is a prefix of a protected top-level name.
fn is_short_form_of_system(name: &str) -> bool {
    let Some((stem, digits)) = name.rsplit_once('~') else { return false };
    !stem.is_empty()
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && WIN_SHORT_BASES.iter().any(|base| base.starts_with(stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win() -> ProtectedPaths {
        let env = |name: &str| match name {
            "SystemRoot" | "windir" => Some(r"C:\Windows".to_owned()),
            "ProgramFiles" => Some(r"C:\Program Files".to_owned()),
            "ProgramFiles(x86)" => Some(r"C:\Program Files (x86)".to_owned()),
            "ProgramData" => Some(r"C:\ProgramData".to_owned()),
            "USERPROFILE" => Some(r"C:\Users\alice".to_owned()),
            _ => None,
        };
        ProtectedPaths::build(true, &env)
    }

    #[test]
    fn windows_protected_table() {
        let p = win();
        let root = r"D:\scan";
        let refused = [
            r"C:\",
            "C:",
            r"c:\",
            r"C:/",
            r"\\?\C:\",
            r"C:\Windows",
            r"c:\WINDOWS\",
            r"C:/Windows/System32",
            r"\\?\C:\Windows\System32\drivers",
            r"C:\Program Files",
            r"c:\program files\",
            r"C:\Program Files (x86)",
            r"C:\PROGRA~1",
            r"C:\PROGRA~2",
            r"C:\ProgramData",
            r"C:\Users",
            r"C:\Users\",
            r"C:\Users\alice",
            r"C:\Users\Bob",
            r"C:\users\bob\.",
            r"C:\Users\KRIMSN~1",
            r"C:\Users\alice\Documents\..",
            r"C:\Users\alice\Downloads\..\..\alice",
            r"C:\Temp\..\Windows",
            r"C:\Temp\..\..\..\Windows\System32",
            r"D:\Windows",
            r"D:\",
            r"D:\scan",
            r"d:\SCAN\",
            r"D:/scan/",
            r"D:\scan\sub\..",
            r"\\?\D:\scan",
            r"\\server\share",
            r"\\?\UNC\server\share",
            r"C:\System Volume Information",
            r"C:\$Recycle.Bin",
            r"relative\x",
            "",
            r"\\.\C:",
            r"\??\C:\x",
            "C:rel",
            "C:\\a\0b",
        ];
        for path in refused {
            assert!(p.check(path, root).is_err(), "{path:?} must be refused");
        }
        let allowed = [
            r"C:\Users\alice\Downloads",
            r"C:\Users\alice\Downloads\big.iso",
            r"D:\scan\sub",
            r"d:/scan/sub/file.txt",
            r"C:\Program Files\OldApp",
            r"C:\ProgramData\Cache",
            r"\\?\D:\scan\sub\x",
            r"D:\data",
            r"C:\Temp",
            r"C:\Windows.old\x",
            r"C:\Windows.old",
            r"\\server\share\dir",
        ];
        for path in allowed {
            assert!(p.check(path, root).is_ok(), "{path:?} must be allowed");
        }
    }

    #[test]
    fn env_roots_extend_the_literal_list() {
        let env = |name: &str| match name {
            "SystemRoot" => Some(r"E:\WINNT".to_owned()),
            "ProgramData" => Some(r"E:\AppData".to_owned()),
            "USERPROFILE" => Some(r"F:\Profiles\carol".to_owned()),
            _ => None,
        };
        let p = ProtectedPaths::build(true, &env);
        for path in [r"e:\winnt\system32", r"E:\AppData", r"F:\Profiles\carol", r"F:\Profiles"] {
            assert!(p.check(path, r"G:\x").is_err(), "{path}");
        }
        assert!(p.check(r"F:\Profiles\carol\Downloads", r"G:\x").is_ok());
    }

    #[test]
    fn unix_protected_table() {
        let env = |name: &str| (name == "HOME").then(|| "/home/dave".to_owned());
        let p = ProtectedPaths::build(false, &env);
        for path in ["/", "/usr", "/usr/", "/etc/../usr", "/home", "/home/erin", "/home/dave", "rel", "/proj"] {
            let r = p.check(path, "/proj");
            assert!(r.is_err(), "{path}");
        }
        assert!(p.check("/home/dave/Downloads/x", "/proj").is_ok());
        assert!(p.check("/proj/a", "/proj").is_ok());
        assert!(p.check("/opt/app", "/proj").is_ok());
    }

    #[test]
    fn short_forms_need_digits_and_a_known_stem() {
        assert!(is_short_form_of_system("progra~1"));
        assert!(is_short_form_of_system("windows~1"));
        assert!(!is_short_form_of_system("progra~x"));
        assert!(!is_short_form_of_system("photos~1"));
        assert!(!is_short_form_of_system("~1"));
    }
}
