//! SDK 层 permission / subagent / task / hook 可观察事件映射（P18-12 §3）。
//!
//! Adapter 只做显式翻译：permission 决策仍由 Core policy 做出（adapter 不
//! 批准、不拒绝任何工具调用），subagent / task / hook 事件只映射为可观察
//! canonical 边界。SDK 类型是内部线协议，本模块只读取已知字段；未知
//! control 子类型显式失败（权限语义不允许静默丢弃）。

use agent_domain::ToolCallId;
use serde_json::{json, Value};

use crate::error::ClaudeGatewayError;
use crate::stream::GatewayEvent;
use crate::wire::ClaudeStreamEvent;

/// Claude 客户端侧的可观察事件（权限 / 生命周期 / 任务 / hook / 取消）。
#[derive(Clone, Debug, PartialEq)]
pub enum ControlEvent {
    /// 客户端工具权限请求（对应 canonical `ToolApprovalRequested`）。
    PermissionRequested {
        request_id: String,
        tool_name: String,
        input: Value,
        /// 已关联的 tool call id（能关联时携带）。
        tool_call_id: Option<ToolCallId>,
    },
    /// 客户端对权限请求的决策（最终裁决仍归 Core policy）。
    PermissionDecided {
        request_id: String,
        decision: GatewayPermissionDecision,
    },
    /// subagent 启动观察（session / agent / parent 来自线协议 payload 或身份头）。
    SubagentStarted {
        session_id: Option<String>,
        agent_id: Option<String>,
        parent_agent_id: Option<String>,
    },
    /// subagent 停止观察。
    SubagentStopped {
        session_id: Option<String>,
        agent_id: Option<String>,
        status: Option<String>,
    },
    /// task 启动观察。
    TaskStarted {
        task_name: Option<String>,
        agent_id: Option<String>,
    },
    /// task 完成观察。
    TaskCompleted {
        task_name: Option<String>,
        agent_id: Option<String>,
        status: Option<String>,
    },
    /// hook 生命周期观察（hook_name 保留，未知字段不进入 canonical）。
    HookObserved {
        hook_name: String,
        trigger: Option<String>,
        status: Option<String>,
    },
    /// SDK 提交的 tool_result（Messages 请求侧 tool_result 的同源观察）。
    ToolResultSubmitted { tool_use_id: String, is_error: bool },
    /// SDK 运行结果消息。
    RunResult { result_type: Option<String> },
    /// 流被取消 / 中断（断流、cancel）。
    Interrupted { reason: Option<String> },
}

/// 客户端权限决策的显式翻译。
///
/// 语义：`allow` → 单次批准（最小授权，`ApprovedForRun` 需 Core policy 显式
/// 升级）；`deny` → 拒绝；SDK error / 取消 → 取消。`ask` 需要交互决策，
/// adapter 不参与，显式失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayPermissionDecision {
    Allowed,
    Denied,
    Cancelled,
}

impl GatewayPermissionDecision {
    /// 翻译为 canonical 决策（agent-events 词汇）。`Allowed` 只映射为单次批准。
    pub fn to_canonical(self) -> Result<agent_events::ApprovalDecision, ClaudeGatewayError> {
        match self {
            GatewayPermissionDecision::Allowed => Ok(agent_events::ApprovalDecision::ApprovedOnce),
            GatewayPermissionDecision::Denied => Ok(agent_events::ApprovalDecision::Denied),
            GatewayPermissionDecision::Cancelled => Ok(agent_events::ApprovalDecision::Cancelled),
        }
    }
}

