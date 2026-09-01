//! V2 现行 `--json` AgentEvent payload.type → Headless `AppEvent` tag 对照表。
//!
//! `--json` 目前在 stdout 写出磁盘/线上 [`pawork_domain::AgentEventEnvelope`]
//! （`payload.type` 为 AgentEvent 的 serde tag）。S10 收口波会把它对齐为
//! [`HeadlessResponse::Event`]，其 `envelope.payload` 使用本 crate
//! [`crate::AppEvent`] 的 serde tag。
//!
//! 本模块只记录对照关系，**不实现 CLI 转换器**。未列出的 AgentEvent 变体
//! 表示当前没有对应的 AppEvent 镜像（headless 事件帧不发该条）。

/// 一条 `--json` payload.type → Headless AppEvent tag 对照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonToHeadlessEventMap {
    /// `AgentEvent` 的 serde `type`（snake_case）。
    pub agent_event_type: &'static str,
    /// 对应 `AppEvent` 的 serde `type`；`None` 表示不对齐到 headless Event。
    pub app_event_tag: Option<&'static str>,
    pub note: &'static str,
}

/// V2 现行 `--json` AgentEvent → `HeadlessResponse::Event` 的 AppEvent tag。
pub const JSON_TO_HEADLESS_EVENT_MAP: &[JsonToHeadlessEventMap] = &[
    JsonToHeadlessEventMap {
        agent_event_type: "run_started",
        app_event_tag: Some("run_changed"),
        note: "Run 生命周期投影为 RunChanged(Created)",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "context_prepared",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "provider_request_started",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "usage_updated",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "assistant_text_delta",
        app_event_tag: Some("assistant_delta"),
        note: "流式正文",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "assistant_thinking_delta",
        app_event_tag: Some("thinking_delta"),
        note: "流式思考",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_call_started",
        app_event_tag: Some("tool_started"),
        note: "工具开始",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_call_arguments_delta",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_approval_requested",
        app_event_tag: Some("tool_approval_required"),
        note: "审批请求；GUI 直播卡片另有路径，headless 仍映射此 tag",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_approval_responded",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_execution_started",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_output_delta",
        app_event_tag: Some("tool_output"),
        note: "工具输出增量",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "tool_execution_completed",
        app_event_tag: Some("tool_completed"),
        note: "工具结束",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "message_committed",
        app_event_tag: None,
        note: "无 AppEvent 镜像（时间线投影不走 Event 帧）",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "provider_transcript_continued",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "server_tool",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "transcript_envelope",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "compaction_started",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "compaction_completed",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "checkpoint_created",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "checkpoint_rolled_back",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "run_completed",
        app_event_tag: Some("run_changed"),
        note: "Run 生命周期投影为 RunChanged(Completed)",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "run_cancelled",
        app_event_tag: Some("run_changed"),
        note: "Run 生命周期投影为 RunChanged(Cancelled)",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "run_failed",
        app_event_tag: Some("run_changed"),
        note: "Run 生命周期投影为 RunChanged(Failed)",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "plan",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "goal",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "task",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "automation",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "monitor",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "memory",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "review",
        app_event_tag: None,
        note: "无 AppEvent 镜像",
    },
    JsonToHeadlessEventMap {
        agent_event_type: "diagnostic",
        app_event_tag: Some("diagnostic"),
        note: "诊断事件 1:1",
    },
];

