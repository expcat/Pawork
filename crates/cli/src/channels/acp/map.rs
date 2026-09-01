//! ACP ↔ canonical 显式映射表（P17-7）。
//!
//! 每条映射都有对应 golden/单元测试；未列入表内的 method / event / 状态一律显式
//! 拒绝或标记为宿主内部（不静默丢弃）。Core 侧类型一律来自 `pawork-protocol` / `pawork-domain`，
//! 客户端专有 JSON 不进入 Core。

use pawork_domain::{ErrorCategory, ErrorContext};
use pawork_protocol::adapter::{AdapterError, AdapterErrorFrame};
use pawork_protocol::{AppEvent, AppResponse, AppResponseEnvelope, ApprovalDecision};
use serde_json::Value;

use crate::channels::acp::wire::{
    ContentBlock, JsonRpcError, PermissionOption, PermissionOptionKind, RequestPermissionParams,
    SessionUpdate, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolKind, ERROR_AUTH_REQUIRED,
    ERROR_INTERNAL, ERROR_INVALID_PARAMS, ERROR_INVALID_REQUEST, ERROR_METHOD_NOT_FOUND,
    ERROR_REQUEST_CANCELLED, ERROR_RESOURCE_NOT_FOUND,
};

/// 首轮提供的权限选项（optionId 稳定，供 golden fixture 与客户端 UI 复用）。
pub const PERMISSION_OPTION_ALLOW_ONCE: &str = "allow-once";
pub const PERMISSION_OPTION_REJECT_ONCE: &str = "reject-once";

/// Adapter 层错误 → JSON-RPC 错误码（显式表）。
pub fn jsonrpc_code_for(error: &AdapterError) -> i32 {
    match error {
        AdapterError::ProtocolUnsupported(_) => ERROR_METHOD_NOT_FOUND,
        AdapterError::CapabilityUnsupported(_) | AdapterError::InvalidFrame(_) => {
            ERROR_INVALID_PARAMS
        }
        AdapterError::UnsupportedSchema { .. } => ERROR_INVALID_REQUEST,
        AdapterError::UnknownSession(_) | AdapterError::CoreSessionNotFound(_) => {
            ERROR_RESOURCE_NOT_FOUND
        }
        AdapterError::SessionNotAttached(_) => ERROR_INVALID_PARAMS,
        AdapterError::SessionConflict(_)
        | AdapterError::RevisionExhausted(_)
        | AdapterError::StaleOwner { .. }
        | AdapterError::HostUnavailable(_) => ERROR_INTERNAL,
    }
}

/// `CanonicalCoreFrame::Error(AdapterErrorFrame)` → JSON-RPC 错误码
/// （dispatch 后只剩 frame，无法反推原始 `AdapterError`）。
pub fn jsonrpc_code_for_frame(frame: &AdapterErrorFrame) -> i32 {
    match frame.code.as_str() {
        "protocol_unsupported" => ERROR_METHOD_NOT_FOUND,
        "capability_unsupported" | "invalid_frame" | "session_not_attached" => ERROR_INVALID_PARAMS,
        "unsupported_schema" => ERROR_INVALID_REQUEST,
        "unknown_session" | "core_session_not_found" => ERROR_RESOURCE_NOT_FOUND,
        _ => ERROR_INTERNAL,
    }
}

/// canonical `ErrorContext` → JSON-RPC 错误对象。
pub fn error_context_to_jsonrpc(context: &ErrorContext) -> JsonRpcError {
    let code = match context.category {
        ErrorCategory::NotFound => ERROR_RESOURCE_NOT_FOUND,
        ErrorCategory::Authentication | ErrorCategory::Authorization => ERROR_AUTH_REQUIRED,
        ErrorCategory::InvalidRequest => ERROR_INVALID_PARAMS,
        ErrorCategory::Cancelled => ERROR_REQUEST_CANCELLED,
        _ => ERROR_INTERNAL,
    };
    JsonRpcError::new(code, context.message.clone())
}

