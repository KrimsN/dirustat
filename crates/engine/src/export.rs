//! Export of the tree to files (G-019): streaming CSV and JSON writers, the hook for the PNG
//! renderer of the shell, target-path validation and the atomic file write.
//!
//! Nothing here touches the session: a writer takes a `&Tree` (the commands hand over an `Arc<Tree>`
//! snapshot, so the session lock is not held while a big file is written). The walk is iterative
//! with an incrementally built path, so a 30 000-level directory chain costs the same per row as a
//! flat one. Every limit (rows, bytes, depth) is explicit and reported back as `truncated`.

use crate::ext::extension_of;
use crate::paths::{is_local_path, is_unc_path, normalize_scan_root};
use crate::protected::key_of;
use crate::{CommandError, tree_node_path};
use dirustat_model::{Children, NodeId, NodeKind, SizeMode, Tree};
use dirustat_platform::{PlatformError, PlatformServices};
use serde::Serialize;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Rows written when `max_nodes` is not given.
pub const DEFAULT_MAX_NODES: u64 = 1_000_000;
/// Largest `max_nodes` a caller may ask for.
pub const HARD_MAX_NODES: u64 = 20_000_000;
/// Output bytes after which the writer stops when `max_bytes` is not given (1 GiB).
pub const DEFAULT_MAX_BYTES: u64 = 1 << 30;
/// Largest `max_bytes` a caller may ask for (16 GiB).
pub const HARD_MAX_BYTES: u64 = 16 << 30;
/// Longest accepted target path, in bytes.
const MAX_TARGET_LEN: usize = 32_000;

/// Table formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
}

impl Format {
    /// File extension (without the dot) the target must have.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// Which nodes become rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    All,
    Files,
    Dirs,
}

/// What to export: a subtree, a depth, a row filter and the two hard limits.
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub root: NodeId,
    /// Levels below `root` that are walked (`None` = all); `root` itself is level 0.
    pub max_depth: Option<u32>,
    pub rows: RowKind,
    pub max_nodes: u64,
    /// The writer stops after the row that reaches this many output bytes.
    pub max_bytes: u64,
    /// CSV only: start the file with a UTF-8 byte order mark (helps Excel).
    pub bom: bool,
}

/// What a writer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub rows: u64,
    pub bytes: u64,
    /// `Some("max_nodes")` or `Some("max_bytes")` when the walk stopped early.
    pub truncated: Option<&'static str>,
}

/// One exported node.
#[derive(Serialize)]
struct Row<'a> {
    path: &'a str,
    name: &'a str,
    kind: &'static str,
    depth: u32,
    logical_size: u64,
    allocated_size: u64,
    file_count: u64,
    /// Seconds since the Unix epoch; `null` when unknown.
    mtime: Option<i64>,
    /// Lower-case extension without the dot; empty for directories and files without one.
    extension: &'a str,
    /// The node could not be read completely.
    error: bool,
    /// The stored name had invalid UTF-16 that was replaced with U+FFFD.
    name_lossy: bool,
}

const CSV_HEADER: &str =
    "path,name,kind,depth,logical_size,allocated_size,file_count,mtime,extension,error,name_lossy\r\n";

