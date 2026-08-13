//! Codex App Server ↔ canonical 显式映射表（P18-11）。
//!
//! 每条映射都有对应 golden/单元测试；未列入表内的 method / event / 字段一律
//! 显式拒绝。Core 侧类型一律来自 `core-api` / `agent-domain`，不发明 GUI frame。
//! subagent 血缘（`parentThreadId` / `forkedFromId`）必须原样保留。

use client_adapter_api::{AdapterError, AdapterErrorFrame};
use core_api::{AppEvent, AppResponse, AppResponseEnvelope, ApprovalDecision, RunState};
use serde_json::{json, Value};

use crate::wire::{
    ApprovalDecisionWire, CommandApprovalParams, JsonRpcError, ThreadObject, TurnObject,
    TurnStatus, UserInput, ERROR_INTERNAL, ERROR_INVALID_PARAMS, ERROR_INVALID_REQUEST,
    ERROR_METHOD_NOT_FOUND, ERROR_OVERLOADED, ERROR_OVERLOADED_MESSAGE,
};

/// Thread 血缘：fork 与 subagent 共用，禁止在映射中丢弃。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadLineage {
    pub parent_thread_id: Option<String>,
    pub forked_from_id: Option<String>,
}

/// Adapter 层错误 → JSON-RPC 错误码。
pub fn jsonrpc_code_for(error: &AdapterError) -> i32 {
    match error {
        AdapterError::ProtocolUnsupported(_) => ERROR_METHOD_NOT_FOUND,
        AdapterError::CapabilityUnsupported(_) | AdapterError::InvalidFrame(_) => {
            ERROR_INVALID_PARAMS
        }
        AdapterError::UnsupportedSchema { .. } => ERROR_INVALID_REQUEST,
        AdapterError::UnknownSession(_) | AdapterError::CoreSessionNotFound(_) => {
            ERROR_INVALID_PARAMS
        }
        AdapterError::SessionNotAttached(_) => ERROR_INVALID_PARAMS,
        AdapterError::SessionConflict(_)
        | AdapterError::RevisionExhausted(_)
        | AdapterError::StaleOwner { .. }
        | AdapterError::HostUnavailable(_) => ERROR_INTERNAL,
    }
}

pub fn jsonrpc_code_for_frame(frame: &AdapterErrorFrame) -> i32 {
    match frame.code.as_str() {
        "protocol_unsupported" => ERROR_METHOD_NOT_FOUND,
        "capability_unsupported"
        | "invalid_frame"
        | "session_not_attached"
        | "unknown_session"
        | "core_session_not_found" => ERROR_INVALID_PARAMS,
        "unsupported_schema" => ERROR_INVALID_REQUEST,
        _ => ERROR_INTERNAL,
    }
}

pub fn jsonrpc_error_for(error: &AdapterError) -> JsonRpcError {
    JsonRpcError::new(jsonrpc_code_for(error), error.to_string())
}

/// `-32001` 过载映射：仅匹配官方文案/码，不把其它 HostUnavailable 伪装成 overload。
pub fn overloaded_error() -> JsonRpcError {
    JsonRpcError::new(ERROR_OVERLOADED, ERROR_OVERLOADED_MESSAGE)
}

/// 从 `turn/start` / `turn/steer` input 提取用户文本。
///
/// 仅支持 `text`；image/audio 等多媒体块携带未协商能力语义，显式失败。
pub fn extract_user_message(input: &[UserInput]) -> Result<String, AdapterError> {
    let mut parts: Vec<String> = Vec::new();
    for block in input {
        match block {
            UserInput::Text { text } => {
                if !text.trim().is_empty() {
                    parts.push(text.clone());
                }
            }
            UserInput::Image { .. }
            | UserInput::LocalImage { .. }
            | UserInput::Audio { .. }
            | UserInput::LocalAudio { .. } => {
                return Err(AdapterError::ProtocolUnsupported(
                    "turn input image/audio blocks are not supported".into(),
                ));
            }
            UserInput::Unknown => {
                return Err(AdapterError::InvalidFrame(
                    "unknown turn input block type".into(),
                ));
            }
        }
    }
    let message = parts.join("\n");
    if message.trim().is_empty() {
        return Err(AdapterError::InvalidFrame(
            "turn input must contain at least one non-empty text block".into(),
        ));
    }
    Ok(message)
}

/// 线协议审批 decision → canonical [`ApprovalDecision`]。
pub fn approval_decision(wire: &ApprovalDecisionWire) -> ApprovalDecision {
    match wire {
        ApprovalDecisionWire::Accept => ApprovalDecision::ApproveOnce,
        ApprovalDecisionWire::AcceptForSession => ApprovalDecision::ApproveForRun,
        ApprovalDecisionWire::Decline => ApprovalDecision::Deny,
        ApprovalDecisionWire::Cancel => ApprovalDecision::Cancel,
    }
}

