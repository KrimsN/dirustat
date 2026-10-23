//! Protected paths for destructive commands (`docs/safety.md`, principle 2).
//!
//! A pure, lexical check (no I/O): the volume root, the scan root, the system directories
//! (`Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`, `Users`), an entire user
//! profile (`C:\Users\<name>`), `AppData` and `NTUSER.DAT*` inside it, the subtrees of `Windows`,
//! `Recovery`, `Boot`, `EFI`, `$Recycle.Bin` and `System Volume Information`, and the direct
//! children of `Program Files*` and `ProgramData`. Administrative UNC shares (`\\host\C$\...`,
//! `\\host\ADMIN$`) are volumes: the same rules apply to the remainder. Trailing dots and spaces
//! of components are ignored like Windows does. Paths are normalized first, so
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

    /// Addressing counterpart of [`Key::starts_with`] for [`key_exact`] keys: components compare
    /// with [`name_matches`] (exact, ASCII case folded on Windows only).
    pub fn starts_with_names(&self, other: &Key, windows: bool) -> bool {
        self.root == other.root
            && self.comps.len() >= other.comps.len()
            && self.comps.iter().zip(&other.comps).all(|(a, b)| name_matches(a, b, windows))
    }

    /// Whether two [`key_exact`] keys name the same location (see [`Key::starts_with_names`]).
    pub fn same_names(&self, other: &Key, windows: bool) -> bool {
        self.comps.len() == other.comps.len() && self.starts_with_names(other, windows)
    }
}

/// Normalizes `path` for the protected-path checks; `None` when it is not an absolute path of the
/// supported forms (relative, drive-relative, device paths such as `\\.\` and `\\?\GLOBALROOT`).
///
/// This is the **permissive** key: surrounding whitespace is trimmed and components are folded with
/// the full Unicode lower-casing, so it over-matches on purpose (the safe direction for a deny
/// list). It must never pick the node a destructive command acts on, see [`key_exact`].
pub(crate) fn key_of(path: &str, windows: bool) -> Option<Key> {
    key_impl(path, windows, false)
}

/// Normalizes `path` for **addressing** (QA-139): nothing is trimmed and the components keep their
/// exact spelling. Only the drive letter and the `\\?\` / `/` spellings are unified. Callers
/// compare components with [`name_matches`], which folds ASCII case only.
pub(crate) fn key_exact(path: &str, windows: bool) -> Option<Key> {
    key_impl(path, windows, true)
}

/// Whether the node name `name` is the path component `comp`: exact equality, except that on
/// Windows ASCII letters compare case-insensitively. No other folding (Kelvin sign, fullwidth
/// letters, normalisation forms) and no trimming, so a request can never alias a different name.
pub(crate) fn name_matches(name: &str, comp: &str, windows: bool) -> bool {
    if windows { name.eq_ignore_ascii_case(comp) } else { name == comp }
}

fn key_impl(path: &str, windows: bool, exact: bool) -> Option<Key> {
    if exact {
        return key_core(path, windows, true);
    }
    let body = path.trim_start();
    let trimmed = body.trim_end();
    let mut key = key_core(trimmed, windows, false)?;
    // G-064 (QA-148): trimming the whole path must not make a name that consists only of Unicode
    // whitespace (NBSP, U+3000, U+0085, ...) vanish, or the key would name the parent (or the scan
    // root) instead of the item. Whitespace that follows a separator is such a final component and
    // is kept; whitespace glued to a name (`dupe<NBSP>`) is still dropped, which over-matches for
    // the deny list on purpose. The key is therefore never shorter than the item's real location.
    let after_separator = if windows { trimmed.ends_with(['\\', '/']) } else { trimmed.ends_with('/') };
    let tail = body.get(trimmed.len()..).unwrap_or_default();
    if after_separator && !tail.is_empty() {
        key.comps.push(if windows { tail.to_lowercase() } else { tail.to_owned() });
    }
    Some(key)
}

