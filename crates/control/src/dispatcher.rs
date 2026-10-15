//! Transport-agnostic request handling: one JSON request line in, one JSON response line out.

use crate::bridge::{NullBridge, Region, ScreenshotRequest, UiBridge};
use crate::error::{self, ControlError};
use crate::tree::session_tree;
use dirustat_engine::{Session, is_unc_path};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The session shared between the UI, the control server and MCP.
pub type SharedSession = Arc<Mutex<Session>>;

/// Wraps a session for sharing.
pub fn share(session: Session) -> SharedSession {
    Arc::new(Mutex::new(session))
}

/// Locks the session, recovering from poisoning (a panicked handler must not take everything down).
pub fn lock_session(session: &SharedSession) -> MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Longest accepted request line, in bytes.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Largest response line; bigger results become a `too_large` error.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;

/// Something that answers protocol methods: the in-process [`Dispatcher`] or a remote
/// [`crate::ControlClient`]. MCP is written against this trait.
pub trait Backend {
    fn call(&self, method: &str, params: &Value) -> Result<Value, ControlError>;
}

/// Handles control requests against a shared session and a [`UiBridge`].
#[derive(Clone)]
pub struct Dispatcher {
    session: SharedSession,
    bridge: Arc<dyn UiBridge>,
    key: Option<String>,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").field("key_required", &self.key.is_some()).finish_non_exhaustive()
    }
}

impl Dispatcher {
    /// Dispatcher without key checking (in-process use, e.g. MCP over stdio).
    pub fn new(session: SharedSession, bridge: Arc<dyn UiBridge>) -> Self {
        Self { session, bridge, key: None }
    }

    /// Headless dispatcher: `ui.*` and `app.quit` answer `unsupported`.
    pub fn headless(session: SharedSession) -> Self {
        Self::new(session, Arc::new(NullBridge))
    }

    /// Requires every request to carry `key` (compared in constant time).
    #[must_use]
    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    pub fn session(&self) -> &SharedSession {
        &self.session
    }

    /// Handles one request line and returns one response line (no trailing newline). Never panics.
    pub fn handle_line(&self, line: &str) -> String {
        self.handle_line_checked(line).0
    }

    /// Like [`Self::handle_line`]; the flag tells whether the request passed the key check (always
    /// true for a dispatcher without a key). The server uses it to end the pre-auth phase.
    pub fn handle_line_checked(&self, line: &str) -> (String, bool) {
        let (response, authorized) = if line.len() > MAX_REQUEST_BYTES {
            (failure(&Value::Null, &ControlError::new(error::TOO_LARGE, "request exceeds the size limit")), false)
        } else {
            match serde_json::from_str::<Value>(line) {
                Ok(request) => self.handle_value_checked(&request),
                Err(e) => (
                    failure(&Value::Null, &ControlError::new(error::BAD_REQUEST, format!("malformed JSON: {e}"))),
                    false,
                ),
            }
        };
        (serialize_bounded(&response), authorized)
    }

    /// Handles one parsed request and returns the response object.
    pub fn handle_value(&self, request: &Value) -> Value {
        self.handle_value_checked(request).0
    }

