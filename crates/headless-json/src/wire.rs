//! Headless JSON 协议帧类型。
//!
//! 所有帧都是单行 JSON（JSONL，`\n` 结尾，UTF-8）。Command / Query / Event
//! 帧直接承载 [`core_api::AppCommandEnvelope`] / [`AppQueryEnvelope`] /
//! [`AppEventEnvelope`]，不另造协议；`compat_*` 帧是本层定义的稳定协议入口，
//! 由 Host 接线映射到 `session-store` 的 compat 导入实现（P16-10）。

use core_api::{ApiVersion, AppCommandEnvelope, AppEventEnvelope, AppQueryEnvelope};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 单帧 JSON 载荷上限（字节）。防止损坏或恶意的行声明超大内容；
/// 超过上限的行在分配之前即被拒绝（`TooLarge`）。
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// 协议对外可协商的能力（与 GUI Connection Protocol 的 capabilities 正交）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdkCapability {
    /// Session 生命周期（create / open / fork）。
    Sessions,
    /// Run 生命周期（start / cancel / retry）。
    Runs,
    /// 事件流式订阅（`event` 帧）。
    Streaming,
    /// 外部会话 compat 导入入口。
    CompatImport,
    /// 导入历史查询入口。
    CompatHistory,
}

/// 客户端 → Host 请求帧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HeadlessRequest {
    /// 握手：声明客户端身份、支持版本与请求能力。
    Hello {
        client_name: String,
        client_version: String,
        supported_api_versions: Vec<ApiVersion>,
        capabilities: Vec<SdkCapability>,
    },
    /// 命令信封（直通 [`AppCommandEnvelope`]）。
    Command { envelope: AppCommandEnvelope },
    /// 查询信封（直通 [`AppQueryEnvelope`]）。
    Query { envelope: AppQueryEnvelope },
    /// compat 导入入口：把外部会话内容（Claude/Codex/Grok/Cursor）导入为
    /// canonical 事件。Host 接线映射到 `session-store::compat_import`。
    CompatImport {
        request_id: String,
        source: CompatSource,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<CompatImportOptions>,
    },
    /// 导入历史查询入口（分页）。
    CompatHistory {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
}

/// 握手请求（`hello` 帧的解析后形态）。由 Host 接线层消费，不进入
/// [`TranslatedRequest`] 分发路径；Host 返回 [`HeadlessResponse::HelloAck`]
/// 或显式错误帧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HelloRequest {
    pub client_name: String,
    pub client_version: String,
    pub supported_api_versions: Vec<ApiVersion>,
    pub capabilities: Vec<SdkCapability>,
}

impl From<HelloRequest> for HeadlessRequest {
    fn from(hello: HelloRequest) -> Self {
        HeadlessRequest::Hello {
            client_name: hello.client_name,
            client_version: hello.client_version,
            supported_api_versions: hello.supported_api_versions,
            capabilities: hello.capabilities,
        }
    }
}

impl HeadlessRequest {
    /// 尝试把请求帧解析为握手请求；非 `hello` 帧返回 `None`。
    pub fn as_hello(&self) -> Option<HelloRequest> {
        match self {
            HeadlessRequest::Hello {
                client_name,
                client_version,
                supported_api_versions,
                capabilities,
            } => Some(HelloRequest {
                client_name: client_name.clone(),
                client_version: client_version.clone(),
                supported_api_versions: supported_api_versions.clone(),
                capabilities: capabilities.clone(),
            }),
            _ => None,
        }
    }

    /// 请求关联 id（error 帧回填用）：compat 帧用 `request_id`，信封帧用
    /// 信封内的 command/query id；`hello` 无关联 id。
    pub fn request_id(&self) -> Option<String> {
        match self {
            HeadlessRequest::Hello { .. } => None,
            HeadlessRequest::Command { envelope } => Some(envelope.command_id.as_str().to_string()),
            HeadlessRequest::Query { envelope } => Some(envelope.request_id.as_str().to_string()),
            HeadlessRequest::CompatImport { request_id, .. }
            | HeadlessRequest::CompatHistory { request_id, .. } => Some(request_id.clone()),
        }
    }
}