fn opt_string(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

/// `control_request` → 权限请求观察。未知子类型显式失败（fail-closed）。
pub fn map_control_request(
    event: &ClaudeStreamEvent,
) -> Result<Vec<GatewayEvent>, ClaudeGatewayError> {
    let ClaudeStreamEvent::ControlRequest {
        request_id,
        subtype,
        data,
    } = event
    else {
        return Ok(Vec::new());
    };
    match subtype.as_str() {
        "can_use_tool" => {
            let tool_name = data
                .get("tool_name")
                .and_then(Value::as_str)
                .ok_or(ClaudeGatewayError::MalformedEvent(
                    "control_request".into(),
                    "tool_name",
                ))?
                .to_string();
            let input = data.get("input").cloned().unwrap_or(Value::Null);
            let tool_call_id = data
                .get("tool_use_id")
                .and_then(Value::as_str)
                .map(ToolCallId::from);
            Ok(vec![GatewayEvent::Control(
                ControlEvent::PermissionRequested {
                    request_id: request_id.clone(),
                    tool_name,
                    input,
                    tool_call_id,
                },
            )])
        }
        _ => Err(ClaudeGatewayError::UnsupportedEvent(
            "control_request".into(),
            "unknown subtype (permission semantics fail closed)",
        )),
    }
}

/// `control_response` → 权限决策观察。`ask` / 未知行为显式失败。
pub fn map_control_response(
    event: &ClaudeStreamEvent,
) -> Result<Vec<GatewayEvent>, ClaudeGatewayError> {
    let ClaudeStreamEvent::ControlResponse {
        request_id,
        subtype,
        data,
    } = event
    else {
        return Ok(Vec::new());
    };
    let decision = match subtype.as_str() {
        "success" => {
            let behavior = data
                .get("response")
                .and_then(|response| response.get("behavior"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ClaudeGatewayError::MalformedEvent("control_response".into(), "behavior")
                })?;
            match behavior {
                "allow" => GatewayPermissionDecision::Allowed,
                "deny" => GatewayPermissionDecision::Denied,
                "ask" => {
                    return Err(ClaudeGatewayError::UnsupportedEvent(
                        "control_response".into(),
                        "permission behavior `ask` requires interactive decision",
                    ));
                }
                _ => {
                    return Err(ClaudeGatewayError::UnsupportedEvent(
                        "control_response".into(),
                        "unknown permission behavior",
                    ));
                }
            }
        }
        // SDK 处理失败 → 取消该次请求（fail-closed，不落入 allow）。
        "error" => GatewayPermissionDecision::Cancelled,
        _ => {
            return Err(ClaudeGatewayError::UnsupportedEvent(
                "control_response".into(),
                "unknown response subtype",
            ));
        }
    };
    Ok(vec![GatewayEvent::Control(
        ControlEvent::PermissionDecided {
            request_id: request_id.clone(),
            decision,
        },
    )])
}

/// `hook_event` → subagent / task / hook 可观察事件。
///
/// hook_name 与 Claude Code hook 词汇对齐（SubagentStart / SubagentStop /
/// TaskStart / TaskComplete / 其余为通用 HookObserved）；payload 中只读取
/// 已知标量键，未知字段不进入 canonical 事件。
pub fn map_hook_event(event: &Value) -> Vec<GatewayEvent> {
    let hook_name = event
        .get("hook_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match hook_name {
        "SubagentStart" => vec![GatewayEvent::Control(ControlEvent::SubagentStarted {
            session_id: opt_string(event, "session_id"),
            agent_id: opt_string(event, "agent_id"),
            parent_agent_id: opt_string(event, "parent_agent_id"),
        })],
        "SubagentStop" => vec![GatewayEvent::Control(ControlEvent::SubagentStopped {
            session_id: opt_string(event, "session_id"),
            agent_id: opt_string(event, "agent_id"),
            status: opt_string(event, "status"),
        })],
        "TaskStart" => vec![GatewayEvent::Control(ControlEvent::TaskStarted {
            task_name: opt_string(event, "task_name"),
            agent_id: opt_string(event, "agent_id"),
        })],
        "TaskComplete" => vec![GatewayEvent::Control(ControlEvent::TaskCompleted {
            task_name: opt_string(event, "task_name"),
            agent_id: opt_string(event, "agent_id"),
            status: opt_string(event, "status"),
        })],
        other => vec![GatewayEvent::Control(ControlEvent::HookObserved {
            hook_name: other.to_string(),
            trigger: opt_string(event, "trigger"),
            status: opt_string(event, "status"),
        })],
    }
}

/// SDK `user` 消息（tool_result 提交）→ 可观察事件。未知块类型保留上报。
pub fn map_user_message(content: &[Value]) -> Vec<GatewayEvent> {
    let mut events = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                if let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) {
                    events.push(GatewayEvent::Control(ControlEvent::ToolResultSubmitted {
                        tool_use_id: tool_use_id.to_string(),
                        is_error: block
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }));
                }
            }
            Some(other) => events.push(GatewayEvent::Unmapped {
                event_type: format!("user_block:{other}"),
            }),
            None => events.push(GatewayEvent::Unmapped {
                event_type: "user_block:<missing>".into(),
            }),
        }
    }
    events
}

