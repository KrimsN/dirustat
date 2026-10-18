//! `platform.*` commands: reveal in the file manager, shell properties, free space of a volume.
//!
//! None of them is destructive. `platform.reveal` and `platform.properties` are the only
//! commands that may show an OS window from a programmatic call: they launch the file manager /
//! the shell properties window (an external process or system dialog, not an app dialog), and
//! return at once. Parameters are validated by the platform layer (empty, relative, NUL and
//! over-long paths are `invalid_params`).

use super::{CommandSpec, always, parse_params};
use crate::{CommandError, Session};
use dirustat_platform::PlatformError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "platform.reveal",
            label: "Show in file manager",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": u64} arena index of a node, or {\"path\": string} an absolute path (exactly one). \
                Opens the OS file manager with the item selected; launches an external window, returns at once. \
                Returns {path}.",
            destructive: false,
            enabled: always,
            run: run_reveal,
        },
        CommandSpec {
            id: "platform.properties",
            label: "Properties",
            menu: "",
            shortcut: None,
            params_doc: "{\"id\": u64} arena index of a node, or {\"path\": string} an absolute path (exactly one). \
                Opens the OS properties window of the item; launches a system window, returns at once. \
                Returns {path}.",
            destructive: false,
            enabled: always,
            run: run_properties,
        },
        CommandSpec {
            id: "platform.free_space",
            label: "Free space",
            menu: "",
            shortcut: None,
            params_doc: "{\"path\": string} an existing absolute path. Returns {path, total, free} in bytes of \
                the volume that holds it.",
            destructive: false,
            enabled: always,
            run: run_free_space,
        },
    ]
}

/// Maps a platform failure to the command error clients see: bad input is the caller's mistake
/// (`invalid_params`), a missing service is `unsupported`, anything else is `internal`.
pub(crate) fn platform_error(err: PlatformError) -> CommandError {
    match err {
        PlatformError::Unsupported(what) => CommandError::Unsupported(what.to_owned()),
        PlatformError::NotFound(_) | PlatformError::InvalidPath(_) => CommandError::InvalidParams(err.to_string()),
        PlatformError::PermissionDenied(_) | PlatformError::Failed { .. } => CommandError::Platform(err.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetParams {
    id: Option<u64>,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathParams {
    path: String,
}

/// Resolves `{id | path}` (exactly one) to a path string; an id goes through the tree.
fn resolve_target(session: &Session, params: &Value) -> Result<String, CommandError> {
    let p: TargetParams = parse_params(params)?;
    match (p.id, p.path) {
        (Some(index), None) => {
            let node = super::select::node_by_index(session.tree(), index)
                .ok_or_else(|| CommandError::InvalidParams(format!("no node with id {index}")))?;
            session.node_path(node).ok_or_else(|| CommandError::InvalidParams("node has no path".to_owned()))
        }
        (None, Some(path)) => Ok(path),
        _ => Err(CommandError::InvalidParams("give exactly one of `id` or `path`".to_owned())),
    }
}

fn run_reveal(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let path = resolve_target(session, params)?;
    session.platform().reveal_in_file_manager(Path::new(&path)).map_err(platform_error)?;
    Ok(json!({ "path": path }))
}

fn run_properties(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let path = resolve_target(session, params)?;
    session.platform().show_properties(Path::new(&path)).map_err(platform_error)?;
    Ok(json!({ "path": path }))
}

fn run_free_space(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: PathParams = parse_params(params)?;
    let space = session.platform().free_space(&PathBuf::from(&p.path)).map_err(platform_error)?;
    Ok(json!({ "path": p.path, "total": space.total, "free": space.free }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dirustat_platform::{FakePlatform, PlatformCall, SpaceInfo};
    use std::sync::Arc;

    fn session_with_fake() -> (tempfile::TempDir, Session, Arc<FakePlatform>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let mut s = Session::new();
        let handle = dirustat_scan::start(dir.path(), dirustat_scan::ScanOptions::default()).unwrap();
        assert_eq!(s.finish_scan(handle), Ok(true));
        let fake = Arc::new(FakePlatform::new());
        s.set_platform(fake.clone());
        (dir, s, fake)
    }

    #[test]
    fn reveal_and_properties_by_id_and_by_path() {
        let (dir, mut s, fake) = session_with_fake();
        let file = s.largest_files(1, 0).first().copied().unwrap();
        let file_path = s.node_path(file).unwrap();
        let out = s.execute("platform.reveal", &json!({"id": file.index()})).unwrap();
        assert_eq!(out["path"], file_path.as_str());
        s.execute("platform.properties", &json!({"path": dir.path().to_str().unwrap()})).unwrap();
        assert_eq!(
            fake.calls(),
            vec![PlatformCall::Reveal(PathBuf::from(&file_path)), PlatformCall::Properties(dir.path().to_path_buf()),]
        );
    }

    #[test]
    fn free_space_returns_numbers() {
        let (dir, mut s, fake) = session_with_fake();
        fake.set_space(SpaceInfo { total: 1000, free: 250 });
        let out = s.execute("platform.free_space", &json!({"path": dir.path().to_str().unwrap()})).unwrap();
        assert_eq!((out["total"].as_u64(), out["free"].as_u64()), (Some(1000), Some(250)));
    }

    #[test]
    fn hostile_params_are_invalid_params_and_never_reach_the_os() {
        let (_dir, mut s, fake) = session_with_fake();
        let bad = [
            json!({}),
            json!({"id": 0, "path": "C:\\x"}),
            json!({"id": u64::MAX}),
            json!({"id": -1}),
            json!({"path": ""}),
            json!({"path": "relative/x"}),
            json!({"path": "a\u{0}b"}),
            json!({"path": "x".repeat(100_000)}),
            json!({"path": 5}),
            json!({"bogus": 1}),
        ];
        for id in ["platform.reveal", "platform.properties", "platform.free_space"] {
            for p in &bad {
                let err = s.execute(id, p).unwrap_err();
                assert_eq!(err.code(), "invalid_params", "{id} {p}");
            }
        }
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn platform_failures_map_to_codes() {
        let (dir, mut s, fake) = session_with_fake();
        let params = json!({"path": dir.path().to_str().unwrap()});
        for (err, code) in [
            (PlatformError::Unsupported("x"), "unsupported"),
            (PlatformError::NotFound("x".into()), "invalid_params"),
            (PlatformError::PermissionDenied("x".into()), "internal"),
            (PlatformError::Failed { op: "x", message: "y".into() }, "internal"),
        ] {
            fake.set_failure(Some(err));
            assert_eq!(s.execute("platform.reveal", &params).unwrap_err().code(), code);
        }
    }

    #[test]
    fn commands_are_not_destructive() {
        assert!(specs().iter().all(|c| !c.destructive));
    }
}
