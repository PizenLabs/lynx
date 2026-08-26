//! JSON-RPC 2.0 envelope over stdio: framing, lifecycle methods, and
//! routing between the MCP tool protocol and raw primitive calls.

use std::io::{BufRead, Write};

use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::session::Session;
use crate::tools;

/// MCP protocol version this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// One inbound JSON-RPC frame.
#[derive(Debug, Deserialize)]
struct Frame {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// Serves JSON-RPC over stdin/stdout until EOF.
///
/// All diagnostics go to stderr; stdout carries only response frames.
pub(crate) fn serve(session: &Session) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(session, &line) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Handles one raw request line; `None` means "produce no response"
/// (notifications, blank input).
fn handle_line(session: &Session, line: &str) -> Option<String> {
    let frame: Frame = match serde_json::from_str(line) {
        Ok(frame) => frame,
        Err(err) => {
            return Some(error_response(
                Value::Null,
                -32700,
                format!("parse error: {err}"),
            ));
        }
    };
    // Requests carry an id; notifications do not and are never answered.
    let id = frame.id?;
    Some(route(session, id, &frame.method, frame.params.as_ref()))
}

fn route(session: &Session, id: Value, method: &str, params: Option<&Value>) -> String {
    let empty = Value::Null;
    let params = params.unwrap_or(&empty);
    match method {
        "initialize" => success_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "lynx-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "ping" => success_response(id, json!({})),
        "tools/list" => success_response(id, tools::definitions()),
        "tools/call" => call_tool(session, id, params),
        _ if tools::TOOL_NAMES.contains(&method) => {
            // Legacy direct-method form: raw primitive result instead of an
            // MCP content envelope.
            match tools::dispatch(session.engine(), session.workspace_root(), method, params) {
                Ok(result) => success_response(id, result),
                Err(failure) => error_response(id, failure.code, failure.message),
            }
        }
        _ => error_response(id, -32601, format!("method not found: {method}")),
    }
}

fn call_tool(session: &Session, id: Value, params: &Value) -> String {
    #[derive(Deserialize)]
    struct CallParams {
        name: String,
        #[serde(default)]
        arguments: Value,
    }
    let call: CallParams = match serde_json::from_value(params.clone()) {
        Ok(call) => call,
        Err(err) => return error_response(id, -32602, format!("invalid params: {err}")),
    };
    if !tools::TOOL_NAMES.contains(&call.name.as_str()) {
        return error_response(id, -32602, format!("unknown tool: {}", call.name));
    }
    match tools::dispatch(
        session.engine(),
        session.workspace_root(),
        &call.name,
        &call.arguments,
    ) {
        Ok(result) => success_response(
            id,
            json!({
                "content": [{"type": "text", "text":
                    serde_json::to_string_pretty(&result).expect("Value serializes")}],
                "isError": false,
            }),
        ),
        Err(failure) => {
            if failure.code == -32602 {
                // Client-side contract violations stay protocol errors.
                return error_response(id, failure.code, failure.message);
            }
            success_response(
                id,
                json!({
                    "content": [{"type": "text", "text": failure.message}],
                    "isError": true,
                }),
            )
        }
    }
}

fn success_response(id: Value, result: Value) -> String {
    serde_json::to_string(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
        .expect("response serializes")
}

fn error_response(id: Value, code: i64, message: impl Into<String>) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()},
    }))
    .expect("response serializes")
}

/// Full-frame handling, exposed for tests.
#[cfg(test)]
pub(crate) fn handle_for_test(session: &Session, line: &str) -> Option<String> {
    handle_line(session, line)
}
