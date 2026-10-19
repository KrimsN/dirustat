//! Filesystem helpers behind the destructive file commands: re-validation of a tree node against
//! the disk (`docs/safety.md`, principle 3) and link-safe permanent removal.
//!
//! Nothing here follows a symlink or junction when deleting: a link is removed itself, never its
//! target, and walks use `symlink_metadata`. All walks are iterative.

use crate::CommandError;
use crate::protected::{Key, ProtectedPaths, key_of};
use dirustat_model::{ATTR_HARDLINK_DUPLICATE, NodeData, NodeKind};
use std::fs::{self, Metadata};
use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[cfg(windows)]
fn attributes_of(meta: &Metadata) -> u32 {
    std::os::windows::fs::MetadataExt::file_attributes(meta)
}

#[cfg(not(windows))]
fn attributes_of(_meta: &Metadata) -> u32 {
    0
}

/// Symlink, junction or any other reparse point.
pub(crate) fn is_link(meta: &Metadata) -> bool {
    meta.file_type().is_symlink() || attributes_of(meta) & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Modification time in whole seconds since the epoch, computed like the scanner does.
fn mtime_secs(meta: &Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok(),
        Err(before) => i64::try_from(before.duration().as_secs()).ok().map(i64::wrapping_neg),
    }
}

fn stale(why: &str) -> CommandError {
    CommandError::StaleTree(why.to_owned())
}

/// The object at `path` must still be what the tree says: it exists, has the same kind (file,
/// directory, link), and for files the same size; where the tree knows a modification time it
/// must match too. The model stores no file id, so size and mtime are the identity.
pub(crate) fn verify_matches(path: &Path, kind: NodeKind, data: &NodeData) -> Result<(), CommandError> {
    let meta = fs::symlink_metadata(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => stale("the path no longer exists"),
        _ => stale("the path cannot be inspected"),
    })?;
    let on_disk = if is_link(&meta) {
        NodeKind::Link
    } else if meta.is_dir() {
        NodeKind::Dir
    } else {
        NodeKind::File
    };
    if on_disk != kind {
        return Err(stale("the kind of the object changed since the scan"));
    }
    // A second or later hardlink name is stored with size 0 (the file is counted once), so only
    // the first name of a file has a size to compare.
    let hardlink_duplicate = data.attributes & ATTR_HARDLINK_DUPLICATE != 0;
    if kind == NodeKind::File && !hardlink_duplicate && meta.len() != data.logical_size {
        return Err(stale("the file size changed since the scan"));
    }
    if let (Some(seen), Some(now)) = (data.mtime, mtime_secs(&meta))
        && seen != now
    {
        return Err(stale("the modification time changed since the scan"));
    }
    Ok(())
}

/// Win32 device names: `CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9` (also with the
/// superscript digits), `CONIN$`, `CONOUT$`, `CLOCK$`, with or without an extension, any case.
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$") {
        return true;
    }
    let mut chars = stem.chars();
    let head: String = chars.by_ref().take(3).collect();
    matches!(head.as_str(), "COM" | "LPT")
        && matches!(chars.next(), Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'))
        && chars.next().is_none()
}

/// The first reason a single path component cannot be addressed safely on Windows: `:` would name
/// an alternate data stream, trailing dots and spaces are silently dropped by Win32 (two different
/// on-disk names then collapse into one path), device names open a device, and the reserved
/// characters or control characters cannot occur in a name. `None` on other platforms.
pub(crate) fn name_problem(name: &str, windows: bool) -> Option<&'static str> {
    if !windows {
        return None;
    }
    if name.contains(':') {
        return Some("a name contains `:` (alternate data stream)");
    }
    if name.contains(['*', '?', '"', '<', '>', '|']) || name.chars().any(char::is_control) {
        return Some("a name contains a reserved character");
    }
    if name.ends_with(['.', ' ']) {
        return Some("a name ends with a dot or a space");
    }
    if is_reserved_device_name(name) {
        return Some("a name is a reserved Windows device name");
    }
    None
}

