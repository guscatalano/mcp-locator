//! JSON-RPC 2.0 types for the broker pipe protocol (spec/002 §3).
//!
//! Framing is newline-delimited: one JSON object per line, in both directions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version spoken over the pipe. Additive changes only within a major version;
/// clients and broker negotiate at handshake (spec/002 §2).
pub const BROKER_PROTOCOL: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// JSON-RPC reserved codes, plus the broker's own range starting at -32000.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResult {
    #[serde(rename = "brokerVersion")]
    pub broker_version: String,
    #[serde(rename = "brokerProtocol")]
    pub broker_protocol: u32,
}

/// Runtime state of a registered server. Only `Registered` is reachable in this slice —
/// activation and the rest of the lifetime machine land with the activation engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerState {
    Registered,
    Launching,
    Running,
    Idle,
    Stopping,
    Orphaned,
}
