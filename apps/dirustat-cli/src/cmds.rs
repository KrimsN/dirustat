//! Execution of parsed subcommands against an engine [`Session`].

use crate::args::{Cli, CmdCall};
use crate::{EXIT_ERROR, EXIT_OK};
use dirustat_engine::{CommandError, Session};
use dirustat_model::{NodeKind, SizeMode};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// Interval between polls of a running scan.
const POLL: Duration = Duration::from_millis(10);
/// Interval between progress lines on stderr.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

/// A failed subcommand: message for stderr plus exit code.
struct Failure {
    code: i32,
    message: String,
}

impl Failure {
    fn command(e: &CommandError) -> Self {
        Self { code: EXIT_ERROR, message: format!("error[{}]: {e}", e.code()) }
    }
    fn runtime(message: String) -> Self {
        Self { code: EXIT_ERROR, message: format!("error: {message}") }
    }
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        Self::runtime(format!("i/o: {e}"))
    }
}

impl From<CommandError> for Failure {
    fn from(e: CommandError) -> Self {
        Self::command(&e)
    }
}

type Outcome = Result<(), Failure>;

/// MCP over stdio: a local session, or a bridge to a running app's control channel.
fn mcp(connect: Option<&str>) -> Outcome {
    let stdin = io::stdin().lock();
    let stdout = io::stdout().lock();
    let served = match connect {
        None => dirustat_control::serve_mcp(dirustat_control::share(Session::new()), stdin, stdout),
        Some(addr) => {
            let dir = dirustat_control::default_key_dir()
                .ok_or_else(|| Failure::runtime("no per-user config directory for the control key".into()))?;
            let client = dirustat_control::ControlClient::connect_with_key_dir(addr, &dir)
                .map_err(|e| Failure::runtime(format!("cannot connect to {addr}: {e}")))?;
            dirustat_control::serve_mcp_with(&client, stdin, stdout)
        }
    };
    served.map_err(|e| Failure::runtime(format!("mcp: {e}")))
}

/// Runs a parsed command line and returns the exit code.
pub fn execute(cli: Cli, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let result = match cli {
        Cli::Help => Ok(()),
        Cli::Commands { json } => list_commands(json, out),
        Cli::Scan { folder, json_out, size_mode, top } => scan(&folder, json_out.as_deref(), size_mode, top, out, err),
        Cli::Info { folder } => info(&folder, out, err),
        Cli::Run { scan, cmds, confirm } => run_cmds(scan.as_deref(), &cmds, confirm, out, err),
        Cli::Mcp { connect } => mcp(connect.as_deref()),
    };
    match result {
        Ok(()) => EXIT_OK,
        Err(f) => {
            let _ = writeln!(err, "{}", f.message);
            f.code
        }
    }
}

fn list_commands(as_json: bool, out: &mut dyn Write) -> Outcome {
    let infos = Session::new().commands();
    if as_json {
        let text = serde_json::to_string_pretty(&infos).map_err(|e| Failure::runtime(e.to_string()))?;
        writeln!(out, "{text}")?;
        return Ok(());
    }
    for c in &infos {
        let flags = if c.destructive { " [destructive]" } else { "" };
        let menu = if c.menu.is_empty() { "-" } else { c.menu };
        let shortcut = c.shortcut.unwrap_or("-");
        writeln!(out, "{}\t{}\t{}\t{}{}", c.id, c.label, menu, shortcut, flags)?;
    }
    Ok(())
}

/// Starts a scan and blocks until it is finished, printing progress lines to `err`.
fn scan_blocking(session: &mut Session, folder: &str, err: &mut dyn Write) -> Outcome {
    session.start_scan(folder)?;
    wait_for_scan(session, err)
}

fn set_size_mode(session: &mut Session, mode: SizeMode) -> Result<(), Failure> {
    let name = match mode {
        SizeMode::Logical => "logical",
        SizeMode::Allocated => "allocated",
    };
    session.execute("view.size_mode", &json!({ "mode": name }))?;
    Ok(())
}

fn kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Dir => "dir",
        NodeKind::File => "file",
        NodeKind::Link => "link",
    }
}

