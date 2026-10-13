//! `scan.*` commands: background scans owned by the session.

use super::{CommandSpec, no_params, parse_params};
use crate::{CommandError, PendingScan, Session};
use dirustat_scan::{ScanOptions, start_deferred};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "scan.start",
            label: "Scan folder",
            menu: "File/Scan folder",
            shortcut: Some("Ctrl+O"),
            params_doc: "{\"path\": string} folder to scan. Returns at once; read progress with session.state and finish with session.poll.",
            destructive: false,
            enabled: |s| !s.is_scanning(),
            run: run_start,
        },
        CommandSpec {
            id: "scan.cancel",
            label: "Cancel scan",
            menu: "File/Cancel scan",
            shortcut: None,
            params_doc: "none. Asks the running scan to stop; session.poll then drops the partial tree.",
            destructive: false,
            enabled: Session::is_scanning,
            run: run_cancel,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    path: String,
}

fn run_start(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: StartParams = parse_params(params)?;
    session.start_scan(&p.path)?;
    Ok(json!({ "scanning": true, "path": p.path }))
}

fn run_cancel(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    no_params(params)?;
    session.cancel_scan();
    Ok(json!({ "cancelling": true }))
}

impl Session {
    /// True while a scan started by `scan.start` has not been joined by [`Session::poll_scan`].
    pub fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// Starts a background scan of `path`; the tree is replaced when [`Session::poll_scan`]
    /// observes completion.
    pub fn start_scan(&mut self, path: &str) -> Result<(), CommandError> {
        if self.scan.is_some() {
            return Err(CommandError::NotEnabled("scan.start".to_owned()));
        }
        if path.is_empty() {
            return Err(CommandError::InvalidParams("path is empty".to_owned()));
        }
        // The root is checked on the scan thread: `metadata` on a dead network path can block for
        // a long time and this runs under the session lock. A bad root surfaces when the scan is
        // polled (`session.poll` / `session.state`) as a scan error, and the session stays usable.
        // A whole-volume root tries the MFT fast path first and falls back to the portable walk on its own.
        let opts = ScanOptions { prefer_fast_path: self.settings.prefer_fast_path, ..ScanOptions::default() };
        let handle = start_deferred(Path::new(path), opts)
            .map_err(|e| CommandError::InvalidParams(format!("cannot scan {path:?}: {e}")))?;
        self.scan = Some(PendingScan { handle, root: path.to_owned() });
        Ok(())
    }

    /// Asks the running scan (if any) to stop. The partial tree is discarded by `poll_scan`.
    pub fn cancel_scan(&mut self) {
        if let Some(p) = &self.scan {
            p.handle.cancel();
        }
    }

    /// Non-blocking: if the pending scan has finished, makes its tree current (unless cancelled).
    /// Returns whether the tree was replaced.
    pub fn poll_scan(&mut self) -> Result<bool, CommandError> {
        if !self.scan.as_ref().is_some_and(|p| p.handle.is_finished()) {
            return Ok(false);
        }
        let Some(pending) = self.scan.take() else { return Ok(false) };
        self.finish_scan(pending.handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn poll_until_done(s: &mut Session) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let v = s.execute("session.poll", &json!({})).unwrap();
            if v["scanning"] == false {
                return v["replaced"] == true;
            }
            assert!(Instant::now() < deadline, "scan did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn start_poll_replaces_tree() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let mut s = Session::new();
        let path = dir.path().to_string_lossy().into_owned();
        let v = s.execute("scan.start", &json!({"path": path})).unwrap();
        assert_eq!(v["scanning"], true);
        assert!(poll_until_done(&mut s));
        let st = s.state_json();
        assert_eq!(st["scanning"], false);
        assert_eq!(st["revision"], 1);
        assert!(st["nodes"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn state_reports_the_backend_of_the_last_scan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        for prefer in [true, false] {
            let mut s = Session::new();
            s.execute("settings.set", &json!({"key": "scan.prefer_fast_path", "value": prefer})).unwrap();
            assert!(s.state_json()["last_scan"].is_null());
            s.execute("scan.start", &json!({"path": path})).unwrap();
            let running = s.state_json();
            if running["scanning"] == true {
                assert!(running["progress"]["backend"].is_string());
            }
            assert!(poll_until_done(&mut s));
            // A folder is never a volume root, so the portable walk runs and nothing fell back.
            let last = &s.state_json()["last_scan"];
            assert_eq!(last["backend"], "portable");
            assert!(last["fallback_reason"].is_null());
        }
    }

    #[test]
    fn second_start_while_scanning_is_not_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Session::new();
        let path = dir.path().to_string_lossy().into_owned();
        s.execute("scan.start", &json!({"path": path})).unwrap();
        let err = s.execute("scan.start", &json!({"path": path})).unwrap_err();
        assert_eq!(err.code(), "not_enabled");
        poll_until_done(&mut s);
    }

    #[test]
    fn cancel_ends_the_pending_scan() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Session::new();
        let path = dir.path().to_string_lossy().into_owned();
        s.execute("scan.start", &json!({"path": path})).unwrap();
        s.execute("scan.cancel", &json!({})).unwrap();
        poll_until_done(&mut s);
        assert!(!s.is_scanning());
    }

    #[test]
    fn bad_paths_are_invalid_params() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        let missing = dir.path().join("missing");
        let mut s = Session::new();
        let err = s.execute("scan.start", &json!({"path": ""})).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        assert!(!s.is_scanning());
        // Missing and non-directory roots are found by the scan thread: `scan.start` returns at once
        // and the error is reported by polling, after which the session is idle and unchanged.
        for path in [file.to_string_lossy().into_owned(), missing.to_string_lossy().into_owned()] {
            assert!(s.execute("scan.start", &json!({"path": path})).is_ok(), "{path}");
            let deadline = Instant::now() + Duration::from_secs(30);
            let err = loop {
                match s.execute("session.poll", &json!({})) {
                    Err(e) => break e,
                    Ok(_) => {
                        assert!(Instant::now() < deadline, "bad root was never reported");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
            };
            assert_eq!(err.code(), "invalid_params", "{path}");
            assert!(!s.is_scanning());
            assert_eq!(s.state_json()["nodes"], 1);
        }
        assert_eq!(s.execute("scan.start", &json!({})).unwrap_err().code(), "invalid_params");
        assert_eq!(s.execute("scan.cancel", &json!({})).unwrap_err().code(), "not_enabled");
    }
}