    fn handle_value_checked(&self, request: &Value) -> (Value, bool) {
        let Some(obj) = request.as_object() else {
            let err = ControlError::new(error::BAD_REQUEST, "request must be a JSON object");
            return (failure(&Value::Null, &err), false);
        };
        let id = match obj.get("id") {
            None | Some(Value::Null) => Value::Null,
            Some(v @ (Value::Number(_) | Value::String(_))) => v.clone(),
            Some(_) => {
                let err = ControlError::new(error::BAD_REQUEST, "id must be a number or string");
                return (failure(&Value::Null, &err), false);
            }
        };
        if let Some(expected) = &self.key {
            let given = obj.get("key").and_then(Value::as_str).unwrap_or("");
            if !constant_time_eq(expected.as_bytes(), given.as_bytes()) {
                return (failure(&id, &ControlError::new(error::UNAUTHORIZED, "missing or wrong key")), false);
            }
        }
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return (failure(&id, &ControlError::new(error::BAD_REQUEST, "method must be a string")), true);
        };
        let empty = Value::Object(Map::new());
        let params = match obj.get("params") {
            None | Some(Value::Null) => &empty,
            Some(p) => p,
        };
        match self.call_guarded(method, params) {
            Ok(result) => (json!({ "id": id, "ok": true, "result": result }), true),
            Err(err) => (failure(&id, &err), true),
        }
    }

    fn call_guarded(&self, method: &str, params: &Value) -> Result<Value, ControlError> {
        match panic::catch_unwind(AssertUnwindSafe(|| self.dispatch(method, params))) {
            Ok(result) => result,
            Err(_) => Err(ControlError::internal(format!("handler for {method} panicked"))),
        }
    }

    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, ControlError> {
        match method {
            "engine.commands" => {
                expect_empty(params)?;
                let infos = lock_session(&self.session).commands();
                serde_json::to_value(infos).map_err(|e| ControlError::internal(e.to_string()))
            }
            "engine.execute" => self.engine_execute(params),
            "session.state" => self.session_state(params),
            "session.tree" => {
                let mut session = lock_session(&self.session);
                session.poll_scan()?;
                session_tree(session.tree(), params)
            }
            "ui.state" => {
                expect_empty(params)?;
                self.bridge.ui_state()
            }
            "ui.set_state" => {
                let patch = ObjectParams::parse(params)?.get("patch")?;
                if !patch.is_object() {
                    return Err(ControlError::invalid_params("patch must be an object"));
                }
                self.bridge.ui_set_state(patch)
            }
            "ui.screenshot" => self.bridge.ui_screenshot(&parse_screenshot(params)?),
            "app.quit" => {
                expect_empty(params)?;
                self.bridge.app_quit()
            }
            other => Err(ControlError::new(error::UNKNOWN_METHOD, format!("unknown method: {other}"))),
        }
    }

    fn engine_execute(&self, params: &Value) -> Result<Value, ControlError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct P {
            command: String,
            #[serde(default)]
            params: Value,
        }
        let p = P::deserialize(params).map_err(|e| ControlError::invalid_params(e.to_string()))?;
        // A remote caller must not be able to point any path-taking command at a network share (it
        // can hang on a dead server and leaks credentials to it). The CLI and UI are not affected.
        // Every string in the params is checked, so future path parameters are covered too.
        if contains_unc_path(&p.params, 0) {
            return Err(ControlError::invalid_params("UNC paths are not supported over control"));
        }
        let result = lock_session(&self.session).execute(&p.command, &p.params)?;
        Ok(result)
    }

    fn session_state(&self, params: &Value) -> Result<Value, ControlError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct P {
            path: Option<String>,
        }
        let p = P::deserialize(params).map_err(|e| ControlError::invalid_params(e.to_string()))?;
        let state = {
            let mut session = lock_session(&self.session);
            session.poll_scan()?;
            session.state_json()
        };
        match p.path {
            None => Ok(state),
            // A JSON pointer ("/selection") selects part of the state.
            Some(path) => state
                .pointer(&path)
                .cloned()
                .ok_or_else(|| ControlError::invalid_params(format!("no state at path {path}"))),
        }
    }
}

impl Backend for Dispatcher {
    fn call(&self, method: &str, params: &Value) -> Result<Value, ControlError> {
        self.call_guarded(method, params)
    }
}

/// Error response object.
fn failure(id: &Value, err: &ControlError) -> Value {
    json!({ "id": id, "ok": false, "error": { "code": err.code(), "message": err.message() } })
}

/// Serializes `response`; an oversized or unserializable result becomes an error line.
fn serialize_bounded(response: &Value) -> String {
    let text = serde_json::to_string(response).unwrap_or_default();
    if !text.is_empty() && text.len() <= MAX_RESPONSE_BYTES {
        return text;
    }
    let id = response.get("id").cloned().unwrap_or(Value::Null);
    let err = ControlError::new(error::TOO_LARGE, "response exceeds the size limit; use pagination");
    serde_json::to_string(&failure(&id, &err)).unwrap_or_else(|_| r#"{"id":null,"ok":false}"#.to_owned())
}

/// Compares in time independent of where the first difference is (length is not secret).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = u8::from(a.len() != b.len());
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

/// True if any string inside `value` (bounded depth) is a UNC path.
fn contains_unc_path(value: &Value, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match value {
        Value::String(s) => is_unc_path(s),
        Value::Array(items) => items.iter().any(|v| contains_unc_path(v, depth + 1)),
        Value::Object(map) => map.values().any(|v| contains_unc_path(v, depth + 1)),
        _ => false,
    }
}

fn expect_empty(params: &Value) -> Result<(), ControlError> {
    match params {
        Value::Object(m) if m.is_empty() => Ok(()),
        Value::Null => Ok(()),
        _ => Err(ControlError::invalid_params("this method takes no params")),
    }
}

/// A params value known to be an object.
struct ObjectParams<'a>(&'a Map<String, Value>);

