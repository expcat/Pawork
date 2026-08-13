//! 最小 JSON-RPC 2.0 消息模型与序列化（仅 LSP 客户端用到的子集）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC id。LSP 习惯用整数；规范允许 string/null。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    Null,
    Number(i64),
    String(String),
}

impl RequestId {
    pub fn as_json(&self) -> Value {
        match self {
            RequestId::Null => Value::Null,
            RequestId::Number(n) => Value::from(*n),
            RequestId::String(s) => Value::from(s.as_str()),
        }
    }
}

/// 客户端发出的 JSON-RPC 请求。
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    pub jsonrpc: &'static str,
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id: id.into().as_json(),
            method: method.into(),
            params,
        }
    }
}

/// 客户端发出的 JSON-RPC 通知（无 id）。
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
        }
    }
}

/// 服务端返回的 JSON-RPC 响应。
#[derive(Debug, Clone, Deserialize)]
pub struct Response {
    #[serde(default)]
    pub id: Value,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// 服务端消息：可能是响应（带 id）、也可能是通知（无 id，如 publishDiagnostics）。
#[derive(Debug, Clone, Deserialize)]
pub struct ServerMessage {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

impl ServerMessage {
    /// 分流：若带 id 视为响应，否则视为服务端通知。
    pub fn kind(&self) -> ServerMessageKind<'_> {
        if self.id.is_some() {
            ServerMessageKind::Response(self)
        } else {
            ServerMessageKind::Notification(self)
        }
    }
}

pub enum ServerMessageKind<'a> {
    Response(&'a ServerMessage),
    Notification(&'a ServerMessage),
}

impl From<i64> for RequestId {
    fn from(v: i64) -> Self {
        RequestId::Number(v)
    }
}
impl From<&str> for RequestId {
    fn from(v: &str) -> Self {
        RequestId::String(v.to_string())
    }
}

/// JSON-RPC 标准错误码。
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    /// `$/cancelRequest` 之外，服务端定义的错误码起点。
    pub const SERVER_ERROR_START: i32 = -32099;
    /// LSP 请求被取消。
    pub const REQUEST_CANCELLED: i32 = -32800;
    /// LSP 请求内容被修改（例如 didChange 使结果失效）。
    pub const CONTENT_MODIFIED: i32 = -32801;
}
