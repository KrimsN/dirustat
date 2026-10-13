//! MCP server over stdio (JSON-RPC 2.0, one message per line), written by hand on `serde_json`.
//! Every registry command is a tool; `dirustat://session/state` and `dirustat://session/tree`
//! are resources.

use crate::client::ControlClient;
use crate::dispatcher::{Backend, Dispatcher, MAX_REQUEST_BYTES, SharedSession};
use crate::error::{self, ControlError};
use crate::line::{LineRead, discard_line, read_line_bounded};
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};

const STATE_URI: &str = "dirustat://session/state";
const TREE_URI: &str = "dirustat://session/tree";
const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const RESOURCE_NOT_FOUND: i64 = -32002;

/// Serves MCP for an in-process session until the reader ends. `ui.*` is unavailable here.
pub fn serve_mcp<R: BufRead, W: Write>(session: SharedSession, reader: R, writer: W) -> io::Result<()> {
    serve_mcp_with(&Dispatcher::headless(session), reader, writer)
}

/// `mcp --connect`: serves MCP on the given streams, forwarding to a running control channel.
pub fn serve_mcp_connect<R: BufRead, W: Write>(addr: &str, key: &str, reader: R, writer: W) -> io::Result<()> {
    let client = ControlClient::connect(addr, key)?;
    serve_mcp_with(&client, reader, writer)
}

/// Serves MCP against any [`Backend`].
pub fn serve_mcp_with<B: Backend + ?Sized, R: BufRead, W: Write>(
    backend: &B,
    mut reader: R,
    mut writer: W,
) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let reply = match read_line_bounded(&mut reader, MAX_REQUEST_BYTES, &mut buf, None)? {
            LineRead::Eof => return Ok(()),
            LineRead::TooLong { line_complete } => {
                if !line_complete {
                    discard_line(&mut reader)?;
                }
                Some(rpc_error(&Value::Null, INVALID_REQUEST, "message too large"))
            }
            LineRead::Line => handle_bytes(backend, &buf),
        };
        if let Some(reply) = reply {
            writer.write_all(reply.to_string().as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
}

fn handle_bytes<B: Backend + ?Sized>(backend: &B, bytes: &[u8]) -> Option<Value> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let message: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => return Some(rpc_error(&Value::Null, PARSE_ERROR, &format!("parse error: {e}"))),
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    match panic::catch_unwind(AssertUnwindSafe(|| handle_message(backend, &message))) {
        Ok(reply) => reply,
        Err(_) => Some(rpc_error(&id, INTERNAL_ERROR, "internal error")),
    }
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Handles one JSON-RPC message; `None` for notifications.
pub fn handle_message<B: Backend + ?Sized>(backend: &B, message: &Value) -> Option<Value> {
    let Some(obj) = message.as_object() else {
        return Some(rpc_error(&Value::Null, INVALID_REQUEST, "batches and non-objects are not supported"));
    };
    let id = obj.get("id");
    let method = obj.get("method").and_then(Value::as_str);
    let (Some(id), Some(method)) = (id, method) else {
        // No id: a notification (or a response we never asked for) gets no reply.
        return if obj.contains_key("id") {
            Some(rpc_error(&Value::Null, INVALID_REQUEST, "method must be a string"))
        } else {
            None
        };
    };
    if !matches!(id, Value::Number(_) | Value::String(_)) {
        return Some(rpc_error(&Value::Null, INVALID_REQUEST, "id must be a number or string"));
    }
    let empty = Value::Object(Map::new());
    let params = obj.get("params").unwrap_or(&empty);
    let outcome = match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => tools_list(backend),
        "tools/call" => tools_call(backend, params),
        "resources/list" => Ok(resources_list()),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "resources/read" => resources_read(backend, params),
        other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
    };
    Some(match outcome {
        Ok(result) => rpc_result(id, result),
        Err((code, message)) => rpc_error(id, code, &message),
    })
}

type RpcFailure = (i64, String);

fn backend_failure(err: &ControlError) -> RpcFailure {
    (INTERNAL_ERROR, err.to_string())
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested.filter(|v| SUPPORTED_VERSIONS.contains(v)).unwrap_or(SUPPORTED_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false }
        },
        "serverInfo": { "name": "dirustat", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "diRUSTat disk analyzer. Every command is a tool; start with scan.start, then read dirustat://session/state."
    })
}

fn tools_list<B: Backend + ?Sized>(backend: &B) -> Result<Value, RpcFailure> {
    let commands = backend.call("engine.commands", &json!({})).map_err(|e| backend_failure(&e))?;
    let tools: Vec<Value> =
        commands.as_array().map(|list| list.iter().filter_map(tool_from_command).collect()).unwrap_or_default();
    Ok(json!({ "tools": tools }))
}