impl<'a> ObjectParams<'a> {
    fn parse(params: &'a Value) -> Result<Self, ControlError> {
        params.as_object().map(Self).ok_or_else(|| ControlError::invalid_params("params must be an object"))
    }

    fn get(&self, key: &str) -> Result<&'a Value, ControlError> {
        self.0.get(key).ok_or_else(|| ControlError::invalid_params(format!("missing field `{key}`")))
    }
}

fn parse_screenshot(params: &Value) -> Result<ScreenshotRequest, ControlError> {
    let obj = ObjectParams::parse(params)?;
    if let Some(unknown) = obj.0.keys().find(|k| !matches!(k.as_str(), "path" | "region" | "overwrite")) {
        return Err(ControlError::invalid_params(format!("unknown field `{unknown}`")));
    }
    let path = obj.get("path")?.as_str().ok_or_else(|| ControlError::invalid_params("path must be a string"))?;
    if path.is_empty() || path.len() > MAX_PATH_BYTES || path.contains('\0') {
        return Err(ControlError::invalid_params("path is empty, too long or contains NUL"));
    }
    if is_unc_path(path) {
        return Err(ControlError::invalid_params("UNC paths are not supported over control"));
    }
    let region = match obj.0.get("region") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_region(v)?),
    };
    let overwrite = match obj.0.get("overwrite") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ControlError::invalid_params("overwrite must be a boolean")),
    };
    Ok(ScreenshotRequest { path: PathBuf::from(path), region, overwrite })
}