/// Host → 客户端响应帧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HeadlessResponse {
    /// 握手成功。`negotiated` 是双方共同支持的协议版本；
    /// `granted` 是 Host 实际授予的能力子集。
    HelloAck {
        instance_id: String,
        negotiated: ApiVersion,
        granted: Vec<SdkCapability>,
    },
    /// 命令/查询响应信封（直通 [`AppResponseEnvelope`]）。
    Response {
        envelope: core_api::AppResponseEnvelope,
    },
    /// 事件信封（直通 [`AppEventEnvelope`]）。
    Event { envelope: AppEventEnvelope },
    /// compat 导入结果。
    CompatImportResult {
        request_id: String,
        report: CompatImportReport,
    },
    /// 导入历史分页结果。
    CompatHistoryResult {
        request_id: String,
        entries: Vec<CompatHistoryEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    /// 显式错误帧（unknown / unsupported / malformed 等）。
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        kind: ProtocolErrorKind,
        message: String,
    },
}

/// 显式协议错误类别。unknown / unsupported 都有独立类别，客户端可按类别
/// 决定降级路径（能力协商、版本升级或跳过）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorKind {
    /// 请求帧 `type` 无法识别（显式 unknown 错误）。
    UnknownRequestType,
    /// 请求的能力 Host 不支持。
    UnsupportedCapability,
    /// 请求的 api_version 与 Host 不兼容。
    IncompatibleApiVersion,
    /// 请求未经握手或握手失败。
    NotHandshaked,
    /// 帧不是合法 JSON 或结构不符。
    MalformedFrame,
    /// 帧超过 [`MAX_FRAME_BYTES`]。
    TooLarge,
    /// Host 拒绝了请求载荷（如 compat 导入命中 Secret / 解析或校验失败）。
    /// 与 unsupported 区分：能力存在，但请求内容被拒绝。
    CompatRejected,
    /// Host 事件流背压：订阅者落后，错过的事件数在 message 中显式给出
    /// （不静默丢弃；客户端可据此降级或补拉历史）。
    Backpressure,
    /// Host 内部错误（无业务决策泄漏）。
    Internal,
}

/// 协议层错误：类别 + 消息。所有 unknown / unsupported / malformed 情况
/// 都以显式错误帧返回，不静默忽略。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("{kind:?}: {message}")]
pub struct ProtocolError {
    pub kind: ProtocolErrorKind,
    pub message: String,
}

impl ProtocolError {
    pub fn new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn malformed(message: impl Into<String>) -> Self {
        Self::new(ProtocolErrorKind::MalformedFrame, message)
    }

    pub fn too_large(message: impl Into<String>) -> Self {
        Self::new(ProtocolErrorKind::TooLarge, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ProtocolErrorKind::UnsupportedCapability, message)
    }

    pub fn unknown_request(message: impl Into<String>) -> Self {
        Self::new(ProtocolErrorKind::UnknownRequestType, message)
    }
}

/// 外部会话来源（与 `session-store::compat_import::ExternalSource` 一一对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatSource {
    Claude,
    Codex,
    Grok,
    Cursor,
}

impl CompatSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Cursor => "cursor",
        }
    }
}

impl std::fmt::Display for CompatSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// compat 导入选项。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatImportOptions {
    /// 只校验与解析，不落库（dry run 返回完整报告但不持久化）。
    #[serde(default)]
    pub dry_run: bool,
}

/// 导入报告（协议面；Host 从 `session-store::CompatImportReport` 映射）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatImportReport {
    pub source: Option<CompatSource>,
    pub session_id: String,
    pub original_id: Option<String>,
    pub imported_events: usize,
    pub imported_messages: usize,
    pub imported_tool_calls: usize,
    pub imported_tool_results: usize,
    pub imported_usages: usize,
    pub imported_reviews: usize,
    pub raw_records: usize,
    pub deduplicated: bool,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub unknown_fields: std::collections::BTreeMap<String, String>,
}

/// 导入历史条目（协议面）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatHistoryEntry {
    pub session_id: String,
    pub source: CompatSource,
    pub original_id: Option<String>,
    pub imported_events: usize,
    pub imported_at_unix_ms: u64,
}

/// 解析后的请求：Host 接线层可直接分发的最小切片。
#[derive(Clone, Debug, PartialEq)]
pub enum TranslatedRequest {
    Command(AppCommandEnvelope),
    Query(AppQueryEnvelope),
    CompatImport(CompatImportRequest),
    CompatHistory(CompatHistoryQuery),
}

/// compat 导入请求（解包后的稳定入口）。
#[derive(Clone, Debug, PartialEq)]
pub struct CompatImportRequest {
    pub request_id: String,
    pub source: CompatSource,
    pub content: String,
    pub options: CompatImportOptions,
}

/// compat 历史查询（解包后的稳定入口）。
#[derive(Clone, Debug, PartialEq)]
pub struct CompatHistoryQuery {
    pub request_id: String,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}
