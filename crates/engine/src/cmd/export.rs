//! `export.*` commands (G-019): the tree as CSV or JSON, the treemap as PNG.
//!
//! All three take an explicit target `path`; a programmatic call never opens a dialog (the shell's
//! File > Export entries pick the path with a native dialog and then call these). The checks run
//! under the session lock ([`begin_table`], [`begin_png`]); the slow part (walking, rendering,
//! writing) is an [`ExportJob`] that owns an `Arc<Tree>` snapshot and runs without the lock, see
//! `detached`. Files are written atomically (temp file + rename) and an existing file is only
//! replaced with `overwrite: true`.

use super::select::node_by_index;
use super::{CommandSpec, parse_params};
use crate::export::{
    self, DEFAULT_MAX_BYTES, DEFAULT_MAX_NODES, Format, HARD_MAX_BYTES, HARD_MAX_NODES, PngRenderer, PngRequest,
    RowKind, Selection,
};
use crate::{CommandError, Session, SizeModeName, guard};
use dirustat_model::{NodeId, Tree};
use dirustat_platform::PlatformServices;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Default and limits of the PNG size, in pixels.
const DEFAULT_WIDTH: u32 = 1600;
const DEFAULT_HEIGHT: u32 = 1000;
const MIN_SIDE: u32 = 16;
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 16_000_000;
/// Largest PNG the engine accepts from the renderer.
const MAX_PNG_BYTES: usize = 256 << 20;

