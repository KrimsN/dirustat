//! `scan.fast_path_status`: reports whether the NTFS MFT fast path can be used for a volume and, if
//! not, why. It only probes (opens the volume read-only and closes it again); it never elevates,
//! never opens a dialog and never changes anything (ADR 0002: the app never elevates silently).

use super::{CommandSpec, always, parse_params};
use crate::{CommandError, Session};
use dirustat_ntfs::VolumeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "scan.fast_path_status",
        label: "Check fast scan availability",
        menu: "",
        shortcut: None,
        params_doc: "{\"volume\"?: string} a drive letter such as \"C:\" (default: the system drive). Returns \
            {volume, available: bool, reason: \"available\"|\"not_windows\"|\"access_denied\"|\"not_ntfs\"|\
            \"not_found\"|\"invalid_name\"|\"malformed\"|\"io_error\", needs_admin: bool, \
            preferred: bool (setting scan.prefer_fast_path)}. Read-only probe; never elevates.",
        destructive: false,
        enabled: always,
        run: run_status,
    }]
}

/// Outcome of probing a volume for the MFT fast path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FastPathStatus {
    pub volume: String,
    pub available: bool,
    /// Stable machine-readable reason (`available` when the fast path can be used).
    pub reason: &'static str,
    /// The only fix is to restart with administrator rights.
    pub needs_admin: bool,
}

impl FastPathStatus {
    fn from_result(volume: &str, result: Result<(), VolumeError>) -> Self {
        let (available, reason) = match result {
            Ok(()) => (true, "available"),
            Err(VolumeError::Unsupported) => (false, "not_windows"),
            Err(VolumeError::AccessDenied) => (false, "access_denied"),
            Err(VolumeError::NotNtfs) => (false, "not_ntfs"),
            Err(VolumeError::NotFound) => (false, "not_found"),
            Err(VolumeError::InvalidName) => (false, "invalid_name"),
            Err(VolumeError::Malformed | VolumeError::InvalidRange | VolumeError::ShortRead) => (false, "malformed"),
            Err(VolumeError::Io(_)) => (false, "io_error"),
        };
        Self { volume: volume.chars().take(64).collect(), available, reason, needs_admin: reason == "access_denied" }
    }
}

/// Probes `volume` (for example `C:`) for the fast path. Never panics.
pub fn fast_path_status(volume: &str) -> FastPathStatus {
    FastPathStatus::from_result(volume, dirustat_ntfs::probe_detailed(volume).map(|_| ()))
}

/// The drive-letter volume (`C:`) when `path` names exactly a volume root (`C:`, `c:\`, `C:/`).
pub fn volume_root_of(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    let rest: String = chars.collect();
    matches!(rest.as_str(), "" | "\\" | "/").then(|| format!("{}:", letter.to_ascii_uppercase()))
}

fn default_volume() -> String {
    std::env::var("SystemDrive").ok().and_then(|v| volume_root_of(&v)).unwrap_or_else(|| "C:".to_owned())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StatusParams {
    volume: Option<String>,
}

fn run_status(session: &mut Session, params: &Value) -> Result<Value, CommandError> {
    let p: StatusParams = if params.is_null() { StatusParams::default() } else { parse_params(params)? };
    let volume = p.volume.unwrap_or_else(default_volume);
    let status = fast_path_status(&volume);
    let mut out = serde_json::to_value(&status).map_err(|e| CommandError::Scan(e.to_string()))?;
    if let Value::Object(map) = &mut out {
        map.insert("preferred".to_owned(), Value::Bool(session.settings().prefer_fast_path));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_volume_names_are_reported_not_errors() {
        let mut s = Session::new();
        let long = "x".repeat(100_000);
        for name in ["", "not a volume", "CC:", "1:", "\\\\?\\C:", "\u{0}", "..\\..", long.as_str()] {
            let out = s.execute("scan.fast_path_status", &json!({ "volume": name })).unwrap();
            assert_eq!(out["available"], false, "{name:?}");
            let reason = out["reason"].as_str().unwrap_or_default();
            assert!(matches!(reason, "invalid_name" | "not_windows"), "{name:?}: {reason}");
            assert_eq!(out["needs_admin"], false);
            assert!(out["volume"].as_str().is_some_and(|v| v.chars().count() <= 64));
        }
    }

    #[test]
    fn default_volume_and_preference_are_reported() {
        let mut s = Session::new();
        let out = s.execute("scan.fast_path_status", &json!({})).unwrap();
        assert_eq!(out["preferred"], true);
        assert!(out["volume"].as_str().is_some_and(|v| volume_root_of(v).is_some()));
        assert_eq!(s.execute("scan.fast_path_status", &Value::Null).unwrap()["preferred"], true);
        s.execute("settings.set", &json!({"key": "scan.prefer_fast_path", "value": false})).unwrap();
        assert_eq!(s.execute("scan.fast_path_status", &json!({})).unwrap()["preferred"], false);
        let reason = out["reason"].as_str().unwrap_or_default().to_owned();
        assert_eq!(out["needs_admin"], reason == "access_denied");
        assert_eq!(out["available"], reason == "available");
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_not_windows() {
        let mut s = Session::new();
        let out = s.execute("scan.fast_path_status", &json!({"volume": "C:"})).unwrap();
        assert_eq!(out["reason"], "not_windows");
        assert_eq!(out["available"], false);
        assert_eq!(out["needs_admin"], false);
    }

    #[test]
    fn bad_params_are_invalid_params() {
        let mut s = Session::new();
        for p in [json!({"volume": 5}), json!({"x": 1}), json!([]), json!("C:")] {
            assert_eq!(s.execute("scan.fast_path_status", &p).unwrap_err().code(), "invalid_params", "{p}");
        }
    }

    #[test]
    fn every_volume_error_maps_to_a_stable_reason() {
        let cases = [
            (VolumeError::Unsupported, "not_windows", false),
            (VolumeError::AccessDenied, "access_denied", true),
            (VolumeError::NotNtfs, "not_ntfs", false),
            (VolumeError::NotFound, "not_found", false),
            (VolumeError::InvalidName, "invalid_name", false),
            (VolumeError::Malformed, "malformed", false),
            (VolumeError::InvalidRange, "malformed", false),
            (VolumeError::ShortRead, "malformed", false),
            (VolumeError::Io(5), "io_error", false),
        ];
        for (err, reason, admin) in cases {
            let st = FastPathStatus::from_result("C:", Err(err));
            assert_eq!((st.reason, st.needs_admin, st.available), (reason, admin, false));
        }
        let ok = FastPathStatus::from_result("C:", Ok(()));
        assert!(ok.available && ok.reason == "available" && !ok.needs_admin);
    }

    #[test]
    fn volume_roots_are_recognised() {
        assert_eq!(volume_root_of("c:\\").as_deref(), Some("C:"));
        assert_eq!(volume_root_of("D:").as_deref(), Some("D:"));
        assert_eq!(volume_root_of("E:/").as_deref(), Some("E:"));
        for no in ["", "C", "C:\\Users", "CC:\\", "1:\\", "\u{e9}:\\", "C:\\\\", "/"] {
            assert_eq!(volume_root_of(no), None, "{no:?}");
        }
    }
}