/// 构造带血缘的 Thread 对象（fork / subagent 不得丢失 parent/forkedFrom）。
pub fn thread_object(thread_id: &str, lineage: &ThreadLineage) -> ThreadObject {
    ThreadObject {
        id: thread_id.to_string(),
        preview: Some(String::new()),
        parent_thread_id: lineage.parent_thread_id.clone(),
        forked_from_id: lineage.forked_from_id.clone(),
        session_id: Some(thread_id.to_string()),
    }
}

pub fn thread_result(thread_id: &str, lineage: &ThreadLineage) -> Value {
    json!({ "thread": thread_object(thread_id, lineage) })
}

pub fn turn_object(turn_id: &str, status: TurnStatus) -> TurnObject {
    TurnObject {
        id: turn_id.to_string(),
        status,
        items: Vec::new(),
        error: None,
    }
}

pub fn turn_result(turn_id: &str, status: TurnStatus) -> Value {
    json!({ "turn": turn_object(turn_id, status) })
}

/// canonical 命令响应 → JSON-RPC result。
pub fn response_to_result(
    method: &str,
    envelope: &AppResponseEnvelope,
    thread_id: Option<&str>,
    lineage: &ThreadLineage,
) -> Result<Value, AdapterError> {
    match &envelope.response {
        AppResponse::Error(context) => Err(AdapterError::InvalidFrame(context.message.clone())),
        AppResponse::Artifact { .. } => Err(AdapterError::InvalidFrame(
            "artifact responses are not supported on the Codex app-server channel".into(),
        )),
        AppResponse::Accepted { run_id, .. } => match method {
            "turn/start" => {
                let turn_id = run_id
                    .as_ref()
                    .map(|id| id.as_str().to_string())
                    .ok_or_else(|| {
                        AdapterError::InvalidFrame(
                            "RunStart response did not carry run_id (turn id)".into(),
                        )
                    })?;
                Ok(turn_result(&turn_id, TurnStatus::InProgress))
            }
            "turn/interrupt" | "thread/compact/start" | "thread/unsubscribe" => Ok(json!({})),
            other => Err(AdapterError::ProtocolUnsupported(format!(
                "no Codex result mapping for accepted response of `{other}`"
            ))),
        },
        AppResponse::Data(value) => {
            let session_id = value
                .get("session_id")
                .and_then(Value::as_str)
                .or(thread_id)
                .ok_or_else(|| {
                    AdapterError::InvalidFrame(
                        "session command response did not carry session_id".into(),
                    )
                })?;
            match method {
                "thread/start" | "thread/resume" | "thread/fork" => {
                    Ok(thread_result(session_id, lineage))
                }
                _ => Ok(value.clone()),
            }
        }
    }
}

/// `RunState` 终态 → `turn.status`。
pub fn turn_status_for(state: &RunState) -> Option<TurnStatus> {
    match state {
        RunState::Completed => Some(TurnStatus::Completed),
        RunState::Cancelled | RunState::Interrupted => Some(TurnStatus::Interrupted),
        RunState::Failed => Some(TurnStatus::Failed),
        _ => None,
    }
}

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

/// Core 事件 → Codex 通知 method + params。
///
/// `None` 表示该事件没有 Codex 线表示（宿主内部，不向客户端发射）。
/// GUI 事件（`gui_client_*`）明确拒绝混入本通道。
pub fn translate_event(
    event: &AppEvent,
    thread_id: &str,
) -> Result<Option<(String, Value)>, AdapterError> {
    match event {
        AppEvent::GuiClientConnected { .. } | AppEvent::GuiClientDisconnected { .. } => {
            Err(AdapterError::InvalidFrame(
                "GUI Connection Protocol frames must not mix into the Codex app-server channel"
                    .into(),
            ))
        }
        AppEvent::SessionChanged { session_id, .. } => Ok(Some((
            "thread/started".into(),
            json!({
                "thread": thread_object(session_id.as_str(), &ThreadLineage::default())
            }),
        ))),
        AppEvent::RunChanged { run_id, state } => {
            if let Some(status) = turn_status_for(state) {
                Ok(Some((
                    "turn/completed".into(),
                    json!({ "turn": turn_object(run_id.as_str(), status) }),
                )))
            } else if matches!(
                state,
                RunState::Created | RunState::PreparingContext | RunState::StreamingResponse
            ) {
                Ok(Some((
                    "turn/started".into(),
                    json!({ "turn": turn_object(run_id.as_str(), TurnStatus::InProgress) }),
                )))
            } else {
                Ok(None)
            }
        }
        AppEvent::AssistantDelta {
            run_id,
            message_id,
            delta,
        } => Ok(Some((
            "item/agentMessage/delta".into(),
            json!({
                "threadId": thread_id,
                "turnId": run_id.as_str(),
                "itemId": message_id.as_str(),
                "delta": delta,
            }),
        ))),
        AppEvent::ThinkingDelta {
            run_id,
            message_id,
            delta,
        } => Ok(Some((
            "item/reasoning/summaryTextDelta".into(),
            json!({
                "threadId": thread_id,
                "turnId": run_id.as_str(),
                "itemId": message_id.as_str(),
                "delta": delta,
            }),
        ))),
        AppEvent::ToolStarted {
            run_id,
            tool_call_id,
            name,
        } => Ok(Some((
            "item/started".into(),
            json!({
                "threadId": thread_id,
                "turnId": run_id.as_str(),
                "item": {
                    "id": tool_call_id.as_str(),
                    "type": "commandExecution",
                    "command": name,
                    "status": "inProgress",
                }
            }),
        ))),
        AppEvent::ToolOutput {
            run_id,
            tool_call_id,
            delta,
            ..
        } => Ok(Some((
            "item/commandExecution/outputDelta".into(),
            json!({
                "threadId": thread_id,
                "turnId": run_id.as_str(),
                "itemId": tool_call_id.as_str(),
                "delta": delta,
            }),
        ))),
        AppEvent::ToolCompleted {
            run_id,
            tool_call_id,
            success,
        } => Ok(Some((
            "item/completed".into(),
            json!({
                "threadId": thread_id,
                "turnId": run_id.as_str(),
                "item": {
                    "id": tool_call_id.as_str(),
                    "type": "commandExecution",
                    "status": if *success { "completed" } else { "failed" },
                }
            }),
        ))),
        AppEvent::ToolApprovalRequired { .. } => Err(AdapterError::InvalidFrame(
            "tool approval is a server-initiated JSON-RPC request; use approval_request".into(),
        )),
        _ => Ok(None),
    }
}