/// canonical 响应信封 → JSON-RPC result（Data 直通；Accepted 为 `{}`）。
pub fn response_to_result(envelope: &AppResponseEnvelope) -> Result<Value, JsonRpcError> {
    match &envelope.response {
        AppResponse::Data(value) => Ok(value.clone()),
        AppResponse::Accepted { .. } => Ok(serde_json::json!({})),
        AppResponse::Artifact { .. } => Err(JsonRpcError::new(
            ERROR_INTERNAL,
            "artifact responses are not supported on the ACP channel",
        )),
        AppResponse::Error(context) => Err(error_context_to_jsonrpc(context)),
    }
}

/// 从 ACP prompt content block 数组提取用户消息文本。
///
/// 首轮支持 `text` 块与 ACP v1 基线 `resource_link`（映射为 canonical 安全
/// 文本引用 `[name](uri)`，不拉取资源、不请求网络、不误要求 image/audio/
/// embeddedContext 能力）；其余或未知类型显式拒绝，并指出类型名与能力门控。
pub fn extract_user_message(prompt: &[Value]) -> Result<String, AdapterError> {
    let mut parts: Vec<String> = Vec::new();
    for block in prompt {
        let Some(block_type) = block.get("type").and_then(Value::as_str) else {
            return Err(AdapterError::InvalidFrame(
                "prompt content block is missing a string `type` field".into(),
            ));
        };
        match block_type {
            "text" => {
                let text = block.get("text").and_then(Value::as_str).ok_or_else(|| {
                    AdapterError::InvalidFrame(
                        "text content block must carry a string `text` field".into(),
                    )
                })?;
                parts.push(text.to_string());
            }
            "resource_link" => {
                let uri = block.get("uri").and_then(Value::as_str).ok_or_else(|| {
                    AdapterError::InvalidFrame(
                        "resource_link content block must carry a string `uri` field".into(),
                    )
                })?;
                if uri.trim().is_empty() {
                    return Err(AdapterError::InvalidFrame(
                        "resource_link `uri` must be non-empty".into(),
                    ));
                }
                // 安全映射：仅作文本引用拼入用户消息，Core 不据此访问资源。
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(uri);
                parts.push(format!("[{name}]({uri})"));
            }
            "image" | "audio" | "resource" => {
                return Err(AdapterError::InvalidFrame(format!(
                    "content block type `{block_type}` is not supported (prompt capability not negotiated)"
                )));
            }
            other => {
                return Err(AdapterError::InvalidFrame(format!(
                    "unknown content block type `{other}`"
                )));
            }
        }
    }
    let message = parts.join("\n");
    if message.trim().is_empty() {
        return Err(AdapterError::InvalidFrame(
            "prompt must contain at least one non-empty text block".into(),
        ));
    }
    Ok(message)
}

/// Core 事件 → ACP `session/update` 负载（事件回译）。
///
/// 返回 `None` 表示该事件没有 ACP v1 表示（宿主内部事件，不向客户端发射）。
pub fn translate_session_update(event: &AppEvent) -> Option<SessionUpdate> {
    match event {
        AppEvent::AssistantDelta {
            message_id, delta, ..
        } => Some(SessionUpdate::AgentMessageChunk {
            message_id: Some(message_id.as_str().to_string()),
            content: ContentBlock::Text {
                text: delta.clone(),
            },
        }),
        AppEvent::ThinkingDelta {
            message_id, delta, ..
        } => Some(SessionUpdate::AgentThoughtChunk {
            message_id: Some(message_id.as_str().to_string()),
            content: ContentBlock::Text {
                text: delta.clone(),
            },
        }),
        AppEvent::ToolStarted {
            tool_call_id, name, ..
        } => Some(SessionUpdate::ToolCall {
            tool_call_id: tool_call_id.as_str().to_string(),
            title: name.clone(),
            kind: Some(ToolKind::Other),
            status: Some(ToolCallStatus::Pending),
        }),
        AppEvent::ToolOutput {
            tool_call_id,
            delta,
            ..
        } => Some(SessionUpdate::ToolCallUpdate {
            tool_call_id: tool_call_id.as_str().to_string(),
            status: Some(ToolCallStatus::InProgress),
            content: Some(vec![ToolCallContent::Content {
                content: ContentBlock::Text {
                    text: delta.clone(),
                },
            }]),
            title: None,
        }),
        AppEvent::ToolCompleted {
            tool_call_id,
            success,
            ..
        } => Some(SessionUpdate::ToolCallUpdate {
            tool_call_id: tool_call_id.as_str().to_string(),
            status: Some(if *success {
                ToolCallStatus::Completed
            } else {
                ToolCallStatus::Failed
            }),
            content: None,
            title: None,
        }),
        // ToolApprovalRequired 由宿主转为 session/request_permission 请求；
        // RunChanged 终态由宿主转为 session/prompt 响应。两者都不作为 update 发射。
        AppEvent::ToolApprovalRequired { .. } | AppEvent::RunChanged { .. } => None,
        _ => None,
    }
}

