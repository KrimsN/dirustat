//! `platform.*` commands: reveal in the file manager, shell properties, free space of a volume.
//!
//! None of them is destructive. `platform.reveal` and `platform.properties` are the only
//! commands that may show an OS window from a programmatic call: they launch the file manager /
//! the shell properties window (an external process or system dialog, not an app dialog), and
//! return at once. Parameters are validated by the platform layer (empty, relative, NUL and
//! over-long paths are `invalid_params`).

use super::detached::BusyGuard;
use super::{CommandSpec, always, parse_params};
use crate::{CommandError, Session};
use dirustat_platform::{PlatformError, PlatformServices};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::Duration;

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
            params_doc: "{\"path\": string} an existing absolute local drive path (X:\\...; network, device and \
                relative paths are invalid_params). Returns {path, total, free} in bytes of the volume that holds \
                it; gives up with an internal error after 3 s if the volume does not answer. At most one query per \
                session waits on the OS: while an earlier one is still stuck, a new call fails at once with an \
                internal error (\"has not returned yet\"). Hosts that share the session call it through \
                Session::execute_split, which keeps the session free while the OS is asked.",
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
        (None, Some(path)) => require_local(&path).map(|()| path),
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

/// An explicit `path` parameter must be a local lettered path (`X:\...`): a network or device
/// namespace can block on the network and would leak credentials to the named host (QA-117).
/// Paths that come from a node id were produced by a scan and are not re-checked here.
fn require_local(path: &str) -> Result<(), CommandError> {
    if crate::is_local_path(path) {
        Ok(())
    } else {
        Err(CommandError::InvalidParams(
            "path must be an absolute local drive path (X:\\...); network, device and relative paths are refused"
                .to_owned(),
        ))
    }
}

/// A validated `platform.free_space` query, ready to run without the session (QA-143).
#[derive(Debug)]
pub(super) struct FreeSpaceJob {
    platform: Arc<dyn PlatformServices>,
    target: PathBuf,
    path: String,
    busy: Arc<AtomicBool>,
    timeout: Duration,
}

/// Validates the params and captures what the OS call needs; touches no disk.
pub(super) fn begin_free_space(session: &Session, params: &Value) -> Result<FreeSpaceJob, CommandError> {
    let p: PathParams = parse_params(params)?;
    require_local(&p.path)?;
    Ok(FreeSpaceJob {
        platform: session.platform(),
        target: PathBuf::from(&p.path),
        path: p.path,
        busy: Arc::clone(&session.volume_query_busy),
        timeout: session.volume_query_timeout,
    })
}

impl FreeSpaceJob {
    /// Asks the OS on a helper thread and waits at most the timeout, so a stuck volume (a mapped
    /// network drive, a dead device) cannot hold the caller for long. At most one helper exists per
    /// session: while a previous query is still stuck, a new one is refused at once instead of
    /// leaking another thread (QA-143). The helper clears the flag when the OS finally answers.
    pub(super) fn run(self) -> Result<Value, CommandError> {
        let Some(claim) = BusyGuard::claim(&self.busy) else {
            return Err(CommandError::Platform(
                "an earlier query of a volume has not returned yet (the volume may be hung); try again later"
                    .to_owned(),
            ));
        };
        let (tx, rx) = mpsc::channel();
        let (platform, target) = (self.platform, self.target);
        std::thread::Builder::new()
            .name("dirustat-free-space".to_owned())
            .spawn(move || {
                let answer = platform.free_space(&target);
                drop(claim);
                let _ = tx.send(answer);
            })
            .map_err(|e| CommandError::Platform(format!("cannot start the volume query: {e}")))?;
        let space = rx
            .recv_timeout(self.timeout)
            .map_err(|_| CommandError::Platform("the volume did not answer in time".to_owned()))?
            .map_err(platform_error)?;
        Ok(json!({ "path": self.path, "total": space.total, "free": space.free }))
    }
}

fn run_free_space(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    begin_free_space(session, params)?.run()
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

    /// QA-117: namespaces other than a lettered local drive never reach the platform layer.
    #[test]
    fn explicit_paths_must_be_local_and_scans_refuse_devices() {
        let (_dir, mut s, fake) = session_with_fake();
        if cfg!(windows) {
            for bad in [
                r"\\?\GLOBALROOT\Device\Mup\192.0.2.1\share\x",
                r"\\server\share\x",
                r"\\?\UNC\server\share",
                r"\\.\C:",
            ] {
                for id in ["platform.reveal", "platform.properties", "platform.free_space"] {
                    let err = s.execute(id, &json!({"path": bad})).unwrap_err();
                    assert_eq!(err.code(), "invalid_params", "{id} {bad}");
                }
            }
            s.cancel_scan();
            for bad in [r"\\?\GLOBALROOT\Device\Mup\192.0.2.1\share\x", r"\\.\C:", r"\\?\Volume{1}\x"] {
                let err = s.execute("scan.start", &json!({"path": bad})).unwrap_err();
                assert_eq!(err.code(), "invalid_params", "scan.start {bad}");
                assert!(!s.is_scanning(), "{bad}");
            }
        }
        assert!(fake.calls().is_empty());
    }

    #[derive(Debug)]
    struct SlowPlatform;

    impl dirustat_platform::PlatformServices for SlowPlatform {
        fn drives(&self) -> dirustat_platform::Result<Vec<dirustat_platform::DriveInfo>> {
            Ok(Vec::new())
        }
        fn free_space(&self, _path: &Path) -> dirustat_platform::Result<SpaceInfo> {
            std::thread::sleep(Duration::from_secs(2));
            Ok(SpaceInfo { total: 1, free: 1 })
        }
        fn move_to_recycle_bin(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
        fn reveal_in_file_manager(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
        fn show_properties(&self, _path: &Path) -> dirustat_platform::Result<()> {
            Ok(())
        }
    }

    /// QA-117: a volume that never answers cannot hold the session for more than the timeout.
    #[test]
    fn free_space_gives_up_on_a_stuck_volume() {
        let (dir, mut s, _fake) = session_with_fake();
        s.set_platform(Arc::new(SlowPlatform));
        let started = std::time::Instant::now();
        let err = s.execute("platform.free_space", &json!({"path": dir.path().to_str().unwrap()})).unwrap_err();
        assert_eq!(err.code(), "internal");
        assert!(err.to_string().contains("in time"), "{err}");
        assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
    }

    #[test]
    fn commands_are_not_destructive() {
        assert!(specs().iter().all(|c| !c.destructive));
    }
}