/// Summary of the scanned tree: totals, top-N root children and top-N extensions.
fn summary(session: &Session, mode: SizeMode, top: usize, elapsed: Duration) -> Value {
    let tree = session.tree();
    let (bytes, files, dirs, errors) = tree
        .get(tree.root())
        .map(|n| (n.total_size(mode), n.file_count(), n.dir_count(), n.error_count()))
        .unwrap_or_default();
    let mut children: Vec<(String, &'static str, u64, usize)> = tree
        .children(tree.root())
        .filter_map(|id| {
            tree.get(id).map(|n| (n.name().to_owned(), kind_name(n.kind()), n.total_size(mode), id.index()))
        })
        .collect();
    children.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    let top_children: Vec<Value> = children
        .into_iter()
        .take(top)
        .map(|(name, kind, size, id)| json!({ "id": id, "name": name, "kind": kind, "size": size }))
        .collect();
    let mut exts = session.extension_stats(mode);
    exts.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.ext.cmp(&b.ext)));
    let top_exts: Vec<Value> = exts
        .into_iter()
        .take(top)
        .map(|e| json!({ "ext": e.ext, "files": e.file_count, "size": e.size, "percent": e.percent }))
        .collect();
    json!({
        "size_mode": if mode == SizeMode::Logical { "logical" } else { "allocated" },
        "nodes": tree.len(),
        "files": files,
        "dirs": dirs,
        "bytes": bytes,
        "errors": errors,
        "elapsed_ms": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        "top_children": top_children,
        "top_extensions": top_exts,
    })
}

fn field(v: &Value, key: &str) -> u64 {
    v[key].as_u64().unwrap_or(0)
}

fn scan(
    folder: &str,
    json_out: Option<&str>,
    mode: SizeMode,
    top: usize,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Outcome {
    let mut session = Session::new();
    set_size_mode(&mut session, mode)?;
    let started = Instant::now();
    scan_blocking(&mut session, folder, err)?;
    let s = summary(&session, mode, top, started.elapsed());
    writeln!(out, "nodes:   {}", field(&s, "nodes"))?;
    writeln!(out, "files:   {}", field(&s, "files"))?;
    writeln!(out, "dirs:    {}", field(&s, "dirs"))?;
    writeln!(out, "bytes:   {}", field(&s, "bytes"))?;
    writeln!(out, "errors:  {}", field(&s, "errors"))?;
    writeln!(out, "elapsed: {} ms", field(&s, "elapsed_ms"))?;
    if let Some(path) = json_out {
        let text = serde_json::to_string_pretty(&s).map_err(|e| Failure::runtime(e.to_string()))?;
        std::fs::write(path, text).map_err(|e| Failure::runtime(format!("cannot write {path}: {e}")))?;
    }
    Ok(())
}

fn info(folder: &str, out: &mut dyn Write, err: &mut dyn Write) -> Outcome {
    let mut session = Session::new();
    scan_blocking(&mut session, folder, err)?;
    let text = serde_json::to_string_pretty(&session.state_json()).map_err(|e| Failure::runtime(e.to_string()))?;
    writeln!(out, "{text}")?;
    Ok(())
}

fn run_cmds(scan: Option<&str>, cmds: &[CmdCall], confirm: bool, out: &mut dyn Write, err: &mut dyn Write) -> Outcome {
    let mut session = Session::new();
    if let Some(folder) = scan {
        scan_blocking(&mut session, folder, err)?;
    }
    let destructive: Vec<&str> = session.commands().iter().filter(|c| c.destructive).map(|c| c.id).collect();
    for call in cmds {
        let params = with_confirm(&call.params, confirm && destructive.contains(&call.id.as_str()));
        let value = session.execute(&call.id, &params)?;
        if call.id == "scan.start" {
            wait_for_scan(&mut session, err)?;
        }
        let line = serde_json::to_string(&value).map_err(|e| Failure::runtime(e.to_string()))?;
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// Adds `confirm: true` to an object (or null) params value when requested.
fn with_confirm(params: &Value, confirm: bool) -> Value {
    if !confirm {
        return params.clone();
    }
    match params {
        Value::Null => json!({ "confirm": true }),
        Value::Object(map) => {
            let mut map = map.clone();
            map.insert("confirm".to_owned(), Value::Bool(true));
            Value::Object(map)
        }
        other => other.clone(),
    }
}

/// Blocks until the pending scan has finished, printing progress lines to `err`.
fn wait_for_scan(session: &mut Session, err: &mut dyn Write) -> Outcome {
    let mut last = Instant::now();
    while session.is_scanning() {
        if session.poll_scan()? {
            break;
        }
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            let state = session.state_json();
            let get =
                |k: &str| state.get("progress").and_then(|p| p.get(k)).and_then(serde_json::Value::as_u64).unwrap_or(0);
            writeln!(
                err,
                "scanning: {} dirs, {} files, {} bytes, {} errors",
                get("dirs"),
                get("files"),
                get("bytes"),
                get("errors")
            )?;
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}