/// SDK assistant 消息 / `stream_event` 快照 → text / tool_use canonical 事件。
///
/// 快照模式（`includePartialMessages`）下与流式 delta 二选一，宿主只启用其一；
/// 全量 text → `TextDelta`，全量 tool_use → 完整生命周期，未知块保留上报。
pub fn map_assistant_snapshot(content: &[Value]) -> Vec<GatewayEvent> {
    let mut events = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        events.push(GatewayEvent::Stream(
                            provider_api::ProviderStreamEvent::TextDelta(text.to_string()),
                        ));
                    }
                }
            }
            Some("thinking") => {
                if let Some(thinking) = block.get("thinking").and_then(Value::as_str) {
                    if !thinking.is_empty() {
                        events.push(GatewayEvent::Stream(
                            provider_api::ProviderStreamEvent::ThinkingDelta(thinking.to_string()),
                        ));
                    }
                }
            }
            Some("tool_use") => {
                let (Some(id), Some(name)) = (
                    block.get("id").and_then(Value::as_str),
                    block.get("name").and_then(Value::as_str),
                ) else {
                    events.push(GatewayEvent::Unmapped {
                        event_type: "assistant_tool_use:<missing>".into(),
                    });
                    continue;
                };
                let id = ToolCallId::from(id);
                events.push(GatewayEvent::Stream(
                    provider_api::ProviderStreamEvent::ToolCallStarted {
                        id: id.clone(),
                        name: name.to_string(),
                    },
                ));
                if let Some(input) = block.get("input") {
                    if !input.is_null() {
                        let json = serde_json::to_string(input).unwrap_or_default();
                        if !json.is_empty() {
                            events.push(GatewayEvent::Stream(
                                provider_api::ProviderStreamEvent::ToolCallArgumentsDelta {
                                    id: id.clone(),
                                    json,
                                },
                            ));
                        }
                    }
                }
                events.push(GatewayEvent::Stream(
                    provider_api::ProviderStreamEvent::ToolCallCompleted { id },
                ));
            }
            Some(other) => events.push(GatewayEvent::Unmapped {
                event_type: format!("assistant_block:{other}"),
            }),
            None => events.push(GatewayEvent::Unmapped {
                event_type: "assistant_block:<missing>".into(),
            }),
        }
    }
    events
}