/// Builds a tool description from one `CommandInfo` JSON object.
fn tool_from_command(info: &Value) -> Option<Value> {
    let id = info.get("id")?.as_str()?;
    let label = info.get("label").and_then(Value::as_str).unwrap_or(id);
    let doc = info.get("params_doc").and_then(Value::as_str).unwrap_or("");
    let destructive = info.get("destructive").and_then(Value::as_bool).unwrap_or(false);
    let mut description = format!("{label}. Params: {doc}");
    let schema = if destructive {
        description.push_str(" DESTRUCTIVE: requires confirm=true; never run on real user data.");
        json!({
            "type": "object", "description": doc, "additionalProperties": true,
            "properties": { "confirm": { "type": "boolean", "const": true, "description": "must be true to run" } },
            "required": ["confirm"]
        })
    } else {
        json!({ "type": "object", "description": doc, "additionalProperties": true })
    };
    Some(json!({
        "name": id,
        "title": label,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "title": label,
            "readOnlyHint": !destructive,
            "destructiveHint": destructive,
            "destructive": destructive,
            "idempotentHint": false,
            "openWorldHint": false
        },
        "_meta": {
            "menu": info.get("menu").cloned().unwrap_or(Value::Null),
            "shortcut": info.get("shortcut").cloned().unwrap_or(Value::Null),
            "enabled": info.get("enabled").cloned().unwrap_or(Value::Null)
        }
    }))
}

fn tools_call<B: Backend + ?Sized>(backend: &B, params: &Value) -> Result<Value, RpcFailure> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "tools/call needs a string `name`".to_owned()))?;
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(a @ Value::Object(_)) => a.clone(),
        Some(_) => return Err((INVALID_PARAMS, "`arguments` must be an object".to_owned())),
    };
    match backend.call("engine.execute", &json!({ "command": name, "params": arguments })) {
        Ok(value) => Ok(tool_result(&value.to_string(), false)),
        Err(e) if e.code() == error::UNKNOWN_COMMAND => Err((INVALID_PARAMS, format!("unknown tool: {name}"))),
        Err(e) => Ok(tool_result(&e.to_string(), true)),
    }
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn resources_list() -> Value {
    json!({ "resources": [
        { "uri": STATE_URI, "name": "Session state", "description": "Revision, node count, scan progress, selection.", "mimeType": "application/json" },
        { "uri": TREE_URI, "name": "Session tree", "description": "Largest children of the root (first page; use session.tree via the control channel for more).", "mimeType": "application/json" }
    ] })
}