fn parse_region(value: &Value) -> Result<Region, ControlError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct R {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }
    let r = R::deserialize(value).map_err(|e| ControlError::invalid_params(format!("region: {e}")))?;
    let ok = [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite() && *v >= 0.0);
    if !ok || r.w <= 0.0 || r.h <= 0.0 {
        return Err(ControlError::invalid_params("region needs finite x,y >= 0 and w,h > 0"));
    }
    Ok(Region { x: r.x, y: r.y, w: r.w, h: r.h })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeBridge {
        quits: AtomicUsize,
    }

    impl UiBridge for FakeBridge {
        fn ui_state(&self) -> Result<Value, ControlError> {
            Ok(json!({"panel": "tree"}))
        }
        fn ui_set_state(&self, patch: &Value) -> Result<Value, ControlError> {
            Ok(json!({"applied": patch}))
        }
        fn ui_screenshot(&self, request: &ScreenshotRequest) -> Result<Value, ControlError> {
            Ok(json!({"path": request.path, "region": request.region.is_some(), "overwrite": request.overwrite}))
        }
        fn app_quit(&self) -> Result<Value, ControlError> {
            self.quits.fetch_add(1, Ordering::SeqCst);
            Ok(json!({}))
        }
    }

    struct PanicBridge;
    impl UiBridge for PanicBridge {
        fn ui_state(&self) -> Result<Value, ControlError> {
            std::panic::panic_any("bridge boom")
        }
        fn ui_set_state(&self, _: &Value) -> Result<Value, ControlError> {
            Ok(json!({}))
        }
        fn ui_screenshot(&self, _: &ScreenshotRequest) -> Result<Value, ControlError> {
            Ok(json!({}))
        }
        fn app_quit(&self) -> Result<Value, ControlError> {
            Ok(json!({}))
        }
    }

    fn fake() -> (Dispatcher, Arc<FakeBridge>) {
        let bridge = Arc::new(FakeBridge::default());
        (Dispatcher::new(share(Session::new()), bridge.clone()), bridge)
    }

    fn send(d: &Dispatcher, request: &Value) -> Value {
        serde_json::from_str(&d.handle_line(&request.to_string())).unwrap()
    }

    fn call(d: &Dispatcher, method: &str, params: Value) -> Value {
        send(d, &json!({"id": 1, "method": method, "params": params}))
    }

    fn code(v: &Value) -> &str {
        assert_eq!(v["ok"], false, "{v}");
        v["error"]["code"].as_str().unwrap()
    }

    #[test]
    fn engine_commands_lists_the_registry() {
        let (d, _) = fake();
        let v = call(&d, "engine.commands", json!({}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["id"], 1);
        let list = v["result"].as_array().unwrap();
        assert!(list.iter().any(|c| c["id"] == "scan.start" && c["destructive"] == false));
        assert_eq!(code(&call(&d, "engine.commands", json!({"x": 1}))), "invalid_params");
    }

    #[test]
    fn engine_execute_success_and_error_codes() {
        let (d, _) = fake();
        let v = call(&d, "engine.execute", json!({"command": "session.state"}));
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["result"]["nodes"], 1);
        assert_eq!(code(&call(&d, "engine.execute", json!({"command": "nope"}))), "unknown_command");
        assert_eq!(code(&call(&d, "engine.execute", json!({"command": "scan.cancel"}))), "not_enabled");
        let bad = call(&d, "engine.execute", json!({"command": "select.node", "params": {"id": 0, "bogus": 1}}));
        assert_eq!(code(&bad), "invalid_params");
        assert_eq!(code(&call(&d, "engine.execute", json!({}))), "invalid_params");
        assert_eq!(code(&call(&d, "engine.execute", json!({"command": 5}))), "invalid_params");
        assert_eq!(code(&call(&d, "engine.execute", json!({"command": "x", "extra": 1}))), "invalid_params");
        assert_eq!(code(&call(&d, "engine.execute", json!([1]))), "invalid_params");
        // The session is still usable after errors.
        assert_eq!(call(&d, "session.state", json!({}))["ok"], true);
    }

    #[test]
    fn confirmation_required_is_reported() {
        let session = share(Session::new());
        let d = Dispatcher::headless(session);
        // Any destructive command in the registry must demand confirmation over the wire.
        let list = call(&d, "engine.commands", json!({}));
        for c in list["result"].as_array().unwrap().iter().filter(|c| c["destructive"] == true) {
            let id = c["id"].as_str().unwrap();
            let v = call(&d, "engine.execute", json!({"command": id, "params": {}}));
            assert!(matches!(code(&v), "confirmation_required" | "not_enabled"), "{id}");
        }
    }

    #[test]
    fn session_state_and_pointer_path() {
        let (d, _) = fake();
        let v = call(&d, "session.state", json!({}));
        assert_eq!(v["result"]["scanning"], false);
        let v = call(&d, "session.state", json!({"path": "/nodes"}));
        assert_eq!(v["result"], 1);
        assert_eq!(code(&call(&d, "session.state", json!({"path": "/nope"}))), "invalid_params");
        assert_eq!(code(&call(&d, "session.state", json!({"zzz": 1}))), "invalid_params");
    }

    #[test]
    fn session_tree_is_served_and_capped() {
        let (d, _) = fake();
        let v = call(&d, "session.tree", json!({"depth": 99, "limit": 99999}));
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["depth"], 8);
        assert_eq!(code(&call(&d, "session.tree", json!({"node": 12345}))), "invalid_params");
    }

    #[test]
    fn unc_scan_roots_are_refused_over_control() {
        let (d, _) = fake();
        for path in [r"\\server\share", "//server/share", r"\\?\UNC\server\share", r"\\.\C:", r"  \\host\x"] {
            let v = call(&d, "engine.execute", json!({"command": "scan.start", "params": {"path": path}}));
            assert_eq!(code(&v), "invalid_params", "{path}");
            assert!(v["error"]["message"].as_str().unwrap().contains("UNC"), "{path}");
        }
        for path in [
            r"\??\UNC\server\share",
            r"\??\unc\server\share",
            "/??/UNC/server/share",
            r"\\?\unc\h\s",
            r"/\?\UNC\h\s",
            r"\??\UNC",
        ] {
            assert!(is_unc_path(path), "{path}");
            for command in ["scan.start", "select.node", "some.other"] {
                let v = call(&d, "engine.execute", json!({"command": command, "params": {"path": path}}));
                assert_eq!(code(&v), "invalid_params", "{command} {path}");
                assert!(v["error"]["message"].as_str().unwrap().contains("UNC"), "{command} {path}");
            }
            let v = call(&d, "ui.screenshot", json!({"path": path}));
            assert_eq!(code(&v), "invalid_params", "screenshot {path}");
        }
        assert!(!is_unc_path(r"\??\C:\Users"));
        assert!(!is_unc_path(r"\\?\UNCLE\x"));
        assert!(!is_unc_path(r"C:\Users"));
        assert!(!is_unc_path(r"\\?\C:\Users"));
        assert!(!is_unc_path("/home/user"));
        assert!(!is_unc_path(""));
        // The session never started a scan.
        assert_eq!(call(&d, "session.state", json!({}))["result"]["scanning"], false);
    }

    #[test]
    fn ui_methods_delegate_to_the_bridge() {
        let (d, bridge) = fake();
        assert_eq!(call(&d, "ui.state", json!({}))["result"]["panel"], "tree");
        let v = call(&d, "ui.set_state", json!({"patch": {"zoom": 2}}));
        assert_eq!(v["result"]["applied"]["zoom"], 2);
        let v = call(&d, "ui.screenshot", json!({"path": "shot.png", "region": {"x": 0, "y": 1, "w": 10, "h": 10}}));
        assert_eq!(v["result"]["path"], "shot.png");
        assert_eq!(v["result"]["region"], true);
        assert_eq!(v["result"]["overwrite"], false);
        let v = call(&d, "ui.screenshot", json!({"path": "shot.png", "overwrite": true}));
        assert_eq!(v["result"]["overwrite"], true);
        assert_eq!(call(&d, "app.quit", json!({}))["ok"], true);
        assert_eq!(bridge.quits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn ui_param_validation() {
        let (d, _) = fake();
        for p in [json!({}), json!({"patch": 1}), json!({"patch": [1]}), json!([]), json!("s")] {
            assert_eq!(code(&call(&d, "ui.set_state", p)), "invalid_params");
        }
        for p in [
            json!({}),
            json!({"path": ""}),
            json!({"path": 5}),
            json!({"path": "a\u{0}b"}),
            json!({"path": "x".repeat(5000)}),
            json!({"path": "a.png", "extra": 1}),
            json!({"path": "a.png", "region": {"x": 0, "y": 0, "w": 0, "h": 5}}),
            json!({"path": "a.png", "region": {"x": -1, "y": 0, "w": 5, "h": 5}}),
            json!({"path": "a.png", "region": {"x": 0, "y": 0, "w": 5}}),
            json!({"path": "a.png", "region": "all"}),
            json!({"path": "a.png", "overwrite": "yes"}),
            json!(null),
        ] {
            let v = call(&d, "ui.screenshot", p.clone());
            // `null` params become an empty object, which is missing `path`.
            assert_eq!(code(&v), "invalid_params", "{p}");
        }
        assert_eq!(code(&call(&d, "ui.state", json!({"a": 1}))), "invalid_params");
        assert_eq!(code(&call(&d, "app.quit", json!([1]))), "invalid_params");
    }

    #[test]
    fn headless_bridge_is_unsupported() {
        let d = Dispatcher::headless(share(Session::new()));
        for (m, p) in [
            ("ui.state", json!({})),
            ("ui.set_state", json!({"patch": {}})),
            ("ui.screenshot", json!({"path": "a.png"})),
            ("app.quit", json!({})),
        ] {
            assert_eq!(code(&call(&d, m, p)), "unsupported", "{m}");
        }
    }

    #[test]
    fn unknown_method_and_bad_requests() {
        let (d, _) = fake();
        assert_eq!(code(&call(&d, "nope.nope", json!({}))), "unknown_method");
        assert_eq!(code(&send(&d, &json!({"id": 1}))), "bad_request");
        assert_eq!(code(&send(&d, &json!({"id": 1, "method": 5}))), "bad_request");
        assert_eq!(code(&send(&d, &json!([1, 2]))), "bad_request");
        assert_eq!(code(&send(&d, &json!("text"))), "bad_request");
        let v = send(&d, &json!({"id": [1], "method": "engine.commands"}));
        assert_eq!(code(&v), "bad_request");
        // Missing params and string ids are fine.
        let v = send(&d, &json!({"id": "abc", "method": "engine.commands"}));
        assert_eq!(v["id"], "abc");
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn malformed_json_and_oversized_lines() {
        let (d, _) = fake();
        for line in ["", "{", "not json", "{\"id\":1,", "\u{0}", "{\"a\":}"] {
            let v: Value = serde_json::from_str(&d.handle_line(line)).unwrap();
            assert_eq!(code(&v), "bad_request", "{line:?}");
        }
        let big = format!(r#"{{"id":1,"method":"engine.commands","pad":"{}"}}"#, "x".repeat(MAX_REQUEST_BYTES));
        let v: Value = serde_json::from_str(&d.handle_line(&big)).unwrap();
        assert_eq!(code(&v), "too_large");
    }

    #[test]
    fn deeply_nested_json_is_an_error_not_a_crash() {
        let (d, _) = fake();
        let line = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        let v: Value = serde_json::from_str(&d.handle_line(&line)).unwrap();
        assert_eq!(code(&v), "bad_request");
    }

    #[test]
    fn key_is_enforced() {
        let d = Dispatcher::headless(share(Session::new())).with_key("secret");
        let req = |key: Option<&str>| {
            let mut r = json!({"id": 7, "method": "engine.commands"});
            if let Some(k) = key {
                r["key"] = json!(k);
            }
            send(&d, &r)
        };
        assert_eq!(code(&req(None)), "unauthorized");
        assert_eq!(code(&req(Some(""))), "unauthorized");
        assert_eq!(code(&req(Some("secre"))), "unauthorized");
        assert_eq!(code(&req(Some("secret!"))), "unauthorized");
        assert_eq!(req(Some("secret"))["ok"], true);
        let v = send(&d, &json!({"id": 7, "key": 5, "method": "engine.commands"}));
        assert_eq!(v["id"], 7);
        assert_eq!(code(&v), "unauthorized");
        // Wrong key is rejected before the method is looked at.
        assert_eq!(code(&send(&d, &json!({"key": "x"}))), "unauthorized");
    }

    #[test]
    fn checked_flag_reports_the_key_result() {
        let d = Dispatcher::headless(share(Session::new())).with_key("secret");
        let line = |key: &str| json!({"id": 1, "key": key, "method": "engine.commands"}).to_string();
        assert!(d.handle_line_checked(&line("secret")).1);
        assert!(!d.handle_line_checked(&line("nope")).1);
        assert!(!d.handle_line_checked("{oops").1);
        // A valid key with a bad method is still an authorized peer.
        assert!(d.handle_line_checked(&json!({"key": "secret"}).to_string()).1);
        assert!(Dispatcher::headless(share(Session::new())).handle_line_checked(&line("")).1);
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"\0"));
    }

    #[test]
    fn panicking_bridge_is_caught_per_request() {
        let d = Dispatcher::new(share(Session::new()), Arc::new(PanicBridge));
        assert_eq!(code(&call(&d, "ui.state", json!({}))), "internal");
        assert_eq!(call(&d, "engine.commands", json!({}))["ok"], true);
    }

    #[test]
    fn poisoned_session_mutex_is_recovered() {
        let session = share(Session::new());
        let s2 = session.clone();
        let _ = std::thread::spawn(move || {
            let _guard = s2.lock().unwrap();
            std::panic::panic_any("poison");
        })
        .join();
        assert!(session.is_poisoned());
        let d = Dispatcher::headless(session);
        assert_eq!(call(&d, "session.state", json!({}))["ok"], true);
    }

    #[test]
    fn oversized_response_becomes_an_error() {
        let big = json!({"id": 3, "ok": true, "result": "x".repeat(MAX_RESPONSE_BYTES + 1)});
        let v: Value = serde_json::from_str(&serialize_bounded(&big)).unwrap();
        assert_eq!(code(&v), "too_large");
        assert_eq!(v["id"], 3);
    }

    #[test]
    fn hostile_params_corpus_never_panics_or_breaks_the_session() {
        let (d, _) = fake();
        let long = json!("x".repeat(50_000));
        let mut nested = json!(1);
        for _ in 0..100 {
            nested = json!({"a": [nested]});
        }
        let seeds = vec![
            json!(null),
            json!(true),
            json!(-1),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(1e308),
            json!(""),
            long,
            json!("\u{0}\u{202e}\u{fffd}\u{1f980}"),
            json!("../../no_such_dir_dirustat_hostile/.."),
            json!([]),
            json!([1, "a", null]),
            json!({}),
            json!({"unknown": 1}),
            nested,
        ];
        let mut corpus = seeds.clone();
        for key in
            ["command", "params", "path", "node", "depth", "limit", "offset", "mode", "patch", "region", "confirm"]
        {
            for s in &seeds {
                corpus.push(json!({ key: s }));
            }
        }
        let methods = [
            "engine.commands",
            "engine.execute",
            "session.state",
            "session.tree",
            "ui.state",
            "ui.set_state",
            "ui.screenshot",
            "app.quit",
            "",
            "\u{0}",
        ];
        for m in methods {
            for p in &corpus {
                let line = json!({"id": 1, "method": m, "params": p}).to_string();
                let out = d.handle_line(&line);
                let v: Value = serde_json::from_str(&out).unwrap();
                assert!(v["ok"].is_boolean(), "{m} {p}");
            }
        }
        assert_eq!(call(&d, "session.state", json!({}))["ok"], true);
    }
}