struct Counting<W> {
    inner: W,
    bytes: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes = self.bytes.saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Appends a path segment with exactly one separator in between.
fn push_segment(path: &mut String, name: &str) {
    if !path.is_empty() && !path.ends_with(['/', '\\']) {
        path.push(if cfg!(windows) { '\\' } else { '/' });
    }
    path.push_str(name);
}

/// Appends `text` as one CSV cell: control characters become U+FFFD, a cell a spreadsheet would read
/// as a formula (`=`, `+`, `-`, `@`, tab, CR first) gets a leading `'`, and a cell with a comma, quote,
/// line break or edge space is quoted with doubled quotes (RFC 4180).
fn csv_cell(out: &mut Vec<u8>, text: &str) {
    let clean: String = text
        .chars()
        .map(
            |c| if (u32::from(c) < 0x20 && !matches!(c, '\t' | '\n' | '\r')) || c == '\u{7f}' { '\u{fffd}' } else { c },
        )
        .collect();
    let formula = clean.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let quote = clean.contains([',', '"', '\r', '\n']) || clean.starts_with(' ') || clean.ends_with(' ');
    if quote {
        out.push(b'"');
    }
    if formula {
        out.push(b'\'');
    }
    for ch in clean.chars() {
        if ch == '"' {
            out.extend_from_slice(b"\"\"");
        } else {
            let mut buf = [0_u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    if quote {
        out.push(b'"');
    }
}

fn csv_row(out: &mut Vec<u8>, row: &Row<'_>) {
    out.clear();
    csv_cell(out, row.path);
    out.push(b',');
    csv_cell(out, row.name);
    let mtime = row.mtime.map(|m| m.to_string()).unwrap_or_default();
    let tail = format!(
        ",{},{},{},{},{},{},",
        row.kind, row.depth, row.logical_size, row.allocated_size, row.file_count, mtime
    );
    out.extend_from_slice(tail.as_bytes());
    csv_cell(out, row.extension);
    out.extend_from_slice(format!(",{},{}\r\n", row.error, row.name_lossy).as_bytes());
}

fn kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Link => "link",
    }
}

fn io_json(e: serde_json::Error) -> io::Error {
    io::Error::other(e)
}

/// Writes the selected part of `tree` to `out` as CSV or JSON, row by row.
///
/// Rows come in pre-order (a directory before its children, children in scan order). The JSON file is
/// `{"schema", "scan_root", "root", "nodes": [...], "rows", "truncated", "truncated_reason"}`, written
/// as a stream, so the totals are at the end. The writer never fails on hostile names or sizes; I/O
/// errors are returned.
pub fn write_table<W: Write>(out: W, tree: &Tree, format: Format, sel: &Selection) -> io::Result<Summary> {
    let mut out = Counting { inner: BufWriter::with_capacity(1 << 16, out), bytes: 0 };
    let start_path = tree_node_path(tree, sel.root).unwrap_or_default();
    let scan_root = tree.get(tree.root()).map_or("", |n| n.name());
    match format {
        Format::Csv => {
            if sel.bom {
                out.write_all(b"\xEF\xBB\xBF")?;
            }
            out.write_all(CSV_HEADER.as_bytes())?;
        }
        Format::Json => {
            let head = format!(
                "{{\"schema\":\"dirustat.export.v1\",\"scan_root\":{},\"root\":{},\"nodes\":[",
                serde_json::to_string(scan_root).map_err(io_json)?,
                serde_json::to_string(&start_path).map_err(io_json)?,
            );
            out.write_all(head.as_bytes())?;
        }
    }
    let mut walk = Walk { tree, sel, path: start_path, frames: Vec::new(), rows: 0, truncated: None };
    let mut line = Vec::with_capacity(512);
    let mut first = true;
    walk.run(|row| {
        if format == Format::Csv {
            csv_row(&mut line, row);
        } else {
            line.clear();
            if !first {
                line.push(b',');
            }
            serde_json::to_writer(&mut line, row).map_err(io_json)?;
        }
        first = false;
        out.write_all(&line)?;
        Ok(out.bytes < sel.max_bytes)
    })?;
    if format == Format::Json {
        let tail = format!(
            "],\"rows\":{},\"truncated\":{},\"truncated_reason\":{}}}\n",
            walk.rows,
            walk.truncated.is_some(),
            walk.truncated.map_or_else(|| "null".to_owned(), |r| format!("\"{r}\"")),
        );
        out.write_all(tail.as_bytes())?;
    }
    out.flush()?;
    Ok(Summary { rows: walk.rows, bytes: out.bytes, truncated: walk.truncated })
}

struct Walk<'a> {
    tree: &'a Tree,
    sel: &'a Selection,
    /// Path of the node being visited; frames remember the length to cut back to.
    path: String,
    frames: Vec<(Children<'a>, usize)>,
    rows: u64,
    truncated: Option<&'static str>,
}

impl<'a> Walk<'a> {
    /// Pre-order walk; `emit` returns `false` to stop (byte budget reached).
    fn run(&mut self, mut emit: impl FnMut(&Row<'_>) -> io::Result<bool>) -> io::Result<()> {
        let mut more = self.visit(self.sel.root, 0, &mut emit)?;
        while more {
            let Some((children, base)) = self.frames.last_mut() else { break };
            let base = *base;
            let Some(child) = children.next() else {
                self.path.truncate(base);
                self.frames.pop();
                continue;
            };
            self.path.truncate(base);
            let Some(name) = self.tree.get(child).map(|n| n.name()) else { continue };
            push_segment(&mut self.path, name);
            let depth = u32::try_from(self.frames.len()).unwrap_or(u32::MAX);
            more = self.visit(child, depth, &mut emit)?;
        }
        Ok(())
    }

    /// Emits the row for `id` (when it passes the filter) and opens its children (when allowed).
    /// `false` = stop the whole walk.
    fn visit(
        &mut self,
        id: NodeId,
        depth: u32,
        emit: &mut impl FnMut(&Row<'_>) -> io::Result<bool>,
    ) -> io::Result<bool> {
        let Some(node) = self.tree.get(id) else { return Ok(true) };
        let wanted = match self.sel.rows {
            RowKind::All => true,
            RowKind::Files => node.kind() != NodeKind::Dir,
            RowKind::Dirs => node.kind() == NodeKind::Dir,
        };
        if wanted {
            if self.rows >= self.sel.max_nodes {
                self.truncated = Some("max_nodes");
                return Ok(false);
            }
            let extension = if node.kind() == NodeKind::Dir { String::new() } else { extension_of(node.name()) };
            let data = node.data();
            let row = Row {
                path: &self.path,
                name: node.name(),
                kind: kind_name(node.kind()),
                depth,
                logical_size: node.total_size(SizeMode::Logical),
                allocated_size: node.total_size(SizeMode::Allocated),
                file_count: node.file_count(),
                mtime: data.mtime,
                extension: &extension,
                error: data.error,
                name_lossy: data.name_lossy,
            };
            let keep_going = emit(&row)?;
            self.rows = self.rows.saturating_add(1);
            if !keep_going {
                self.truncated = Some("max_bytes");
                return Ok(false);
            }
        }
        let descend = node.kind() == NodeKind::Dir && self.sel.max_depth.is_none_or(|limit| depth < limit);
        if descend {
            self.frames.push((self.tree.children(id), self.path.len()));
        }
        Ok(true)
    }
}

/// What the shell's PNG renderer is asked for.
#[derive(Clone, Copy, Debug)]
pub struct PngRequest {
    /// Subtree to draw.
    pub root: NodeId,
    pub width: u32,
    pub height: u32,
    pub mode: SizeMode,
    pub dark: bool,
}

/// Renders the treemap of a subtree as PNG bytes. Provided by the UI layer (it owns the colours and
/// the cushion shading); the engine only writes the bytes to disk.
pub type PngRenderer = fn(&Tree, &PngRequest) -> Result<Vec<u8>, String>;

static PNG_RENDERER: OnceLock<PngRenderer> = OnceLock::new();

/// Registers the process-wide PNG renderer; `false` when one is registered already.
pub fn register_png_renderer(renderer: PngRenderer) -> bool {
    PNG_RENDERER.set(renderer).is_ok()
}

pub(crate) fn png_renderer() -> Option<PngRenderer> {
    PNG_RENDERER.get().copied()
}

fn invalid(msg: impl Into<String>) -> CommandError {
    CommandError::InvalidParams(msg.into())
}

/// Directories nothing may be exported into (system locations), as lower-cased keys.
fn system_dirs() -> Vec<crate::protected::Key> {
    let mut dirs = Vec::new();
    let windows = cfg!(windows);
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    if windows {
        for name in ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"] {
            dirs.extend(env(name).and_then(|v| key_of(&v, true)));
        }
    } else {
        for dir in ["/etc", "/usr", "/bin", "/sbin", "/boot", "/sys", "/proc", "/dev", "/lib", "/lib64"] {
            dirs.extend(key_of(dir, false));
        }
    }
    dirs
}

/// Checks `path` as the target of an export and returns the absolute, lexically normalized path.
///
/// Rules: a local drive path (relative paths are made absolute against the working directory), no
/// network or device path, the file extension must be `extension`, the name must be valid, the
/// directory must exist (it is never created) and lie outside system locations and, unless
/// `allow_inside_scan`, outside the scanned folder. An existing target is refused unless `overwrite`,
/// and then it must be a plain file (not a directory or a link).
pub(crate) fn validate_target(
    path: &str,
    extension: &str,
    scan_root: &str,
    overwrite: bool,
    allow_inside_scan: bool,
) -> Result<PathBuf, CommandError> {
    if path.is_empty() || path.contains('\0') || path.len() > MAX_TARGET_LEN {
        return Err(invalid("path must be 1..=32000 characters without NUL"));
    }
    let absolute = std::path::absolute(Path::new(path)).map_err(|e| invalid(format!("bad path: {e}")))?;
    let text = normalize_scan_root(&absolute.to_string_lossy());
    if is_unc_path(&text) || !is_local_path(&text) {
        return Err(invalid("export targets must be on a local drive (no network or device paths)"));
    }
    let target = PathBuf::from(&text);
    if !target.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(extension)) {
        return Err(invalid(format!("the file name must end with .{extension}")));
    }
    let name = target.file_name().and_then(|n| n.to_str()).ok_or_else(|| invalid("path has no usable file name"))?;
    if let Some(problem) = crate::fsops::name_problem(name, cfg!(windows)) {
        return Err(invalid(format!("unusable file name: {problem}")));
    }
    let parent = target.parent().ok_or_else(|| invalid("path has no directory"))?;
    let real_parent = std::fs::canonicalize(parent)
        .map_err(|_| invalid(format!("the directory {} does not exist (it is not created)", parent.display())))?;
    if !real_parent.is_dir() {
        return Err(invalid(format!("{} is not a directory", parent.display())));
    }
    let places = [key_of(&text, cfg!(windows)), key_of(&real_parent.to_string_lossy(), cfg!(windows))];
    let system = system_dirs();
    if places.iter().flatten().any(|k| system.iter().any(|s| k.starts_with(s))) {
        return Err(CommandError::ProtectedPath(format!("{} is in a system location", parent.display())));
    }
    if !allow_inside_scan {
        let root_real = std::fs::canonicalize(scan_root).ok().map(|p| p.to_string_lossy().into_owned());
        let roots = [key_of(scan_root, cfg!(windows)), root_real.and_then(|r| key_of(&r, cfg!(windows)))];
        if places.iter().flatten().any(|k| roots.iter().flatten().any(|r| k.starts_with(r))) {
            return Err(invalid(
                "the target is inside the scanned folder; choose another place or pass allow_inside_scan: true",
            ));
        }
    }
    match std::fs::symlink_metadata(&target) {
        Err(_) => {}
        Ok(_) if !overwrite => {
            return Err(invalid(format!("{} exists; pass overwrite: true to replace it", target.display())));
        }
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(invalid(format!(
                "{} exists and is not a plain file; refusing to replace it",
                target.display()
            )));
        }
        Ok(_) => {}
    }
    Ok(target)
}

fn write_error(target: &Path, e: &io::Error) -> CommandError {
    CommandError::Platform(format!("cannot write {}: {e}", target.display()))
}

/// Creates a fresh uniquely named file next to `target` (never reusing an existing name).
fn create_temp(dir: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut last = None;
    for attempt in 0_u32..8 {
        let tmp = dir.join(format!(".dirustat-export-{}-{nanos}-{attempt}.tmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no temp name available")))
}

/// Writes through `fill` into a temp file next to `target` and moves it into place: a reader never
/// sees a half-written file and the temp file is removed on every failure. Without `overwrite` the
/// move refuses to replace an existing file (no-replace rename, hard link where the platform has no
/// such primitive), so a file that appeared since the validation is not clobbered either.
pub(crate) fn write_atomic<R>(
    target: &Path,
    overwrite: bool,
    platform: &dyn PlatformServices,
    fill: impl FnOnce(&mut std::fs::File) -> io::Result<R>,
) -> Result<R, CommandError> {
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let (tmp, mut file) = create_temp(dir).map_err(|e| write_error(target, &e))?;
    let cleanup = TempGuard(Some(tmp));
    let filled = fill(&mut file).and_then(|r| file.sync_all().map(|()| r));
    drop(file);
    let result = match filled {
        Ok(r) => finish_move(cleanup.path(), target, overwrite, platform).map(|()| r),
        Err(e) => Err(write_error(target, &e)),
    };
    if result.is_ok() {
        cleanup.disarm();
    }
    result
}

/// Removes the temp file on drop unless disarmed, so even a panic in `fill` leaves nothing behind.
struct TempGuard(Option<PathBuf>);

impl TempGuard {
    fn path(&self) -> &Path {
        self.0.as_deref().unwrap_or_else(|| Path::new(""))
    }

    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(tmp) = self.0.take() {
            let _ = std::fs::remove_file(tmp);
        }
    }
}