/// 把 Core 权限请求翻译为发往 Claude 客户端的 `control_request` 帧。
///
/// 请求只携带工具调用信息，不含任何决策；决策由客户端回报后仍经 Core
/// policy 裁决（见 [`GatewayPermissionDecision::to_canonical`]）。
pub fn encode_permission_request(request_id: &str, tool_name: &str, input: Value) -> Value {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": {
            "subtype": "can_use_tool",
            "tool_name": tool_name,
            "input": input,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::parse_event;

    #[test]
    fn can_use_tool_maps_to_permission_request() {
        let event = parse_event(
            r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"},"tool_use_id":"call-1"}}"#,
        )
        .expect("parse");
        let events = map_control_request(&event).expect("map");
        assert_eq!(
            events,
            vec![GatewayEvent::Control(ControlEvent::PermissionRequested {
                request_id: "req-1".into(),
                tool_name: "Bash".into(),
                input: json!({"command": "ls"}),
                tool_call_id: Some(ToolCallId::from("call-1")),
            })]
        );
    }

    #[test]
    fn unknown_control_subtype_fails_closed() {
        let event = parse_event(
            r#"{"type":"control_request","request_id":"req-2","request":{"subtype":"mcp_call","server":"x"}}"#,
        )
        .expect("parse");
        assert!(matches!(
            map_control_request(&event),
            Err(ClaudeGatewayError::UnsupportedEvent(_, _))
        ));
    }

    #[test]
    fn allow_deny_ask_behaviors_map_explicitly() {
        let allow = parse_event(
            r#"{"type":"control_response","response":{"request_id":"req-1","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"allow"}}}}"#,
        )
        .expect("parse");
        assert_eq!(
            map_control_response(&allow).expect("map"),
            vec![GatewayEvent::Control(ControlEvent::PermissionDecided {
                request_id: "req-1".into(),
                decision: GatewayPermissionDecision::Allowed,
            })]
        );
        assert_eq!(
            GatewayPermissionDecision::Allowed
                .to_canonical()
                .expect("canonical"),
            agent_events::ApprovalDecision::ApprovedOnce
        );

        let deny = parse_event(
            r#"{"type":"control_response","response":{"request_id":"req-1","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"deny"}}}}"#,
        )
        .expect("parse");
        assert_eq!(
            map_control_response(&deny).expect("map"),
            vec![GatewayEvent::Control(ControlEvent::PermissionDecided {
                request_id: "req-1".into(),
                decision: GatewayPermissionDecision::Denied,
            })]
        );

        let ask = parse_event(
            r#"{"type":"control_response","response":{"request_id":"req-1","response":{"subtype":"success","request_id":"req-1","response":{"behavior":"ask"}}}}"#,
        )
        .expect("parse");
        assert!(matches!(
            map_control_response(&ask),
            Err(ClaudeGatewayError::UnsupportedEvent(_, _))
        ));

        let sdk_error = parse_event(
            r#"{"type":"control_response","response":{"request_id":"req-1","response":{"subtype":"error","request_id":"req-1","response":{"error":"boom"}}}}"#,
        )
        .expect("parse");
        assert_eq!(
            map_control_response(&sdk_error).expect("map"),
            vec![GatewayEvent::Control(ControlEvent::PermissionDecided {
                request_id: "req-1".into(),
                decision: GatewayPermissionDecision::Cancelled,
            })]
        );
    }

    #[test]
    fn hook_events_map_subagent_task_and_generic() {
        let started = map_hook_event(&json!({
            "hook_name": "SubagentStart",
            "session_id": "sess-1",
            "agent_id": "agent-2",
            "parent_agent_id": "agent-1",
            "unmapped_field": {"should": "not leak"},
        }));
        assert_eq!(
            started,
            vec![GatewayEvent::Control(ControlEvent::SubagentStarted {
                session_id: Some("sess-1".into()),
                agent_id: Some("agent-2".into()),
                parent_agent_id: Some("agent-1".into()),
            })]
        );

        let task = map_hook_event(&json!({
            "hook_name": "TaskComplete",
            "task_name": "deploy",
            "status": "success",
        }));
        assert_eq!(
            task,
            vec![GatewayEvent::Control(ControlEvent::TaskCompleted {
                task_name: Some("deploy".into()),
                agent_id: None,
                status: Some("success".into()),
            })]
        );

        let generic = map_hook_event(&json!({
            "hook_name": "PreToolUse",
            "status": "running",
        }));
        assert_eq!(
            generic,
            vec![GatewayEvent::Control(ControlEvent::HookObserved {
                hook_name: "PreToolUse".into(),
                trigger: None,
                status: Some("running".into()),
            })]
        );
    }

    #[test]
    fn user_message_maps_tool_result_blocks() {
        let events = map_user_message(&[
            json!({"type": "tool_result", "tool_use_id": "call-9", "content": "ok"}),
            json!({"type": "tool_result", "tool_use_id": "call-10", "is_error": true}),
            json!({"type": "text", "text": "human note"}),
        ]);
        assert_eq!(
            events,
            vec![
                GatewayEvent::Control(ControlEvent::ToolResultSubmitted {
                    tool_use_id: "call-9".into(),
                    is_error: false,
                }),
                GatewayEvent::Control(ControlEvent::ToolResultSubmitted {
                    tool_use_id: "call-10".into(),
                    is_error: true,
                }),
                GatewayEvent::Unmapped {
                    event_type: "user_block:text".into(),
                },
            ]
        );
    }

    #[test]
    fn assistant_snapshot_maps_text_and_tool_lifecycle() {
        let events = map_assistant_snapshot(&[
            json!({"type": "text", "text": "hello"}),
            json!({"type": "tool_use", "id": "call-3", "name": "read", "input": {"path": "/a"}}),
        ]);
        assert_eq!(
            events,
            vec![
                GatewayEvent::Stream(provider_api::ProviderStreamEvent::TextDelta("hello".into())),
                GatewayEvent::Stream(provider_api::ProviderStreamEvent::ToolCallStarted {
                    id: ToolCallId::from("call-3"),
                    name: "read".into(),
                }),
                GatewayEvent::Stream(provider_api::ProviderStreamEvent::ToolCallArgumentsDelta {
                    id: ToolCallId::from("call-3"),
                    json: r#"{"path":"/a"}"#.into(),
                }),
                GatewayEvent::Stream(provider_api::ProviderStreamEvent::ToolCallCompleted {
                    id: ToolCallId::from("call-3"),
                }),
            ]
        );
    }

    #[test]
    fn permission_request_encoder_contains_no_decision() {
        let encoded = encode_permission_request("req-1", "Bash", json!({"command": "ls"}));
        assert_eq!(encoded["type"], "control_request");
        assert_eq!(encoded["request"]["subtype"], "can_use_tool");
        assert_eq!(encoded["request"]["tool_name"], "Bash");
        assert!(encoded.get("decision").is_none());
        assert!(encoded["request"].get("behavior").is_none());
    }
}