/// The object opened through `path` must be the one the tree node names: the real name of the
/// opened object (what `canonicalize` reports, with the on-disk spelling) has to equal the final
/// component exactly. Win32 normalisation (`dup.` -> `dup`), 8.3 short names and similar aliases
/// make the two differ, and the operation is refused instead of hitting a different file. Links
/// are not resolved (a link is removed itself), so for them only the name rules apply.
pub(crate) fn verify_identity(path: &Path, kind: NodeKind) -> Result<(), CommandError> {
    if kind == NodeKind::Link {
        return Ok(());
    }
    let alias = || {
        CommandError::Unsupported(
            "refused: the path resolves to an object with a different name (a Win32 name alias); the item was not \
             touched"
                .to_owned(),
        )
    };
    let want = path.file_name().ok_or_else(alias)?;
    let canon = fs::canonicalize(path).map_err(|_| stale("the path cannot be resolved"))?;
    if canon.file_name() == Some(want) { Ok(()) } else { Err(alias()) }
}

/// Resolves the parent of `path` on disk and checks that it is exactly where the lexical path
/// says: below the real scan root, with no link or reparse point in between, and that the real
/// target is not protected. Returns the real (canonical) key of the parent directory.
pub(crate) fn verify_location(
    path: &str,
    root: &str,
    protected: &ProtectedPaths,
    windows: bool,
) -> Result<Key, CommandError> {
    let outside = || CommandError::InvalidParams("path is outside the scanned tree".to_owned());
    let path_key = key_of(path, windows).ok_or_else(outside)?;
    let root_key = key_of(root, windows).ok_or_else(outside)?;
    if !path_key.starts_with(&root_key) {
        return Err(outside());
    }
    let rel = path_key.comps.get(root_key.comps.len()..).unwrap_or_default();
    let (name, rel_parent) = rel.split_last().ok_or_else(outside)?;
    let parent = Path::new(path).parent().ok_or_else(outside)?;
    let canon_parent = fs::canonicalize(parent).map_err(|_| stale("the parent directory cannot be resolved"))?;
    let canon_root = fs::canonicalize(root).map_err(|_| stale("the scan root cannot be resolved"))?;
    let mut expected = canon_root.clone();
    for comp in rel_parent {
        expected.push(comp);
    }
    let real_parent = key_of(&canon_parent.to_string_lossy(), windows).ok_or_else(outside)?;
    let expected_key = key_of(&expected.to_string_lossy(), windows).ok_or_else(outside)?;
    if real_parent != expected_key {
        return Err(CommandError::ProtectedPath(
            "the path reaches the target through a link or reparse point".to_owned(),
        ));
    }
    let real_target = canon_parent.join(name);
    protected
        .check(&real_target.to_string_lossy(), &canon_root.to_string_lossy())
        .map_err(|why| CommandError::ProtectedPath(format!("the real location is {why}")))?;
    Ok(real_parent)
}

/// Clears the read-only attribute so Windows allows deleting the entry.
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)] // Windows semantics: only the read-only attribute.
fn clear_readonly(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        let mut perms = meta.permissions();
        perms.set_readonly(false);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(windows))]
fn clear_readonly(_path: &Path) {}

fn remove_file_any(path: &Path) -> io::Result<()> {
    fs::remove_file(path).or_else(|first| {
        if first.kind() != io::ErrorKind::PermissionDenied {
            return Err(first);
        }
        clear_readonly(path);
        fs::remove_file(path)
    })
}

fn remove_dir_any(path: &Path) -> io::Result<()> {
    fs::remove_dir(path).or_else(|first| {
        if first.kind() != io::ErrorKind::PermissionDenied {
            return Err(first);
        }
        clear_readonly(path);
        fs::remove_dir(path)
    })
}

/// Removes a link itself: `remove_file` for file links, `remove_dir` for directory links and
/// junctions. The target is never touched.
fn remove_link(path: &Path) -> io::Result<()> {
    fs::remove_file(path).or_else(|_| remove_dir_any(path))
}

