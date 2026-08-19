//! ACP v1 稳定线协议 wire 类型（`protocolVersion = 1`，2026-08 官方稳定版）。
//!
//! 只覆盖 P17-7 首轮映射面：`initialize`、`session/new`、`session/resume`、
//! `session/close`、`session/prompt`、`session/cancel`、`session/update`、
//! `session/request_permission`、`$/cancel_request`。字段命名遵循 ACP v1 schema
//! 的 camelCase；`sessionUpdate` 判别值按 schema 为 snake_case。
//!
//! 未知参数规则（显式失败，不静默丢字段）：除规范保留的 `_meta` 外，任何未列入
//! 首轮映射表的 params 字段都会在 [`ParamsExt::reject_unknown`] 中被显式拒绝。
//! 能力对象（`clientCapabilities` / `agentCapabilities`）是可选扩展点，未知能力
//! 由协商层降级记录，不拒绝整个握手。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 当前稳定 wire 协议版本（整数 major，只在不兼容变更时递增）。
pub const PROTOCOL_VERSION: u16 = 1;

/// JSON-RPC 错误码（ACP v1 schema ErrorCode）。
pub const ERROR_PARSE: i32 = -32700;
pub const ERROR_INVALID_REQUEST: i32 = -32600;
pub const ERROR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERROR_INVALID_PARAMS: i32 = -32602;
pub const ERROR_INTERNAL: i32 = -32603;
pub const ERROR_REQUEST_CANCELLED: i32 = -32800;
pub const ERROR_AUTH_REQUIRED: i32 = -32000;
pub const ERROR_RESOURCE_NOT_FOUND: i32 = -32002;

/// JSON-RPC id（number | string | null）。
pub type JsonRpcId = Value;

/// JSON-RPC 2.0 错误对象。
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
}

/// JSON-RPC 2.0 请求（client → agent）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 通知（无 id，无响应）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 成功响应。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub result: Value,
}

/// JSON-RPC 2.0 错误响应。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub id: JsonRpcId,
    pub error: JsonRpcError,
}

/// 解析后的 JSON-RPC 消息。手动解析以获得规范精确的 -32700 / -32600 语义。
#[derive(Clone, Debug, PartialEq)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
    Error(JsonRpcErrorResponse),
}

impl JsonRpcMessage {
    /// 解析原始 JSON 值。返回 JSON-RPC 规范错误对象而非 panic。
    pub fn parse(value: Value) -> Result<Self, JsonRpcError> {
        let Some(object) = value.as_object() else {
            return Err(JsonRpcError::new(
                ERROR_INVALID_REQUEST,
                "request must be a JSON object",
            ));
        };
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
                    jsonrpc: jsonrpc_version(object)?,
                    id,
                    method: method.into(),
                    params,
                }))
            } else {
                Ok(Self::Notification(JsonRpcNotification {
                    jsonrpc: jsonrpc_version(object)?,
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
                Ok(Self::Error(JsonRpcErrorResponse {
                    jsonrpc: jsonrpc_version(object)?,
                    id,
                    error,
                }))
            } else if let Some(result) = object.get("result") {
                Ok(Self::Response(JsonRpcResponse {
                    jsonrpc: jsonrpc_version(object)?,
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

    /// 序列化回 JSON 值（结构性构造，不会失败）。
    pub fn to_value(&self) -> Value {
        match self {
            Self::Request(message) => serde_json::to_value(message),
            Self::Notification(message) => serde_json::to_value(message),
            Self::Response(message) => serde_json::to_value(message),
            Self::Error(message) => serde_json::to_value(message),
        }
        .expect("JSON-RPC messages always serialize")
    }
}

impl JsonRpcRequest {
    /// 序列化回 JSON 值（结构性构造，不会失败）。
    pub fn to_value(&self) -> Value {
        JsonRpcMessage::Request(self.clone()).to_value()
    }
}

impl JsonRpcNotification {
    /// 序列化回 JSON 值（结构性构造，不会失败）。
    pub fn to_value(&self) -> Value {
        JsonRpcMessage::Notification(self.clone()).to_value()
    }
}

impl JsonRpcResponse {
    /// 序列化回 JSON 值（结构性构造，不会失败）。
    pub fn to_value(&self) -> Value {
        JsonRpcMessage::Response(self.clone()).to_value()
    }
}

impl JsonRpcErrorResponse {
    /// 序列化回 JSON 值（结构性构造，不会失败）。
    pub fn to_value(&self) -> Value {
        JsonRpcMessage::Error(self.clone()).to_value()
    }
}

fn jsonrpc_version(object: &serde_json::Map<String, Value>) -> Result<String, JsonRpcError> {
    match object.get("jsonrpc").and_then(Value::as_str) {
        Some("2.0") => Ok("2.0".into()),
        Some(other) => Err(JsonRpcError::new(
            ERROR_INVALID_REQUEST,
            format!("unsupported jsonrpc version `{other}`"),
        )),
        None => Err(JsonRpcError::new(
            ERROR_INVALID_REQUEST,
            "jsonrpc version must be \"2.0\"",
        )),
    }
}

/// `_meta` 是 ACP 规范的保留扩展通道：值不透明，允许存在但不可假设。
pub const META_FIELD: &str = "_meta";

/// params 未知字段显式拒绝（除 `_meta`）。
pub trait ParamsExt {
    fn extra(&self) -> &BTreeMap<String, Value>;

    fn reject_unknown(&self, method: &str) -> Result<(), String> {
        let unknown: Vec<&String> = self
            .extra()
            .keys()
            .filter(|key| key.as_str() != META_FIELD)
            .collect();
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "unsupported params fields for `{method}`: {}",
                unknown
                    .iter()
                    .map(|key| key.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ))
        }
    }
}

/// 客户端实现信息（clientInfo / agentInfo）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
}

/// 客户端文件系统能力（`clientCapabilities.fs`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemCapabilities {
    #[serde(default)]
    pub read_text_file: bool,
    #[serde(default)]
    pub write_text_file: bool,
}

/// 客户端能力。未知能力字段进入 `extra`，由协商层降级记录。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs: Option<FileSystemCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<bool>,
    /// elicitation 能力对象（form/url）；首轮不做结构化解析，仅识别是否存在。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elicitation: Option<Value>,
    /// session 配置能力对象（`configOptions.boolean`）；首轮仅识别是否存在。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// prompt 内容能力（`agentCapabilities.promptCapabilities`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCapabilities {
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub embedded_context: bool,
}

/// MCP 传输能力（`agentCapabilities.mcpCapabilities`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCapabilities {
    #[serde(default)]
    pub http: bool,
    #[serde(default)]
    pub sse: bool,
}

