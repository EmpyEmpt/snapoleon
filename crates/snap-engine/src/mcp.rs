//! MCP server over stdio (JSON-RPC 2.0, newline-delimited). Tools:
//! list_monitors, list_windows, capture_screen, capture_window,
//! capture_region, ocr_image, decode_codes, search_history, get_capture.
//!
//! Stdout carries protocol messages only, one JSON object per line. Diagnostics go through `tracing`, which the
//! host binary must send to stderr.

use std::io::{BufRead, Write};

use serde_json::{Value, json};
use snap_core::Result;

use crate::Engine;

mod tools;

/// Versions we can speak, newest first. A client asking for another one is answered with the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Snapoleon gives you eyes on the user's screen. Use list_monitors and list_windows to see what exists, \
capture_screen, capture_window or capture_region to look at it (every capture returns a capture_id), ocr_image and decode_codes to \
read text and QR codes from a capture or file, and search_history to find earlier screenshots by what was on them.";

pub fn serve_stdio(engine: Engine) -> Result<()> {
    serve(engine, std::io::stdin().lock(), std::io::stdout().lock())
}

/// The server loop over any reader and writer; returns when the input ends.
pub fn serve(engine: Engine, input: impl BufRead, mut output: impl Write) -> Result<()> {
    let mut server = Server { tools: tools::Tools::new(engine) };
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line) {
            writeln!(output, "{reply}")?;
            output.flush()?;
        }
    }
    Ok(())
}

struct Server {
    tools: tools::Tools,
}

/// A JSON-RPC error object.
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl Server {
    fn handle_line(&mut self, line: &str) -> Option<Value> {
        match serde_json::from_str::<Value>(line) {
            Err(e) => Some(error_reply(Value::Null, &RpcError::new(PARSE_ERROR, format!("parse error: {e}")))),
            // Batches are optional in the protocol (and gone in the newest revision), but cost nothing to support.
            Ok(Value::Array(batch)) if !batch.is_empty() => {
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| self.handle_message(m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(message) => self.handle_message(message),
        }
    }

    fn handle_message(&mut self, message: Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error_reply(Value::Null, &RpcError::new(INVALID_REQUEST, "a request must be a JSON object")));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response to something we never asked for, or garbage.
            return id.map(|id| error_reply(id, &RpcError::new(INVALID_REQUEST, "missing method")));
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let result = self.dispatch(method, &params);
        // Notifications (no id) never get a reply, not even for errors.
        let id = id?;
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(e) => error_reply(id, &e),
        })
    }

    fn dispatch(&mut self, method: &str, params: &Value) -> std::result::Result<Value, RpcError> {
        match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::new(INVALID_PARAMS, "tools/call needs a tool name"))?;
                if !tools::exists(name) {
                    return Err(RpcError::new(INVALID_PARAMS, format!("unknown tool: {name}")));
                }
                let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                Ok(match self.tools.call(name, &args) {
                    Ok(output) => output.into_result(false),
                    Err(e) => tools::Output::text(crate::exec::error_text(&e)).into_result(true),
                })
            }
            // Notifications we accept silently.
            "notifications/initialized" | "notifications/cancelled" | "notifications/roots/list_changed" => Ok(Value::Null),
            other => Err(RpcError::new(METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS.into_iter().find(|v| Some(*v) == requested).unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "snapoleon", "title": snap_core::APP_NAME, "version": snap_core::VERSION },
        "instructions": INSTRUCTIONS,
    })
}

fn error_reply(id: Value, e: &RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } })
}
