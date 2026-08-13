//! Codex App Server 线协议（JSON-RPC *风格*，线上**省略 `jsonrpc` 字段**）。
//!
//! 协议基线 2026-08：stdio JSONL 为默认传输；未知 method → ProtocolUnsupported；
//! 携带语义的未知/未映射字段显式失败，禁止静默丢弃。legacy `thread/compacted`
//! 通知与 `contextCompaction` item **不等价**，不得互相顶替。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 错误码。
pub const ERROR_PARSE: i32 = -32700;
pub const ERROR_INVALID_REQUEST: i32 = -32600;
pub const ERROR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERROR_INVALID_PARAMS: i32 = -32602;
pub const ERROR_INTERNAL: i32 = -32603;
/// 有界 ingress 饱和：官方固定文案，客户端应按可重试处理。
pub const ERROR_OVERLOADED: i32 = -32001;
pub const ERROR_OVERLOADED_MESSAGE: &str = "Server overloaded; retry later.";
pub const ERROR_NOT_INITIALIZED: &str = "Not initialized";
pub const ERROR_ALREADY_INITIALIZED: &str = "Already initialized";

/// JSON-RPC id（number | string | null）。
pub type JsonRpcId = Value;

/// JSON-RPC 错误对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn overloaded() -> Self {
        Self::new(ERROR_OVERLOADED, ERROR_OVERLOADED_MESSAGE)
    }

    pub fn not_initialized() -> Self {
        Self::new(ERROR_INVALID_REQUEST, ERROR_NOT_INITIALIZED)
    }

    pub fn already_initialized() -> Self {
        Self::new(ERROR_INVALID_REQUEST, ERROR_ALREADY_INITIALIZED)
    }
}

/// 线协议请求（无 `jsonrpc` 字段）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// 线协议通知（无 id）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// 线协议成功响应。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub id: JsonRpcId,
    pub result: Value,
}

/// 线协议错误响应。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub id: JsonRpcId,
    pub error: JsonRpcError,
}

/// 解析后的线协议消息。
#[derive(Clone, Debug, PartialEq)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
    Error(JsonRpcErrorResponse),
}

impl JsonRpcMessage {
    /// 解析原始 JSON。`jsonrpc` 字段必须缺席；其它未知顶层字段显式失败。
    pub fn parse(value: Value) -> Result<Self, JsonRpcError> {
        let Some(object) = value.as_object() else {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "request must be a JSON object",
            ));
        };
        if object.contains_key("jsonrpc") {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "jsonrpc field must be omitted on the Codex app-server wire",
            ));
        }
        const KNOWN: &[&str] = &["id", "method", "params", "result", "error"];
        let unknown: Vec<&String> = object
            .keys()
            .filter(|key| !KNOWN.contains(&key.as_str()))
            .collect();
        if !unknown.is_empty() {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                format!(
                    "unsupported wire fields: {}",
                    unknown
                        .iter()
                        .map(|key| key.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ));
        }
        let has_id = object.contains_key("id");
        let has_method = object.contains_key("method");
        if has_method {
            let method = object
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    JsonRpcError::new(ERROR_INVALID_REQUEST, "method must be a string")
                })?;
            let params = object.get("params").cloned();
            if has_id {
                let id = object.get("id").cloned().ok_or_else(|| {
                    JsonRpcError::new(ERROR_INVALID_REQUEST, "request id missing")
                })?;
                Ok(Self::Request(JsonRpcRequest {
                    id,
                    method: method.into(),
                    params,
                }))
            } else {
                Ok(Self::Notification(JsonRpcNotification {
                    method: method.into(),
                    params,
                }))
            }
        } else if has_id {
            let id = object
                .get("id")
                .cloned()
                .ok_or_else(|| JsonRpcError::new(ERROR_INVALID_REQUEST, "response id missing"))?;
            if let Some(error) = object.get("error") {
                let error = serde_json::from_value(error.clone()).map_err(|_| {
                    JsonRpcError::new(ERROR_INVALID_REQUEST, "invalid error object")
                })?;
                Ok(Self::Error(JsonRpcErrorResponse { id, error }))
            } else if let Some(result) = object.get("result") {
                Ok(Self::Response(JsonRpcResponse {
                    id,
                    result: result.clone(),
                }))
            } else {
                Err(JsonRpcError::new(
                    ERROR_INVALID_REQUEST,
                    "response must carry result or error",
                ))
            }
        } else {
            Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "message must carry method or id",
            ))
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            Self::Request(message) => serde_json::to_value(message),
            Self::Notification(message) => serde_json::to_value(message),
            Self::Response(message) => serde_json::to_value(message),
            Self::Error(message) => serde_json::to_value(message),
        }
        .expect("Codex wire messages always serialize")
    }

    /// 单行 JSONL（无尾随空白）。
    pub fn to_jsonl(&self) -> String {
        serde_json::to_string(&self.to_value()).expect("Codex wire messages always serialize")
    }
}