/// 空能力对象（如 `sessionCapabilities.resume: {}`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EmptyCapability {}

/// agent session 能力（`agentCapabilities.sessionCapabilities`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<EmptyCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<EmptyCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<EmptyCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<EmptyCapability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<EmptyCapability>,
}

/// agent 认证能力（`agentCapabilities.auth`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAuthCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logout: Option<EmptyCapability>,
}

/// agent 能力（initialize 响应）。首轮声明：resume + close，其余不声明。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default)]
    pub prompt_capabilities: PromptCapabilities,
    #[serde(default)]
    pub mcp_capabilities: McpCapabilities,
    #[serde(default)]
    pub session_capabilities: SessionCapabilities,
    #[serde(default)]
    pub auth: AgentAuthCapabilities,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `initialize` 请求参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_capabilities: Option<ClientCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<Implementation>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for InitializeParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `initialize` 响应结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u16,
    pub agent_capabilities: AgentCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<Implementation>,
    #[serde(default)]
    pub auth_methods: Vec<Value>,
}

/// `session/new` 请求参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewParams {
    pub cwd: String,
    /// ACP v1 官方 schema 必填；缺失按 -32602 严格拒绝（host 未协商
    /// mcp 能力时非空数组在使用点显式拒绝，空数组放行）。
    pub mcp_servers: Vec<Value>,
    #[serde(default)]
    pub additional_directories: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for SessionNewParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `session/new` 响应结果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewResult {
    pub session_id: String,
}

/// `session/resume` 请求参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeParams {
    pub session_id: String,
    pub cwd: String,
    /// 官方 builder 缺省为空数组（与 `session/new` 的必填语义不同：resume
    /// 只是重新 claim 既有会话，客户端可省略）。
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
    #[serde(default)]
    pub additional_directories: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for SessionResumeParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `session/close` 请求参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCloseParams {
    pub session_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for SessionCloseParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `session/prompt` 请求参数。`prompt` 为原始 content block 数组，翻译层按
/// `type` 显式分派（首轮仅支持 `text`，其余类型显式拒绝）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptParams {
    pub session_id: String,
    pub prompt: Vec<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for SessionPromptParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `session/prompt` 响应结果（stop reason）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPromptResult {
    pub stop_reason: StopReason,
}

/// `session/cancel` 通知参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCancelParams {
    pub session_id: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl ParamsExt for SessionCancelParams {
    fn extra(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }
}

/// `session/update` 通知参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUpdateParams {
    pub session_id: String,
    pub update: SessionUpdate,
}

/// `session/request_permission` 请求参数（agent → client）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionParams {
    pub session_id: String,
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
}

/// 提供给客户端的权限选项。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

/// 权限选项种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

/// `$/cancel_request` 通知参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequestParams {
    pub request_id: JsonRpcId,
}

/// `session/update` 的 update 联合（首轮只发射四类；schema 判别值为 snake_case）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    #[serde(rename_all = "camelCase")]
    AgentMessageChunk {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        content: ContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    AgentThoughtChunk {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        content: ContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        tool_call_id: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ToolKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ToolCallStatus>,
    },
    #[serde(rename_all = "camelCase")]
    ToolCallUpdate {
        tool_call_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<ToolCallStatus>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ToolCallContent>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

/// content block（首轮只发射 text）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

/// tool call 内容（首轮只发射 `content` 包装的 text）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content { content: ContentBlock },
}

/// tool call 状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// tool 种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// `session/prompt` 的 stop reason。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

/// tool call 更新对象（`session/request_permission.toolCall` 复用）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
}