/// `ToolApprovalRequired` → server→client 审批请求参数。
pub fn approval_request(
    event: &AppEvent,
    thread_id: &str,
) -> Result<CommandApprovalParams, AdapterError> {
    match event {
        AppEvent::ToolApprovalRequired {
            run_id,
            tool_call_id,
            reason,
        } => Ok(CommandApprovalParams {
            thread_id: thread_id.into(),
            turn_id: run_id.as_str().to_string(),
            item_id: tool_call_id.as_str().to_string(),
            reason: Some(reason.clone()),
            command: None,
            cwd: None,
        }),
        other => Err(AdapterError::InvalidFrame(format!(
            "core event `{}` is not a tool approval",
            app_event_kind(other)
        ))),
    }
}

/// 手动压缩进度：`contextCompaction` item（禁止改发 legacy `thread/compacted`）。
pub fn context_compaction_item(thread_id: &str, turn_id: &str, item_id: &str) -> (String, Value) {
    (
        "item/started".into(),
        json!({
            "threadId": thread_id,
            "turnId": turn_id,
            "item": {
                "id": item_id,
                "type": "contextCompaction",
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{MessageId, RunId, ToolCallId};
    use core_api::AppEvent;

    #[test]
    fn lineage_is_preserved_on_thread_object() {
        let lineage = ThreadLineage {
            parent_thread_id: Some("thr_parent".into()),
            forked_from_id: Some("thr_source".into()),
        };
        let object = thread_object("thr_child", &lineage);
        assert_eq!(object.parent_thread_id.as_deref(), Some("thr_parent"));
        assert_eq!(object.forked_from_id.as_deref(), Some("thr_source"));
    }

    #[test]
    fn multimedia_input_is_explicitly_unsupported() {
        let error = extract_user_message(&[UserInput::Image {
            url: "data:image/png;base64,AAAA".into(),
        }])
        .expect_err("image input must fail");
        assert!(matches!(error, AdapterError::ProtocolUnsupported(_)));
    }

    #[test]
    fn assistant_delta_maps_to_agent_message_delta() {
        let event = AppEvent::AssistantDelta {
            run_id: RunId::from("turn_1"),
            message_id: MessageId::from("item_1"),
            delta: "hello".into(),
        };
        let (method, params) = translate_event(&event, "thr_1")
            .expect("ok")
            .expect("mapped");
        assert_eq!(method, "item/agentMessage/delta");
        assert_eq!(params["threadId"], "thr_1");
        assert_eq!(params["turnId"], "turn_1");
        assert_eq!(params["itemId"], "item_1");
        assert_eq!(params["delta"], "hello");
    }

    #[test]
    fn gui_events_are_rejected() {
        let event = AppEvent::GuiClientConnected {
            client_id: agent_domain::GuiClientId::from("gui-1"),
            connection_id: agent_domain::ConnectionId::from("conn-1"),
        };
        let error = translate_event(&event, "thr_1").expect_err("gui mix forbidden");
        assert!(matches!(error, AdapterError::InvalidFrame(_)));
    }

    #[test]
    fn approval_request_is_not_a_notification() {
        let event = AppEvent::ToolApprovalRequired {
            run_id: RunId::from("turn_1"),
            tool_call_id: ToolCallId::from("item_cmd"),
            reason: "run ls".into(),
        };
        let error = translate_event(&event, "thr_1").expect_err("must not notify");
        assert!(matches!(error, AdapterError::InvalidFrame(_)));
        let params = approval_request(&event, "thr_1").expect("approval params");
        assert_eq!(params.thread_id, "thr_1");
        assert_eq!(params.turn_id, "turn_1");
        assert_eq!(params.item_id, "item_cmd");
    }
}
