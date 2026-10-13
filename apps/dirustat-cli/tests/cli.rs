//! Integration tests: run the compiled binary against temporary trees.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::ffi::OsString;
use std::process::{Command, Output};

fn cli(args: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dirustat-cli")).args(args).output().expect("spawn dirustat-cli")
}

fn s(args: &[&str]) -> Output {
    let v: Vec<OsString> = args.iter().map(OsString::from).collect();
    cli(&v)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), vec![b'x'; 1000]).expect("write");
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
    std::fs::write(dir.path().join("sub").join("b.log"), vec![b'y'; 5000]).expect("write");
    dir
}

fn path(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().into_owned()
}

#[test]
fn help_exits_zero() {
    let o = s(&["--help"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stdout).contains("USAGE"));
}

#[test]
fn no_args_is_usage_error() {
    let o = s(&[]);
    assert_eq!(o.status.code(), Some(2));
    assert!(text(&o.stderr).contains("USAGE"));
}

#[test]
fn commands_lists_registry() {
    let o = s(&["commands"]);
    assert_eq!(o.status.code(), Some(0));
    let t = text(&o.stdout);
    assert!(t.contains("scan.start") && t.contains("session.state"));
    let o = s(&["commands", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    let arr = v.as_array().expect("array");
    assert!(arr.iter().any(|c| c["id"] == "view.size_mode" && c["destructive"] == false));
}

#[test]
fn scan_prints_summary_and_exports_json() {
    let dir = fixture();
    let out = tempfile::tempdir().expect("tempdir");
    let json = out.path().join("o.json");
    let o = s(&["scan", &path(&dir), "--json", &json.to_string_lossy(), "--size-mode", "logical", "--top", "1"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let t = text(&o.stdout);
    assert!(t.contains("nodes:") && t.contains("bytes:   6000"), "{t}");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json).expect("read")).expect("json");
    assert_eq!(v["bytes"], 6000);
    assert_eq!(v["files"], 2);
    assert_eq!(v["top_children"].as_array().expect("arr").len(), 1);
    assert_eq!(v["top_children"][0]["name"], "sub");
    assert_eq!(v["top_extensions"][0]["ext"], "log");
}

#[test]
fn scan_missing_folder_fails_with_code() {
    let dir = fixture();
    let missing = dir.path().join("missing");
    let o = s(&["scan", &missing.to_string_lossy()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("invalid_params"));
}

#[test]
fn info_prints_state() {
    let dir = fixture();
    let o = s(&["info", &path(&dir)]);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(v["scanning"], false);
    assert!(v["nodes"].as_u64().expect("nodes") >= 4);
}

#[test]
fn run_select_and_view_commands() {
    let dir = fixture();
    let o = s(&[
        "run",
        "--scan",
        &path(&dir),
        "--cmd",
        "view.size_mode={\"mode\":\"logical\"}",
        "--cmd",
        "select.node={\"id\":1}",
        "--cmd",
        "session.state",
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let t = text(&o.stdout);
    let lines: Vec<&str> = t.lines().collect();
    assert_eq!(lines.len(), 3, "{t}");
    let last: serde_json::Value = serde_json::from_str(lines.last().expect("line")).expect("json");
    assert_eq!(last["size_mode"], "logical");
    assert_eq!(last["selection"], 1);
}

#[test]
fn run_scan_start_waits_for_the_scan() {
    let dir = fixture();
    let cmd = format!("scan.start={}", serde_json::json!({ "path": path(&dir) }));
    let o = s(&["run", "--cmd", &cmd, "--cmd", "session.state"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let t = text(&o.stdout);
    let last: serde_json::Value = serde_json::from_str(t.lines().last().expect("line")).expect("json");
    assert!(last["nodes"].as_u64().expect("nodes") >= 4);
    assert_eq!(last["scanning"], false);
}

#[test]
fn run_unknown_command_exits_one_with_stable_code() {
    let o = s(&["run", "--cmd", "nope.nothing"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("unknown_command"));
}

#[test]
fn run_stops_at_first_error() {
    let o = s(&["run", "--cmd", "session.state", "--cmd", "nope.x", "--cmd", "session.state"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(text(&o.stdout).lines().count(), 1);
}

#[test]
fn run_invalid_params_code() {
    let o = s(&["run", "--cmd", "select.node={\"id\":1,\"bogus\":2}"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o.stderr).contains("invalid_params"));
}

#[test]
fn bad_args_are_usage_errors() {
    for args in [
        vec!["bogus"],
        vec!["scan"],
        vec!["scan", "a", "--top", "x"],
        vec!["scan", "a", "--size-mode", "huge"],
        vec!["run"],
        vec!["run", "--cmd", "a.b={bad"],
        vec!["info"],
        vec!["commands", "--nope"],
        vec![""],
    ] {
        let o = s(&args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn hostile_args() {
    let huge = "x".repeat(10_000);
    let o = s(&["run", "--cmd", &huge]);
    assert_eq!(o.status.code(), Some(1));
    let o = s(&["scan", &huge]);
    assert_eq!(o.status.code(), Some(1));
    let o = s(&["scan", "a", "--top", &huge]);
    assert_eq!(o.status.code(), Some(2));
    let o = s(&["run", "--cmd", &format!("session.state={{\"a\":\"{huge}\"}}")]);
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn invalid_utf8_arg_is_usage_error() {
    #[cfg(windows)]
    let bad = {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x66, 0xd800, 0x6f])
    };
    #[cfg(unix)]
    let bad = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![0x66, 0xff, 0x6f])
    };
    let o = cli(&[OsString::from("scan"), bad]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn mcp_rejects_bad_args() {
    assert_eq!(s(&["mcp", "x"]).status.code(), Some(2));
    assert_eq!(s(&["mcp", "--connect="]).status.code(), Some(2));
}

#[test]
fn mcp_connect_to_closed_port_is_a_runtime_error() {
    let o = s(&["mcp", "--connect", "127.0.0.1:1"]);
    assert_eq!(o.status.code(), Some(1));
}
