//! Re-walk of an on-disk folder against the tree subtree it was scanned as (QA-118).
//!
//! The per-node stale check of `fsops::verify_matches` looks at one object. A folder is more than
//! itself: files added or rewritten at any depth are invisible to it, so a permanent delete or a
//! move of a folder could take along content the user never saw in the preview. [`verify`] reads
//! the real subtree (iterative, never following links, bounded by an entry cap and a deadline) and
//! compares it with the tree directory by directory: the same names and kinds, and for files and
//! files the same size and whole-second modification time where the tree has them.

use dirustat_model::{ATTR_HARDLINK_DUPLICATE, NodeId, NodeKind, Tree};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Most disk entries one verification reads; a larger folder is refused (use the recycle bin).
pub(crate) const MAX_ENTRIES: usize = 2_000_000;
/// Wall-clock limit of one verification.
pub(crate) const TIME_LIMIT: Duration = Duration::from_secs(30);

/// What one name looks like on one side: how many entries share it, and the details of the first.
#[derive(Clone, Copy, Default)]
struct Facts {
    count: u32,
    size: Option<u64>,
    mtime: Option<i64>,
    /// The name is not valid Unicode and was stored with U+FFFD in place of the bad units.
    lossy: bool,
}

/// Names are keyed as text, so two different raw names can share a key (a lone surrogate and a
/// literal U+FFFD both become `\u{FFFD}`). Such entries cannot be told apart, so a folder holding
/// them is never verified as "unchanged" (QA-140): the caller is told to use the recycle bin.
fn ambiguous(dir_path: &std::path::Path, name: &str) -> String {
    format!(
        "{} holds entries whose names are identical after conversion to text ({name:?}); they cannot be told \
         apart, so the folder cannot be verified; use the recycle bin",
        dir_path.display()
    )
}

type Listing = HashMap<(String, u8), Facts>;

fn kind_code(kind: NodeKind) -> u8 {
    match kind {
        NodeKind::Dir => 0,
        NodeKind::File => 1,
        NodeKind::Link => 2,
    }
}

fn mtime_secs(meta: &fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok(),
        Err(before) => i64::try_from(before.duration().as_secs()).ok().map(i64::wrapping_neg),
    }
}