/// 权限选项（首轮固定：allow-once / reject-once，均可映射到 canonical 决策）。
pub fn permission_options() -> Vec<PermissionOption> {
    vec![
        PermissionOption {
            option_id: PERMISSION_OPTION_ALLOW_ONCE.into(),
            name: "Allow once".into(),
            kind: PermissionOptionKind::AllowOnce,
        },
        PermissionOption {
            option_id: PERMISSION_OPTION_REJECT_ONCE.into(),
            name: "Reject".into(),
            kind: PermissionOptionKind::RejectOnce,
        },
    ]
}

/// 权限选项 id → canonical 审批决策（fail-closed：未知选项拒绝）。
pub fn decision_for_option(option_id: &str) -> Result<ApprovalDecision, AdapterError> {
    match option_id {
        PERMISSION_OPTION_ALLOW_ONCE => Ok(ApprovalDecision::ApproveOnce),
        PERMISSION_OPTION_REJECT_ONCE => Ok(ApprovalDecision::Deny),
        other => Err(AdapterError::InvalidFrame(format!(
            "unknown permission option `{other}` (host only offers allow-once and reject-once)"
        ))),
    }
}

/// `ToolApprovalRequired` 事件 → `session/request_permission` 参数。
pub fn permission_request(
    event: &AppEvent,
    client_session_id: &str,
) -> Result<RequestPermissionParams, AdapterError> {
    match event {
        AppEvent::ToolApprovalRequired {
            tool_call_id,
            reason,
            ..
        } => Ok(RequestPermissionParams {
            session_id: client_session_id.into(),
            tool_call: ToolCallUpdate {
                tool_call_id: tool_call_id.as_str().to_string(),
                title: Some(reason.clone()),
                kind: Some(ToolKind::Other),
                status: Some(ToolCallStatus::Pending),
            },
            options: permission_options(),
        }),
        other => Err(AdapterError::InvalidFrame(format!(
            "permission_request requires ToolApprovalRequired, got {}",
            app_event_kind(other)
        ))),
    }
}

/// 稳定的事件种类标签（snake_case，用于诊断与测试断言）。
pub fn app_event_kind(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::CoreReady { .. } => "core_ready",
        AppEvent::WorkspaceChanged { .. } => "workspace_changed",
        AppEvent::SessionChanged { .. } => "session_changed",
        AppEvent::RunChanged { .. } => "run_changed",
        AppEvent::AssistantDelta { .. } => "assistant_delta",
        AppEvent::ThinkingDelta { .. } => "thinking_delta",
        AppEvent::ToolStarted { .. } => "tool_started",
        AppEvent::ToolOutput { .. } => "tool_output",
        AppEvent::ToolApprovalRequired { .. } => "tool_approval_required",
        AppEvent::ToolCompleted { .. } => "tool_completed",
        AppEvent::DiffChanged { .. } => "diff_changed",
        AppEvent::TerminalOutput { .. } => "terminal_output",
        AppEvent::TerminalExited { .. } => "terminal_exited",
        AppEvent::AuthChanged { .. } => "auth_changed",
        AppEvent::ProviderStatus { .. } => "provider_status",
        AppEvent::PluginError { .. } => "plugin_error",
        AppEvent::Diagnostic { .. } => "diagnostic",
        AppEvent::GuiClientConnected { .. } => "gui_client_connected",
        AppEvent::GuiClientDisconnected { .. } => "gui_client_disconnected",
        AppEvent::QuotaChanged { .. } => "quota_changed",
        AppEvent::QuotaAlert { .. } => "quota_alert",
        AppEvent::TeamEvent { .. } => "team_event",
    }
}