/// An export is possible once a tree with content exists.
fn has_tree(session: &Session) -> bool {
    session.tree().len() > 1
}

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "export.csv",
            label: "Export CSV",
            menu: "",
            shortcut: None,
            params_doc: "{\"path\": string (the .csv file to write; a local drive, in an existing directory), \
                \"overwrite\"?: bool (default false: an existing file is refused), \"allow_inside_scan\"?: bool \
                (default false: a target inside the scanned folder is refused), \"node\"?: u64 (subtree root, \
                default the scan root), \"revision\"?: u64 (the revision \"node\" was read at; any other current \
                revision is stale_tree and nothing is written), \"max_depth\"?: u32 (levels below the root, default all), \"kind\"?: \
                \"all\" (default) | \"files\" (files and links) | \"dirs\", \"max_nodes\"?: 1..=20000000 (default \
                1000000), \"max_bytes\"?: 1..=17179869184 (default 1073741824; the writer stops after the row \
                that reaches it), \"bom\"?: bool (UTF-8 byte order mark for Excel, default false)}. Writes \
                path,name,kind,depth,logical_size,allocated_size,file_count,mtime (Unix seconds, empty when \
                unknown),extension,error,name_lossy as RFC 4180 CSV (CRLF) in pre-order. Text cells that start with \
                = + - @ tab or CR get a leading ' (spreadsheet formula guard); control characters become \
                U+FFFD. Needs a scanned tree. Returns {path, format, rows, bytes, truncated, truncated_reason, \
                revision}; the file is written atomically.",
            destructive: false,
            enabled: has_tree,
            run: run_csv,
        },
        CommandSpec {
            id: "export.json",
            label: "Export JSON",
            menu: "",
            shortcut: None,
            params_doc: "Same params as export.csv without \"bom\" (the file must end in .json). Writes one JSON \
                object {schema: \"dirustat.export.v1\", scan_root, root, nodes: [{path, name, kind, depth, \
                logical_size, allocated_size, file_count, mtime (Unix seconds or null), extension, error, \
                name_lossy}], rows, truncated, truncated_reason}; names with invalid UTF-16 carry U+FFFD and \
                name_lossy: true. Needs a scanned tree. Returns the same object as export.csv.",
            destructive: false,
            enabled: has_tree,
            run: run_json,
        },
        CommandSpec {
            id: "export.png",
            label: "Export treemap PNG",
            menu: "",
            shortcut: None,
            params_doc: "{\"path\": string (the .png file to write), \"overwrite\"?: bool, \"allow_inside_scan\"?: \
                bool (as export.csv), \"node\"?: u64 (subtree to draw, default the scan root), \"revision\"?: u64 (as export.csv), \"width\"?: \
                16..=8192 (default 1600), \"height\"?: 16..=8192 (default 1000; width x height <= 16000000), \
                \"theme\"?: \"dark\" (default) | \"light\", \"size_mode\"?: \"logical\" | \"allocated\" (default: \
                session mode)}. Renders a fresh cushion-shaded treemap of that size (not a window capture). \
                Needs the desktop app (headless hosts answer `unsupported`). Returns {path, format, width, \
                height, bytes, revision}.",
            destructive: false,
            enabled: has_tree,
            run: run_png,
        },
    ]
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum KindName {
    All,
    Files,
    Dirs,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TableParams {
    path: String,
    overwrite: Option<bool>,
    allow_inside_scan: Option<bool>,
    node: Option<u64>,
    revision: Option<u64>,
    max_depth: Option<u32>,
    kind: Option<KindName>,
    max_nodes: Option<u64>,
    max_bytes: Option<u64>,
    bom: Option<bool>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ThemeName {
    Dark,
    Light,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PngParams {
    path: String,
    overwrite: Option<bool>,
    allow_inside_scan: Option<bool>,
    node: Option<u64>,
    revision: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    theme: Option<ThemeName>,
    size_mode: Option<SizeModeName>,
}

/// What an [`ExportJob`] produces.
#[derive(Debug)]
enum Work {
    Table { format: Format, sel: Selection },
    Png { renderer: PngRenderer, request: PngRequest },
}

/// A checked export, ready to run without the session.
#[derive(Debug)]
pub(super) struct ExportJob {
    tree: Arc<Tree>,
    revision: u64,
    target: PathBuf,
    overwrite: bool,
    platform: Arc<dyn PlatformServices>,
    work: Work,
}

fn resolve_root(session: &Session, node: Option<u64>, revision: Option<u64>) -> Result<NodeId, CommandError> {
    if let Some(rev) = revision
        && rev != session.revision()
    {
        return Err(CommandError::StaleTree(format!(
            "node ids were read at revision {rev} but the tree is at revision {}; re-read the ids",
            session.revision()
        )));
    }
    match node {
        None => Ok(session.tree().root()),
        Some(index) => node_by_index(session.tree(), index)
            .ok_or_else(|| CommandError::InvalidParams(format!("no node with id {index}"))),
    }
}

fn limit(name: &str, value: Option<u64>, default: u64, max: u64) -> Result<u64, CommandError> {
    match value {
        None => Ok(default),
        Some(v) if (1..=max).contains(&v) => Ok(v),
        Some(_) => Err(CommandError::InvalidParams(format!("{name} must be 1..={max}"))),
    }
}

fn scan_root_name(session: &Session) -> String {
    let tree = session.tree();
    tree.get(tree.root()).map(|n| n.name().to_owned()).unwrap_or_default()
}

fn job(session: &Session, target: PathBuf, overwrite: bool, work: Work) -> ExportJob {
    ExportJob {
        tree: session.tree_arc(),
        revision: session.revision(),
        target,
        overwrite,
        platform: session.platform(),
        work,
    }
}

/// Validates an `export.csv` / `export.json` call and captures what the write needs.
pub(super) fn begin_table(session: &Session, params: &Value, format: Format) -> Result<ExportJob, CommandError> {
    let p: TableParams = parse_params(params)?;
    if p.bom.is_some() && format != Format::Csv {
        return Err(CommandError::InvalidParams("bom applies to CSV only".to_owned()));
    }
    let root = resolve_root(session, p.node, p.revision)?;
    let sel = Selection {
        root,
        max_depth: p.max_depth,
        rows: match p.kind.unwrap_or(KindName::All) {
            KindName::All => RowKind::All,
            KindName::Files => RowKind::Files,
            KindName::Dirs => RowKind::Dirs,
        },
        max_nodes: limit("max_nodes", p.max_nodes, DEFAULT_MAX_NODES, HARD_MAX_NODES)?,
        max_bytes: limit("max_bytes", p.max_bytes, DEFAULT_MAX_BYTES, HARD_MAX_BYTES)?,
        bom: p.bom.unwrap_or(false),
    };
    let overwrite = p.overwrite.unwrap_or(false);
    let target = export::validate_target(
        &p.path,
        format.extension(),
        &scan_root_name(session),
        overwrite,
        p.allow_inside_scan.unwrap_or(false),
    )?;
    Ok(job(session, target, overwrite, Work::Table { format, sel }))
}

/// Validates an `export.png` call and captures what the render and write need.
pub(super) fn begin_png(session: &Session, params: &Value) -> Result<ExportJob, CommandError> {
    let p: PngParams = parse_params(params)?;
    let renderer = export::png_renderer()
        .ok_or_else(|| CommandError::Unsupported("export.png needs the desktop app".to_owned()))?;
    let (width, height) = (p.width.unwrap_or(DEFAULT_WIDTH), p.height.unwrap_or(DEFAULT_HEIGHT));
    let sides = MIN_SIDE..=MAX_SIDE;
    if !sides.contains(&width) || !sides.contains(&height) || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(CommandError::InvalidParams(format!(
            "width and height must be {MIN_SIDE}..={MAX_SIDE} with at most {MAX_PIXELS} pixels"
        )));
    }
    let request = PngRequest {
        root: resolve_root(session, p.node, p.revision)?,
        width,
        height,
        mode: p.size_mode.map_or_else(|| session.map_size_mode(), SizeModeName::mode),
        dark: !matches!(p.theme, Some(ThemeName::Light)),
    };
    let overwrite = p.overwrite.unwrap_or(false);
    let target = export::validate_target(
        &p.path,
        "png",
        &scan_root_name(session),
        overwrite,
        p.allow_inside_scan.unwrap_or(false),
    )?;
    Ok(job(session, target, overwrite, Work::Png { renderer, request }))
}

impl ExportJob {
    /// Walks or renders and writes the file. Needs no session; a panic becomes an error.
    pub(super) fn run(self) -> Result<Value, CommandError> {
        guard::guarded("export", || self.run_inner())
    }

    fn run_inner(self) -> Result<Value, CommandError> {
        let Self { tree, revision, target, overwrite, platform, work } = self;
        let path = target.to_string_lossy().into_owned();
        match work {
            Work::Table { format, sel } => {
                let summary = export::write_atomic(&target, overwrite, platform.as_ref(), |file| {
                    export::write_table(file, &tree, format, &sel)
                })?;
                Ok(json!({
                    "path": path,
                    "format": format.extension(),
                    "rows": summary.rows,
                    "bytes": summary.bytes,
                    "truncated": summary.truncated.is_some(),
                    "truncated_reason": summary.truncated,
                    "revision": revision,
                }))
            }
            Work::Png { renderer, request } => {
                let bytes = renderer(&tree, &request).map_err(CommandError::Platform)?;
                if bytes.is_empty() || bytes.len() > MAX_PNG_BYTES {
                    return Err(CommandError::Platform("the renderer produced an unusable image".to_owned()));
                }
                export::write_atomic(&target, overwrite, platform.as_ref(), |file| {
                    use std::io::Write;
                    file.write_all(&bytes)
                })?;
                Ok(json!({
                    "path": path,
                    "format": "png",
                    "width": request.width,
                    "height": request.height,
                    "bytes": bytes.len(),
                    "revision": revision,
                }))
            }
        }
    }
}

fn run_csv(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    begin_table(session, params, Format::Csv)?.run()
}

fn run_json(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    begin_table(session, params, Format::Json)?.run()
}

fn run_png(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    begin_png(session, params)?.run()
}

#[cfg(test)]
mod tests;