/// Compares the folder `node` of `tree`, which is at `path` on disk, with the real contents.
/// `tolerate_extra`: the tree may omit entries that exist on disk (exclusions, read errors), so
/// extra disk entries are not a difference; everything the tree has must still match.
/// `Err` carries a human-readable description of the first difference (or of the limit hit).
pub(crate) fn verify(tree: &Tree, node: NodeId, path: PathBuf, tolerate_extra: bool) -> Result<(), String> {
    let deadline = Instant::now().checked_add(TIME_LIMIT);
    let mut stack: Vec<(NodeId, PathBuf)> = vec![(node, path)];
    let mut entries = 0usize;
    while let Some((dir, dir_path)) = stack.pop() {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err("verifying the folder took too long; use the recycle bin or delete smaller folders".to_owned());
        }
        let mut disk: Listing = HashMap::new();
        let mut disk_dirs: HashMap<String, PathBuf> = HashMap::new();
        let read = fs::read_dir(&dir_path).map_err(|_| format!("{} cannot be read", dir_path.display()))?;
        for item in read {
            entries = entries.saturating_add(1);
            if entries > MAX_ENTRIES {
                return Err(format!(
                    "the folder has more than {MAX_ENTRIES} entries and cannot be verified; use the recycle bin"
                ));
            }
            let item = item.map_err(|_| format!("{} cannot be listed completely", dir_path.display()))?;
            let meta = item.metadata().map_err(|_| format!("{} cannot be inspected", item.path().display()))?;
            let kind = if crate::fsops::is_link(&meta) {
                NodeKind::Link
            } else if meta.is_dir() {
                NodeKind::Dir
            } else {
                NodeKind::File
            };
            let raw_name = item.file_name();
            let name = raw_name.to_string_lossy().into_owned();
            let facts = disk.entry((name.clone(), kind_code(kind))).or_default();
            facts.count = facts.count.saturating_add(1);
            if facts.count > 1 {
                return Err(ambiguous(&dir_path, &name));
            }
            if facts.count == 1 {
                facts.lossy = raw_name.to_str().is_none();
                facts.size = (kind == NodeKind::File).then_some(meta.len());
                // Folder times in a listing are lazily updated copies on NTFS (a stale value is normal
                // there), so only files carry a time; folder changes show up as name/size differences.
                facts.mtime = if kind == NodeKind::File { mtime_secs(&meta) } else { None };
                if kind == NodeKind::Dir {
                    disk_dirs.insert(name, item.path());
                }
            }
        }
        let mut seen: Listing = HashMap::new();
        let mut hardlink_names: Vec<(String, u8)> = Vec::new();
        for child in tree.children(dir) {
            let Some(n) = tree.get(child) else { continue };
            let key = (n.name().to_owned(), kind_code(n.kind()));
            let facts = seen.entry(key.clone()).or_default();
            facts.count = facts.count.saturating_add(1);
            if facts.count > 1 {
                return Err(ambiguous(&dir_path, n.name()));
            }
            if facts.count == 1 {
                facts.lossy = n.data().name_lossy;
                facts.size = (n.kind() == NodeKind::File).then(|| n.data().logical_size);
                facts.mtime = if n.kind() == NodeKind::File { n.data().mtime } else { None };
                if n.data().attributes & ATTR_HARDLINK_DUPLICATE != 0 {
                    hardlink_names.push(key);
                }
            }
        }
        for (key, want) in &seen {
            let here = || format!("{}{}{}", dir_path.display(), std::path::MAIN_SEPARATOR, key.0);
            let Some(have) = disk.get(key) else {
                return Err(format!("{} was removed or changed its kind since the scan", here()));
            };
            if have.count != want.count {
                return Err(format!("the number of entries named {} changed since the scan", here()));
            }
            if have.lossy != want.lossy {
                return Err(format!("{} changed the spelling of its name since the scan", here()));
            }
            let skip_size = hardlink_names.contains(key);
            if !skip_size && want.size.is_some() && want.size != have.size {
                return Err(format!("{} changed its size since the scan", here()));
            }
            if let (Some(a), Some(b)) = (want.mtime, have.mtime)
                && a != b
            {
                return Err(format!("{} changed its modification time since the scan", here()));
            }
        }
        if !tolerate_extra && let Some(key) = disk.keys().find(|k| !seen.contains_key(*k)) {
            return Err(format!("{}{}{} is new since the scan", dir_path.display(), std::path::MAIN_SEPARATOR, key.0));
        }
        for child in tree.children(dir) {
            let Some(n) = tree.get(child) else { continue };
            if n.kind() != NodeKind::Dir {
                continue;
            }
            // Names are unique per kind here (a collision was refused above), so the folder is
            // found by name and every one of them is descended.
            if let Some(real) = disk_dirs.get(n.name()) {
                stack.push((child, real.clone()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsops::is_link;
    use dirustat_scan::{ScanOptions, start};

    fn scan(root: &std::path::Path) -> Tree {
        let handle = start(root, ScanOptions::default()).unwrap();
        handle.join().unwrap().tree
    }

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(sub.join("deep")).unwrap();
        fs::write(sub.join("a.txt"), b"aaaa").unwrap();
        fs::write(sub.join("deep").join("b.txt"), b"bb").unwrap();
        (dir, sub)
    }

    fn sub_node(tree: &Tree) -> NodeId {
        tree.children(tree.root()).find(|&c| tree.get(c).is_some_and(|n| n.name() == "sub")).unwrap()
    }

    #[test]
    fn unchanged_subtree_passes() {
        let (dir, sub) = fixture();
        let tree = scan(dir.path());
        assert_eq!(verify(&tree, sub_node(&tree), sub, false), Ok(()));
    }

    #[test]
    fn new_file_deep_down_is_detected() {
        let (dir, sub) = fixture();
        let tree = scan(dir.path());
        fs::write(sub.join("deep").join("NEW.docx"), b"precious").unwrap();
        let err = verify(&tree, sub_node(&tree), sub.clone(), false).unwrap_err();
        assert!(err.contains("NEW.docx"), "{err}");
        // A partial tree (exclusions) tolerates extra disk entries.
        assert_eq!(verify(&tree, sub_node(&tree), sub, true), Ok(()));
    }

    #[test]
    fn rewritten_and_removed_files_are_detected() {
        let (dir, sub) = fixture();
        let tree = scan(dir.path());
        fs::write(sub.join("deep").join("b.txt"), b"much longer content").unwrap();
        assert!(verify(&tree, sub_node(&tree), sub.clone(), true).unwrap_err().contains("size"));
        fs::remove_file(sub.join("deep").join("b.txt")).unwrap();
        assert!(verify(&tree, sub_node(&tree), sub, true).unwrap_err().contains("removed"));
    }

    /// A name that is not valid Unicode (lone surrogate on Windows, bad bytes on Unix) next to a
    /// sibling whose lossy text is identical: `(lone, literal)`.
    #[cfg(any(windows, unix))]
    fn colliding_names(stem: &str) -> (std::ffi::OsString, std::ffi::OsString) {
        #[cfg(windows)]
        let lone = {
            use std::os::windows::ffi::OsStringExt;
            let wide: Vec<u16> = stem.encode_utf16().chain([0xD800]).collect();
            std::ffi::OsString::from_wide(&wide)
        };
        #[cfg(unix)]
        let lone = {
            use std::os::unix::ffi::OsStringExt;
            let mut bytes = stem.as_bytes().to_vec();
            bytes.push(0xFF);
            std::ffi::OsString::from_vec(bytes)
        };
        (lone, std::ffi::OsString::from(format!("{stem}\u{FFFD}")))
    }

    /// QA-140: two folders whose names collide after lossy conversion were compared by count only
    /// and never descended, so new content inside them was deleted unseen.
    #[cfg(any(windows, unix))]
    #[test]
    fn colliding_folder_names_are_not_verified_as_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        let (lone, literal) = colliding_names("d");
        for name in [&lone, &literal] {
            if fs::create_dir_all(sub.join(name)).is_err() {
                return; // this file system cannot hold such names
            }
            fs::write(sub.join(name).join("a.txt"), b"aaaa").unwrap();
        }
        let tree = scan(dir.path());
        assert_eq!(tree.len(), 6, "both folders and their files are in the tree");
        fs::write(sub.join(&lone).join("NEW_UNSEEN.docx"), b"precious").unwrap();
        fs::write(sub.join(&literal).join("NEW_UNSEEN2.docx"), b"precious").unwrap();
        let err = verify(&tree, sub_node(&tree), sub.clone(), false).unwrap_err();
        assert!(err.contains("cannot be told apart"), "{err}");
        // Even a partial tree (which tolerates extra entries) must not accept the pair.
        assert!(verify(&tree, sub_node(&tree), sub, true).is_err());
    }

    /// QA-140: a file rewritten next to a name twin is not hidden behind the shared key.
    #[cfg(any(windows, unix))]
    #[test]
    fn colliding_file_names_are_not_verified_as_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let (lone, literal) = colliding_names("f");
        for name in [&lone, &literal] {
            if fs::write(sub.join(name), b"1234").is_err() {
                return;
            }
        }
        let tree = scan(dir.path());
        fs::write(sub.join(&lone), vec![b'x'; 39]).unwrap();
        let err = verify(&tree, sub_node(&tree), sub, false).unwrap_err();
        assert!(err.contains("cannot be told apart"), "{err}");
    }

    /// A single non-Unicode name has no twin: it is verified (and descended) like any other.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_single_non_unicode_folder_is_still_verified() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        let (lone, _) = colliding_names("d");
        if fs::create_dir_all(sub.join(&lone)).is_err() {
            return;
        }
        fs::write(sub.join(&lone).join("a.txt"), b"aaaa").unwrap();
        let tree = scan(dir.path());
        assert_eq!(verify(&tree, sub_node(&tree), sub.clone(), false), Ok(()));
        fs::write(sub.join(&lone).join("NEW.docx"), b"new").unwrap();
        assert!(verify(&tree, sub_node(&tree), sub, false).unwrap_err().contains("NEW.docx"));
    }

    /// A literal U+FFFD name replaced by a lone-surrogate name keeps the text key but is another file.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_name_that_turns_non_unicode_is_a_difference() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        let (lone, literal) = colliding_names("f");
        fs::write(sub.join(&literal), b"1234").unwrap();
        let tree = scan(dir.path());
        fs::remove_file(sub.join(&literal)).unwrap();
        if fs::write(sub.join(&lone), b"1234").is_err() {
            return;
        }
        let err = verify(&tree, sub_node(&tree), sub, false).unwrap_err();
        assert!(err.contains("spelling of its name"), "{err}");
    }

    #[test]
    fn links_are_not_followed() {
        let (dir, sub) = fixture();
        let outside = dir.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("x.bin"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, sub.join("lnk")).unwrap();
        #[cfg(windows)]
        {
            let made = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(sub.join("lnk"))
                .arg(&outside)
                .output()
                .is_ok_and(|o| o.status.success());
            if !made {
                return;
            }
        }
        let tree = scan(dir.path());
        assert!(fs::symlink_metadata(sub.join("lnk")).is_ok_and(|m| is_link(&m)));
        // New content behind the link is not part of the folder.
        fs::write(outside.join("y.bin"), b"y").unwrap();
        assert_eq!(verify(&tree, sub_node(&tree), sub, false), Ok(()));
    }
}