/// Permanently deletes `path` and everything below it without following any link.
///
/// A link is removed itself. A directory goes through `std::fs::remove_dir_all`, which does not
/// follow symlinks or junctions inside the tree. Residual race (documented in `docs/safety.md`):
/// between the `symlink_metadata` here and the removal the entry can still be swapped for a link
/// by another process; the callers re-run the location check immediately before calling.
pub(crate) fn remove_permanently(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if is_link(&meta) {
        return remove_link(path);
    }
    if !meta.is_dir() {
        return remove_file_any(path);
    }
    fs::remove_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_tree_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().join("top");
        fs::create_dir_all(top.join("a").join("b")).unwrap();
        fs::write(top.join("a").join("b").join("f.txt"), b"x").unwrap();
        fs::write(top.join("g.txt"), b"y").unwrap();
        let ro = top.join("ro.txt");
        fs::write(&ro, b"z").unwrap();
        let mut perms = fs::metadata(&ro).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&ro, perms).unwrap();
        remove_permanently(&top).unwrap();
        assert!(!top.exists());
        assert!(dir.path().exists());
    }

    #[cfg(windows)]
    #[test]
    fn nested_junction_is_removed_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let (top, target) = (dir.path().join("top"), dir.path().join("target"));
        fs::create_dir_all(&top).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("precious.txt"), b"keep").unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(top.join("junc"))
            .arg(&target)
            .output()
            .is_ok_and(|o| o.status.success());
        if !made {
            return;
        }
        remove_permanently(&top).unwrap();
        assert!(!top.exists());
        assert!(target.join("precious.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn nested_symlink_is_removed_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let (top, target) = (dir.path().join("top"), dir.path().join("target"));
        fs::create_dir_all(&top).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("precious.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(&target, top.join("link")).unwrap();
        remove_permanently(&top).unwrap();
        assert!(!top.exists() && target.join("precious.txt").exists());
    }

    #[test]
    fn missing_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_permanently(&dir.path().join("none")).is_err());
    }

    #[test]
    fn verify_detects_changes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f.bin");
        fs::write(&f, b"12345").unwrap();
        let meta = fs::metadata(&f).unwrap();
        let data = NodeData { logical_size: 5, mtime: mtime_secs(&meta), ..NodeData::default() };
        assert!(verify_matches(&f, NodeKind::File, &data).is_ok());
        assert!(verify_matches(&f, NodeKind::Dir, &data).is_err());
        let wrong_size = NodeData { logical_size: 6, ..data };
        assert_eq!(verify_matches(&f, NodeKind::File, &wrong_size).unwrap_err().code(), "stale_tree");
        let wrong_time = NodeData { mtime: data.mtime.map(|t| t + 10), ..data };
        assert_eq!(verify_matches(&f, NodeKind::File, &wrong_time).unwrap_err().code(), "stale_tree");
        fs::remove_file(&f).unwrap();
        assert_eq!(verify_matches(&f, NodeKind::File, &data).unwrap_err().code(), "stale_tree");
    }

    #[test]
    fn location_must_stay_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("f"), b"x").unwrap();
        let protected = ProtectedPaths::from_env();
        let root_s = root.to_string_lossy().into_owned();
        let ok = root.join("sub").join("f").to_string_lossy().into_owned();
        assert!(verify_location(&ok, &root_s, &protected, cfg!(windows)).is_ok());
        let outside = dir.path().join("other").to_string_lossy().into_owned();
        assert_eq!(verify_location(&outside, &root_s, &protected, cfg!(windows)).unwrap_err().code(), "invalid_params");
        let missing_parent = root.join("nope").join("f").to_string_lossy().into_owned();
        assert_eq!(
            verify_location(&missing_parent, &root_s, &protected, cfg!(windows)).unwrap_err().code(),
            "stale_tree"
        );
    }
}