/// 从对照表查询 `--json` payload.type 对应的 AppEvent tag。
pub fn app_event_tag_for_json_type(agent_event_type: &str) -> Option<&'static str> {
    JSON_TO_HEADLESS_EVENT_MAP
        .iter()
        .find(|row| row.agent_event_type == agent_event_type)
        .and_then(|row| row.app_event_tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppEvent;
    use serde_json::Value;

    fn app_event_tags() -> Vec<String> {
        // 用 serde 形状取出全部 AppEvent tag，避免手写漂移。
        let samples = [
            serde_json::to_value(AppEvent::CoreReady {
                handle: crate::ApiHandle {
                    instance_id: pawork_domain::CoreInstanceId::from("i"),
                    api_version: crate::API_VERSION,
                },
            }),
            serde_json::to_value(AppEvent::WorkspaceChanged {
                workspace_id: pawork_domain::WorkspaceId::from("w"),
                revision: 1,
            }),
            serde_json::to_value(AppEvent::SessionChanged {
                session_id: pawork_domain::SessionId::from("s"),
                revision: 1,
            }),
            serde_json::to_value(AppEvent::RunChanged {
                run_id: pawork_domain::RunId::from("r"),
                state: crate::RunState::Created,
            }),
            serde_json::to_value(AppEvent::AssistantDelta {
                run_id: pawork_domain::RunId::from("r"),
                message_id: pawork_domain::MessageId::from("m"),
                delta: String::new(),
            }),
            serde_json::to_value(AppEvent::ThinkingDelta {
                run_id: pawork_domain::RunId::from("r"),
                message_id: pawork_domain::MessageId::from("m"),
                delta: String::new(),
            }),
            serde_json::to_value(AppEvent::ToolStarted {
                run_id: pawork_domain::RunId::from("r"),
                tool_call_id: pawork_domain::ToolCallId::from("t"),
                name: "n".into(),
            }),
            serde_json::to_value(AppEvent::ToolOutput {
                run_id: pawork_domain::RunId::from("r"),
                tool_call_id: pawork_domain::ToolCallId::from("t"),
                delta: String::new(),
                truncated: false,
                artifact_id: None,
            }),
            serde_json::to_value(AppEvent::ToolApprovalRequired {
                run_id: pawork_domain::RunId::from("r"),
                tool_call_id: pawork_domain::ToolCallId::from("t"),
                reason: String::new(),
            }),
            serde_json::to_value(AppEvent::ToolCompleted {
                run_id: pawork_domain::RunId::from("r"),
                tool_call_id: pawork_domain::ToolCallId::from("t"),
                success: true,
            }),
            serde_json::to_value(AppEvent::DiffChanged {
                workspace_id: pawork_domain::WorkspaceId::from("w"),
            }),
            serde_json::to_value(AppEvent::TerminalOutput {
                terminal_session_id: "term".into(),
                delta: String::new(),
            }),
            serde_json::to_value(AppEvent::AuthChanged {
                provider_id: pawork_domain::ProviderId::from("p"),
                state: crate::AuthChangeState::Removed,
            }),
            serde_json::to_value(AppEvent::ProviderStatus {
                provider_id: pawork_domain::ProviderId::from("p"),
                status: crate::ProviderStatus::Ready,
            }),
            serde_json::to_value(AppEvent::PluginError {
                plugin_id: pawork_domain::PluginId::from("pl"),
                error: pawork_domain::ErrorContext {
                    category: pawork_domain::ErrorCategory::Internal,
                    message: "e".into(),
                    retryable: false,
                    retry_after_ms: None,
                    diagnostics: Default::default(),
                },
            }),
            serde_json::to_value(AppEvent::Diagnostic {
                level: crate::DiagnosticLevel::Info,
                code: "c".into(),
                message: "m".into(),
            }),
            serde_json::to_value(AppEvent::GuiClientConnected {
                client_id: pawork_domain::GuiClientId::from("g"),
                connection_id: pawork_domain::ConnectionId::from("c"),
            }),
            serde_json::to_value(AppEvent::GuiClientDisconnected {
                client_id: pawork_domain::GuiClientId::from("g"),
                connection_id: pawork_domain::ConnectionId::from("c"),
            }),
        ];
        samples
            .into_iter()
            .map(|value| {
                let value = value.expect("serialize AppEvent");
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .expect("AppEvent tag")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn mapped_tags_are_real_app_event_tags() {
        let tags = app_event_tags();
        for row in JSON_TO_HEADLESS_EVENT_MAP {
            if let Some(tag) = row.app_event_tag {
                assert!(
                    tags.iter().any(|known| known == tag),
                    "unknown AppEvent tag `{tag}` for {}",
                    row.agent_event_type
                );
            }
        }
    }

    #[test]
    fn table_has_unique_agent_event_types_and_known_mappings() {
        let mut seen = std::collections::BTreeSet::new();
        for row in JSON_TO_HEADLESS_EVENT_MAP {
            assert!(
                seen.insert(row.agent_event_type),
                "duplicate agent_event_type {}",
                row.agent_event_type
            );
        }
        assert_eq!(
            app_event_tag_for_json_type("assistant_text_delta"),
            Some("assistant_delta")
        );
        assert_eq!(
            app_event_tag_for_json_type("run_completed"),
            Some("run_changed")
        );
        assert_eq!(
            app_event_tag_for_json_type("diagnostic"),
            Some("diagnostic")
        );
        assert_eq!(app_event_tag_for_json_type("context_prepared"), None);
        assert!(!JSON_TO_HEADLESS_EVENT_MAP.is_empty());
    }
}