/// params 未知字段显式拒绝。
pub trait ParamsExt {
    fn extra(&self) -> &BTreeMap<String, Value>;

    fn reject_unknown(&self, method: &str) -> Result<(), String> {
        if self.extra().is_empty() {
            Ok(())
        } else {
            Err(format!(
                "unsupported params fields for `{method}`: {}",
                self.extra().keys().cloned().collect::<Vec<_>>().join(",")
            ))
        }
    }
}

/// `initialize.params.clientInfo`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ClientInfo {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `initialize.params.capabilities`。未知能力进入 `extra`，由协商层降级。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexClientCapabilities {
    #[serde(default)]
    pub experimental_api: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub opt_out_notification_methods: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `initialize` 请求参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CodexClientCapabilities>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for InitializeParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `initialize` 成功响应。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub user_agent: String,
    pub codex_home: String,
    pub platform_family: String,
    pub platform_os: String,
}

/// `thread/start` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 工具命名空间 / 动态工具。未协商 `tool.namespace` 时在使用点显式失败。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_tools: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ThreadStartParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `thread/resume` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ThreadResumeParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `thread/fork` 参数。`parentThreadId` 由服务端写回 `forkedFromId`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkParams {
    pub thread_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ThreadForkParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `thread/list` 参数。`parentThreadId` / `ancestorThreadId` 为实验过滤器。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestor_thread_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ThreadListParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `thread/unsubscribe` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadUnsubscribeParams {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ThreadUnsubscribeParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `thread/compact/start` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCompactParams {
    pub thread_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ThreadCompactParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `turn/start` / `turn/steer` 的用户输入块。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserInput {
    Text {
        text: String,
    },
    Image {
        url: String,
    },
    LocalImage {
        path: String,
    },
    Audio {
        url: String,
    },
    LocalAudio {
        path: String,
    },
    #[serde(other)]
    Unknown,
}

/// `turn/start` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for TurnStartParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `turn/steer` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for TurnSteerParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `turn/interrupt` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for TurnInterruptParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// server→client `item/commandExecution/requestApproval` 参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandApprovalParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// 客户端审批响应 `{ decision }`。复杂 amendment 变体视为未支持语义。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDecisionResult {
    pub decision: ApprovalDecisionWire,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for ApprovalDecisionResult {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// 命令审批 decision。带 execpolicy/network amendment 的对象形态显式拒绝。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecisionWire {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
}

/// 线协议 Thread 对象（响应与 `thread/started`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadObject {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// 线协议 Turn 对象。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnObject {
    pub id: String,
    pub status: TurnStatus,
    #[serde(default)]
    pub items: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    InProgress,
    Completed,
    Interrupted,
    Failed,
}

/// 是否为 server→client JSON-RPC **请求**（非通知）。
pub fn is_server_request(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
    )
}

/// 已废弃、不得与 `contextCompaction` 互相顶替的通知名。
pub const DEPRECATED_THREAD_COMPACTED: &str = "thread/compacted";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn omits_jsonrpc_field_on_the_wire() {
        let message = JsonRpcMessage::Request(JsonRpcRequest {
            id: json!(0),
            method: "initialize".into(),
            params: Some(json!({"clientInfo": {"name": "t", "version": "1"}})),
        });
        let value = message.to_value();
        assert!(value.get("jsonrpc").is_none());
        assert_eq!(value["method"], "initialize");
    }

    #[test]
    fn rejects_jsonrpc_field_on_parse() {
        let error = JsonRpcMessage::parse(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize"
        }))
        .expect_err("jsonrpc must be omitted");
        assert_eq!(error.code, ERROR_INVALID_REQUEST);
        assert!(error.message.contains("jsonrpc"));
    }

    #[test]
    fn overloaded_error_matches_official_copy() {
        let error = JsonRpcError::overloaded();
        assert_eq!(error.code, ERROR_OVERLOADED);
        assert_eq!(error.message, ERROR_OVERLOADED_MESSAGE);
    }
}