fn resources_read<B: Backend + ?Sized>(backend: &B, params: &Value) -> Result<Value, RpcFailure> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "resources/read needs a string `uri`".to_owned()))?;
    let method = match uri {
        STATE_URI => "session.state",
        TREE_URI => "session.tree",
        _ => return Err((RESOURCE_NOT_FOUND, format!("resource not found: {uri}"))),
    };
    let value = backend.call(method, &json!({})).map_err(|e| backend_failure(&e))?;
    Ok(json!({ "contents": [{ "uri": uri, "mimeType": "application/json", "text": value.to_string() }] }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::share;
    use dirustat_engine::Session;
    use std::io::Cursor;

    fn run(input: &str) -> Vec<Value> {
        let mut out = Vec::new();
        serve_mcp(share(Session::new()), Cursor::new(input.as_bytes().to_vec()), &mut out).unwrap();
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    fn one(request: &Value) -> Value {
        run(&format!("{request}\n")).remove(0)
    }

    #[test]
    fn initialize_negotiates_version() {
        let v = one(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}));
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(v["result"]["serverInfo"]["name"], "dirustat");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        let v = one(&json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999"}}));
        assert_eq!(v["result"]["protocolVersion"], SUPPORTED_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply_and_ping_works() {
        let out = run(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\n{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], 9);
        assert_eq!(out[0]["result"], json!({}));
    }

    #[test]
    fn tools_list_covers_the_registry() {
        let v = one(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        let tools = v["result"]["tools"].as_array().unwrap();
        let start = tools.iter().find(|t| t["name"] == "scan.start").unwrap();
        assert_eq!(start["inputSchema"]["type"], "object");
        assert_eq!(start["annotations"]["destructiveHint"], false);
        assert!(start["description"].as_str().unwrap().contains("Params:"));
        let n = Session::new().commands().len();
        assert_eq!(tools.len(), n);
    }

    #[test]
    fn destructive_tool_schema_requires_confirm() {
        let info = json!({"id":"file.delete","label":"Delete","params_doc":"path","destructive":true});
        let tool = tool_from_command(&info).unwrap();
        assert_eq!(tool["annotations"]["destructive"], true);
        assert_eq!(tool["inputSchema"]["required"], json!(["confirm"]));
        assert!(tool_from_command(&json!({"label": "x"})).is_none());
    }

    #[test]
    fn tools_call_success_error_and_unknown() {
        let v = one(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"session.state"}}));
        assert_eq!(v["result"]["isError"], false);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap()["nodes"], 1);
        let v =
            one(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"scan.cancel","arguments":{}}}));
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("not_enabled"));
        let v = one(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"nope"}}));
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
        for params in [json!({}), json!({"name": 5}), json!({"name":"session.state","arguments":[1]}), json!(null)] {
            let v = one(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":params}));
            assert_eq!(v["error"]["code"], INVALID_PARAMS);
        }
        let v = one(
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"select.node","arguments":{"id":0,"x":1}}}),
        );
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn resources_list_and_read() {
        let v = one(&json!({"jsonrpc":"2.0","id":1,"method":"resources/list"}));
        let uris: Vec<&str> =
            v["result"]["resources"].as_array().unwrap().iter().filter_map(|r| r["uri"].as_str()).collect();
        assert_eq!(uris, [STATE_URI, TREE_URI]);
        for uri in uris {
            let v = one(&json!({"jsonrpc":"2.0","id":2,"method":"resources/read","params":{"uri":uri}}));
            let c = &v["result"]["contents"][0];
            assert_eq!(c["uri"], uri);
            assert!(serde_json::from_str::<Value>(c["text"].as_str().unwrap()).unwrap().is_object());
        }
        let v = one(&json!({"jsonrpc":"2.0","id":3,"method":"resources/read","params":{"uri":"dirustat://x"}}));
        assert_eq!(v["error"]["code"], RESOURCE_NOT_FOUND);
        let v = one(&json!({"jsonrpc":"2.0","id":4,"method":"resources/read","params":{}}));
        assert_eq!(v["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn protocol_errors() {
        let v = one(&json!({"jsonrpc":"2.0","id":1,"method":"prompts/list"}));
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        let out = run("{broken\n[1,2]\n{\"id\":[1],\"method\":\"ping\"}\n{\"id\":1,\"method\":5}\n");
        assert_eq!(out[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(out[1]["error"]["code"], INVALID_REQUEST);
        assert_eq!(out[2]["error"]["code"], INVALID_REQUEST);
        assert_eq!(out[3]["error"]["code"], INVALID_REQUEST);
        let out = run("\u{ff}\n");
        assert_eq!(out[0]["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn oversized_message_is_rejected_and_stream_recovers() {
        let input =
            format!("{}\n{}\n", "x".repeat(MAX_REQUEST_BYTES + 5), json!({"jsonrpc":"2.0","id":1,"method":"ping"}));
        let out = run(&input);
        assert_eq!(out[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(out[1]["result"], json!({}));
    }

    #[test]
    fn connect_bridge_forwards_to_a_running_server() {
        use crate::bridge::NullBridge;
        use crate::server::ControlServer;
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let session = share(Session::new());
        let server = ControlServer::start(0, dir.path(), session, Arc::new(NullBridge)).unwrap();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/read\",\"params\":{\"uri\":\"dirustat://session/state\"}}\n"
        );
        let mut out = Vec::new();
        let addr = format!("127.0.0.1:{}", server.port());
        serve_mcp_connect(&addr, server.key(), Cursor::new(input.as_bytes().to_vec()), &mut out).unwrap();
        let lines: Vec<Value> =
            String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(lines[0]["result"]["tools"].as_array().unwrap().len() > 3);
        assert!(lines[1]["result"]["contents"][0]["text"].is_string());
        // Wrong key: every call fails with an error, not a crash.
        let mut out = Vec::new();
        serve_mcp_connect(&addr, "wrong", Cursor::new(input.as_bytes().to_vec()), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("unauthorized"));
        // Non-loopback targets are refused before any key is sent.
        assert!(ControlClient::connect("8.8.8.8:80", "k").is_err());
        let client = ControlClient::connect_with_key_dir(&addr, dir.path()).unwrap();
        assert_eq!(client.call("session.state", &json!({})).unwrap()["nodes"], 1);
        assert_eq!(client.call("nope", &json!({})).unwrap_err().code(), "unknown_method");
    }
}