fn key_core(path: &str, windows: bool, exact: bool) -> Option<Key> {
    if path.contains('\0') {
        return None;
    }
    let mut p = path.to_owned();
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
    let fold = |s: &str| if windows && !exact { s.to_lowercase() } else { s.to_owned() };
    let sep = if windows { '\\' } else { '/' };
    if windows {
        let b = n.as_bytes();
        if b.len() >= 3
            && b.first().is_some_and(u8::is_ascii_alphabetic)
            && b.get(1) == Some(&b':')
            && b.get(2) == Some(&b'\\')
        {
            let comps = n.get(3..)?.split(sep).filter(|c| !c.is_empty()).map(fold).collect();
            return Some(Key { root: n.get(..2)?.to_ascii_lowercase(), comps });
        }
        if let Some(rest) = n.strip_prefix(r"\\") {
            if rest.starts_with(['?', '.']) {
                return None;
            }
            // `.` and `..` are resolved here: `normalize_root_for` leaves network paths alone.
            let mut comps: Vec<String> = Vec::new();
            for c in rest.split(sep).filter(|c| !c.is_empty()) {
                match c {
                    "." => {}
                    ".." => {
                        comps.pop();
                    }
                    other => comps.push(fold(other)),
                }
            }
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
const WIN_TOP_SUBTREE: &[&str] = &["windows", "system volume information", "recovery", "boot", "efi", "$recycle.bin"];
/// Top-level names whose direct children are protected too (installed programs, shared app data).
const WIN_TOP_FIRST_LEVEL: &[&str] = &["program files", "program files (x86)", "programdata"];
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
    /// The user profile from the environment (its `AppData` and `NTUSER.DAT*` are protected).
    homes: Vec<Key>,
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
        let mut homes = Vec::new();
        let home = env(if windows { "USERPROFILE" } else { "HOME" });
        if let Some(k) = home.as_deref().and_then(|v| key_of(v, windows)) {
            // The profile itself and the directory that holds all profiles.
            let mut parent = k.clone();
            parent.comps.pop();
            exact.push(k.clone());
            exact.push(parent);
            if windows {
                homes.push(k);
            }
        }
        Self { windows, exact, subtree, homes }
    }

    /// `Err(reason)` when `path` must never be deleted or moved by a command.
    pub fn check(&self, path: &str, scan_root: &str) -> Result<(), String> {
        let key = key_of(path, self.windows).ok_or_else(|| "path is not an absolute local path".to_owned())?;
        let key = self.trimmed(key);
        if let Some(root) = key_of(scan_root, self.windows).map(|r| self.trimmed(r))
            && root == key
        {
            return Err("the scan root itself".to_owned());
        }
        if self.windows
            && key.root == r"\\"
            && let Some(local) = admin_share_as_local(&key)
        {
            // `\\host\C$\Windows` is `C:\Windows` reached over the network.
            return self.check_key(&local).map_err(|why| format!("{why} (via an administrative share)"));
        }
        if self.windows && key.root == r"\\" && key.comps.get(1).is_some_and(|s| s.ends_with('$')) {
            return Err("an administrative share".to_owned());
        }
        self.check_key(&key)
    }

    /// Windows ignores trailing dots and spaces of a path component (`C:\Windows.` is
    /// `C:\Windows`); components are trimmed the same way so the table cannot be dodged.
    fn trimmed(&self, mut key: Key) -> Key {
        if self.windows {
            key.comps = key
                .comps
                .into_iter()
                .map(|c| c.trim_end_matches(['.', ' ']).to_owned())
                .filter(|c| !c.is_empty())
                .collect();
        }
        key
    }

    fn check_key(&self, key: &Key) -> Result<(), String> {
        if Self::is_volume_root(key) {
            return Err("a volume root".to_owned());
        }
        if self.exact.contains(key) {
            return Err("a system or user-profile directory".to_owned());
        }
        if self.subtree.iter().any(|s| key.starts_with(s)) {
            return Err("inside the system directory".to_owned());
        }
        if self.homes.iter().any(|h| Self::in_profile_internals(key, h)) {
            return Err("profile data (AppData, NTUSER.DAT)".to_owned());
        }
        if self.windows { Self::check_windows(key) } else { Self::check_unix(key) }
    }

    fn is_volume_root(key: &Key) -> bool {
        if key.root == r"\\" { key.comps.len() <= 2 } else { key.comps.is_empty() }
    }

    /// `<home>\AppData` (and its first level) and `<home>\NTUSER.DAT*`, for the profile named by the environment.
    fn in_profile_internals(key: &Key, home: &Key) -> bool {
        if key.root != home.root || !key.comps.starts_with(&home.comps) {
            return false;
        }
        let rel = key.comps.get(home.comps.len()..).unwrap_or_default();
        profile_internals(rel)
    }

    fn check_windows(key: &Key) -> Result<(), String> {
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
        let short = is_short_form_of_system(top);
        // Direct children of `Program Files*` and `ProgramData` (installed products, shared data).
        if key.comps.len() == 2 && (WIN_TOP_FIRST_LEVEL.contains(&top.as_str()) || short) {
            return Err("an installed program or shared data directory".to_owned());
        }
        // `C:\Users\<name>` and `C:\Documents and Settings\<name>`: an entire user profile.
        let profile_holder = matches!(top.as_str(), "users" | "documents and settings") || short;
        if key.comps.len() == 2 && profile_holder {
            return Err("an entire user profile".to_owned());
        }
        if profile_holder && key.comps.get(2..).is_some_and(profile_internals) {
            return Err("profile data (AppData, NTUSER.DAT)".to_owned());
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

/// Components below a profile root that are never deleted by a command: `AppData` and its first
/// level (`Local`, `Roaming`, `LocalLow`), the 8.3 form of `AppData`, and `NTUSER.DAT*`.
fn profile_internals(rel: &[String]) -> bool {
    let Some(first) = rel.first() else { return false };
    let appdata = first == "appdata" || is_short_form_of(first, "appdata");
    (appdata && rel.len() <= 2) || (rel.len() == 1 && first.starts_with("ntuser.dat"))
}

/// The local-drive equivalent of an administrative share: `\\host\C$\x` -> `c:` + `x`.
/// `ADMIN$` is the Windows directory itself.
fn admin_share_as_local(key: &Key) -> Option<Key> {
    let share = key.comps.get(1)?;
    let rest = key.comps.get(2..).unwrap_or_default().to_vec();
    if share == "admin$" {
        let mut comps = vec!["windows".to_owned()];
        comps.extend(rest);
        return Some(Key { root: "c:".to_owned(), comps });
    }
    let letter = share.strip_suffix('$')?;
    let mut chars = letter.chars();
    let c = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next().is_some() {
        return None;
    }
    Some(Key { root: format!("{c}:"), comps: rest })
}

/// `name~N` where the stem is a prefix of `base`.
fn is_short_form_of(name: &str, base: &str) -> bool {
    let Some((stem, digits)) = name.rsplit_once('~') else { return false };
    !stem.is_empty() && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && base.starts_with(stem)
}

/// `PROGRA~1`-style 8.3 name whose stem is a prefix of a protected top-level name.
fn is_short_form_of_system(name: &str) -> bool {
    WIN_SHORT_BASES.iter().any(|base| is_short_form_of(name, base))
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
            r"C:\Program Files\OldApp\cache",
            r"C:\ProgramData\Vendor\Cache",
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
    fn unc_forms_of_system_locations_are_refused() {
        let p = win();
        let root = r"D:\scan";
        for path in [
            r"\\localhost\c$\Windows",
            r"\\localhost\C$\Windows\System32\x",
            r"\\127.0.0.1\C$",
            r"\\127.0.0.1\C$\",
            r"\\host\ADMIN$",
            r"\\host\admin$\system32",
            r"\\host\IPC$",
            r"\\host\d$\Windows\x",
            r"\\host\c$\Program Files",
            r"\\host\c$\Program Files\App",
            r"\\host\c$\ProgramData\Vendor",
            r"\\host\c$\Users\bob",
            r"\\host\c$\Users\bob\AppData",
            r"\\host\c$\Users\bob\NTUSER.DAT",
            r"\\?\UNC\localhost\c$\Windows\x",
            r"//localhost/c$/Windows/x",
            r"\\localhost\c$\Temp\..\Windows",
            r"\\host\share\..\c$\Windows",
            r"\\localhost\c$\Windows.\x",
            r"\\host\share",
        ] {
            assert!(p.check(path, root).is_err(), "{path:?} must be refused");
        }
        for path in [r"\\host\share\dir", r"\\localhost\c$\Temp\x", r"\\?\UNC\host\share\a\b"] {
            assert!(p.check(path, root).is_ok(), "{path:?} must be allowed by the lexical table");
        }
    }

    #[test]
    fn subtrees_first_levels_and_profile_internals() {
        let p = win();
        let root = r"D:\scan";
        for path in [
            r"C:\Recovery\x",
            r"C:\Boot\BCD",
            r"C:\EFI\Microsoft",
            r"C:\$Recycle.Bin\S-1-5\x",
            r"C:\System Volume Information\x\y",
            r"C:\Windows\x\y\z",
            r"C:\Windows.\x",
            r"C:\Windows \System32",
            r"C:\Program Files\App",
            r"C:\Program Files (x86)\App",
            r"C:\ProgramData\Vendor",
            r"C:\PROGRA~1\App",
            r"C:\Users\alice\AppData",
            r"C:\Users\alice\AppData\Local",
            r"C:\Users\alice\AppData\Roaming",
            r"C:\Users\bob\AppData\LocalLow",
            r"C:\Users\bob\APPDAT~1",
            r"C:\Users\bob\NTUSER.DAT",
            r"C:\Users\bob\ntuser.dat.LOG1",
            r"C:\Documents and Settings\bob\AppData",
        ] {
            assert!(p.check(path, root).is_err(), "{path:?} must be refused");
        }
        for path in [
            r"C:\Program Files\App\sub",
            r"C:\Users\alice\AppData\Local\Temp",
            r"C:\Users\alice\AppData\Local\Temp\x.tmp",
            r"C:\Users\alice\Downloads\ntuser.dat",
        ] {
            assert!(p.check(path, root).is_ok(), "{path:?} must be allowed");
        }
        let env = |name: &str| (name == "USERPROFILE").then(|| r"F:\Profiles\carol".to_owned());
        let custom = ProtectedPaths::build(true, &env);
        for path in [r"F:\Profiles\carol\AppData", r"F:\Profiles\carol\AppData\Local", r"F:\Profiles\carol\NTUSER.DAT1"]
        {
            assert!(custom.check(path, r"G:\x").is_err(), "{path}");
        }
        assert!(custom.check(r"F:\Profiles\carol\AppData\Local\Temp", r"G:\x").is_ok());
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

    /// QA-139: the addressing key keeps names exact, the protection key stays permissive.
    #[test]
    fn exact_key_keeps_names_while_the_protection_key_over_matches() {
        let exact = key_exact("C:\\Root\\dupe\u{a0}", true).unwrap();
        assert_eq!(exact.root, "c:");
        assert_eq!(exact.comps, ["Root", "dupe\u{a0}"]);
        assert_eq!(key_exact(" C:\\x", true), None, "no trimming: a padded path is not absolute");
        assert_eq!(key_exact("C:\\x\\", true).unwrap().comps, ["x"]);
        assert_eq!(key_exact("/a/B", false).unwrap().comps, ["a", "B"]);
        assert_eq!(key_exact(r"\\?\UNC\Srv\Share\X", true).unwrap().comps, ["Srv", "Share", "X"]);
        // The permissive key still trims and folds, so the deny list over-matches.
        let loose = key_of("C:\\WINDOWS\u{a0}", true).unwrap();
        assert_eq!(loose.comps, ["windows"]);
        assert!(win().check("C:\\Windows\u{a0}", r"D:\scan").is_err());
    }

    /// G-064 (QA-148): a final component made only of Unicode whitespace is a component of the
    /// permissive key too, so the key never collapses into its parent or the scan root.
    #[test]
    fn whitespace_only_final_component_survives_the_permissive_key() {
        for ws in ["\u{a0}", "\u{3000}", "\u{2003}", "\u{85}", "\u{a0}\u{a0}", "\u{a0}\u{3000}"] {
            let loose = key_of(&format!("C:\\Root\\Sub\\{ws}"), true).unwrap();
            assert_eq!(loose.comps, ["root", "sub", ws], "{ws:?}");
            let at_root = key_of(&format!("C:\\Root\\{ws}"), true).unwrap();
            assert_eq!(at_root.comps, ["root", ws], "{ws:?}");
            assert_eq!(key_exact(&format!("C:\\Root\\{ws}"), true).unwrap().comps, ["Root", ws], "{ws:?}");
            // The check no longer mistakes the item for its parent or for the scan root.
            assert!(win().check(&format!("D:\\scan\\{ws}"), r"D:\scan").is_ok(), "{ws:?}");
            assert!(win().check(&format!("D:\\scan\\sub\\{ws}"), r"D:\scan").is_ok(), "{ws:?}");
        }
        // Whitespace glued to a name is still dropped (over-matching is the safe direction).
        assert_eq!(key_of("C:\\Root\\dupe\u{a0}", true).unwrap().comps, ["root", "dupe"]);
        assert_eq!(key_of("  C:\\Root\\dupe \t\n", true).unwrap().comps, ["root", "dupe"]);
        assert_eq!(key_of("/a/b\u{a0}", false).unwrap().comps, ["a", "b"]);
        assert_eq!(key_of("/a/\u{3000}", false).unwrap().comps, ["a", "\u{3000}"]);
    }

    /// G-064: keeping the whitespace-only component does not weaken the deny list.
    #[test]
    fn whitespace_variants_of_protected_paths_stay_refused() {
        let p = win();
        for path in [
            "C:\\Windows\u{a0}",
            "C:\\Windows\\\u{a0}",
            "C:\\Windows\\System32\\\u{3000}",
            "C:\\Users\\\u{a0}",
            "C:\\\u{a0}\\..",
            "C:\\Program Files\u{2003}",
            "D:\\scan\u{a0}",
            "D:\\scan\\sub\\..\\\u{a0}\\..",
        ] {
            assert!(p.check(path, r"D:\scan").is_err(), "{path:?} must stay refused");
        }
    }

    #[test]
    fn exact_keys_compare_with_name_matches() {
        let a = key_exact(r"C:\Root\Sub", true).unwrap();
        let b = key_exact(r"c:\root\SUB", true).unwrap();
        assert!(a.same_names(&b, true) && !a.same_names(&b, false));
        let child = key_exact("C:\\Root\\Sub\\\u{a0}", true).unwrap();
        assert!(child.starts_with_names(&a, true) && !a.starts_with_names(&child, true) && !a.same_names(&child, true));
        let lookalike = key_exact("C:\\Root\\Sub\u{a0}", true).unwrap();
        assert!(!lookalike.same_names(&a, true) && !lookalike.starts_with_names(&a, true));
    }

    #[test]
    fn names_match_exactly_except_ascii_case_on_windows() {
        assert!(name_matches("Readme.TXT", "readme.txt", true));
        assert!(!name_matches("Readme.TXT", "readme.txt", false));
        for (name, comp) in
            [("k", "\u{212A}"), ("a", "\u{ff41}"), ("\u{ff41}", "\u{ff21}"), ("dupe", "dupe\u{a0}"), ("dupe", "dupe ")]
        {
            assert!(!name_matches(name, comp, true), "{name:?} vs {comp:?}");
            assert!(!name_matches(comp, name, true), "{comp:?} vs {name:?}");
        }
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