fn finish_move(
    tmp: &Path,
    target: &Path,
    overwrite: bool,
    platform: &dyn PlatformServices,
) -> Result<(), CommandError> {
    if overwrite {
        return std::fs::rename(tmp, target).map_err(|e| write_error(target, &e));
    }
    let exists = || invalid(format!("{} exists; pass overwrite: true to replace it", target.display()));
    match platform.rename_no_replace(tmp, target) {
        Ok(()) => Ok(()),
        Err(PlatformError::Unsupported(_)) => match std::fs::hard_link(tmp, target) {
            Ok(()) => {
                let _ = std::fs::remove_file(tmp);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(exists()),
            Err(e) => Err(write_error(target, &e)),
        },
        Err(_) if std::fs::symlink_metadata(target).is_ok() => Err(exists()),
        Err(e) => Err(CommandError::Platform(format!("cannot write {}: {e}", target.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_fill_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.bin");
        let platform = dirustat_platform::FakePlatform::default();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = write_atomic::<()>(&target, false, &platform, |_| panic!("boom"));
        }));
        assert!(caught.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "temp file left behind");
    }

    fn cell(text: &str) -> String {
        let mut out = Vec::new();
        csv_cell(&mut out, text);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn csv_cells_are_quoted_and_formula_safe() {
        assert_eq!(cell("plain.txt"), "plain.txt");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
        assert_eq!(cell(" lead"), "\" lead\"");
        for f in ["=1+1", "+1", "-1", "@SUM(A1)", "\tx", "\rx"] {
            assert!(cell(f).trim_matches('"').starts_with('\''), "{f:?} -> {:?}", cell(f));
        }
        assert_eq!(cell("=a,b"), "\"'=a,b\"");
        assert_eq!(cell("a\u{0}b\u{7}c"), "a\u{fffd}b\u{fffd}c");
        assert_eq!(cell("\u{65e5}\u{672c}\u{1f980}"), "\u{65e5}\u{672c}\u{1f980}");
    }

    #[test]
    fn segments_get_one_separator() {
        let sep = if cfg!(windows) { '\\' } else { '/' };
        let mut p = String::from("C:\\");
        push_segment(&mut p, "a");
        assert_eq!(p, "C:\\a");
        let mut q = String::from("root");
        push_segment(&mut q, "b");
        assert_eq!(q, format!("root{sep}b"));
    }
}
