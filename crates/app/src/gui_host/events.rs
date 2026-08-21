use pawork_domain::{AgentEvent, AgentEventEnvelope};
use pawork_protocol::{AppEvent, CommandSource, DiagnosticLevel, RunState};

/// 选择要广播给 GUI 的 App 事件；其余事件仍持久化，只是不进实时流。
pub(in crate::gui_host) fn broadcast_event(envelope: &AgentEventEnvelope) -> Option<AppEvent> {
    let run = envelope.run_id.clone();
    Some(match &envelope.payload {
        AgentEvent::RunStarted { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Created,
        },
        AgentEvent::AssistantTextDelta { message_id, delta } => AppEvent::AssistantDelta {
            run_id: run,
            message_id: message_id.clone(),
            delta: delta.clone(),
        },
        AgentEvent::ToolCallStarted {
            tool_call_id,
            name,
        } => AppEvent::ToolStarted {
            run_id: run,
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
        },
        AgentEvent::ToolOutputDelta {
            tool_call_id,
            delta,
            ..
        } => AppEvent::ToolOutput {
            run_id: run,
            tool_call_id: tool_call_id.clone(),
            delta: delta.clone(),
            truncated: false,
            artifact_id: None,
        },
        AgentEvent::ToolApprovalRequested { .. } => {
            // Requested 现在在等待前 emit 并落盘；live 卡片仍由
            // on_pending 广播 ToolApprovalRequired，mapper 仍返回 None。
            return None;
        }
        AgentEvent::ToolExecutionCompleted { result, .. } => AppEvent::ToolCompleted {
            run_id: run,
            tool_call_id: result.tool_call_id.clone(),
            success: !result.is_error,
        },
        AgentEvent::RunCompleted { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Completed,
        },
        AgentEvent::RunCancelled { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Cancelled,
        },
        AgentEvent::RunFailed { .. } => AppEvent::RunChanged {
            run_id: run,
            state: RunState::Failed,
        },
        AgentEvent::Diagnostic { code, details } => map_diagnostic(code, details),
        _ => return None,
    })
}

fn map_diagnostic(code: &str, details: &serde_json::Value) -> AppEvent {
    if code.starts_with("degrade.") {
        let level = details
            .get("severity")
            .and_then(serde_json::Value::as_str)
            .map(diagnostic_level_from_str)
            .unwrap_or(DiagnosticLevel::Info);
        let message = details
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| details.to_string());
        return AppEvent::Diagnostic {
            level,
            code: code.to_string(),
            message,
        };
    }
    AppEvent::Diagnostic {
        level: DiagnosticLevel::Info,
        code: code.to_string(),
        message: details.to_string(),
    }
}

fn diagnostic_level_from_str(value: &str) -> DiagnosticLevel {
    match value {
        "info" => DiagnosticLevel::Info,
        "warning" => DiagnosticLevel::Warning,
        "error" => DiagnosticLevel::Error,
        _ => DiagnosticLevel::Info,
    }
}

// Pin: degrade.* Diagnostic codes stay on AppEvent::Diagnostic. Do not add a
// dedicated AppEvent arm; desktop projection currently ignores these codes.
const _: fn(&str, &serde_json::Value) -> AppEvent = map_diagnostic;

/// 幂等按 GUI 客户端列式隔离：command_id / key 保持原值，scope 单独成列。
pub(in crate::gui_host) fn client_scope_from_source(source: &CommandSource) -> String {
    match source {
        CommandSource::LocalGui { client_id } | CommandSource::RemoteGui { client_id, .. } => {
            client_id.as_str().to_string()
        }
        CommandSource::Automation => "automation".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pawork_domain::{EventId, EventSequence, RunId, SessionId, Timestamp};
    use serde_json::json;

    fn envelope(payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from("evt-1"),
            SessionId::from("sess-1"),
            RunId::from("run-1"),
            EventSequence::new(1),
            Timestamp::from_unix_millis(1),
            payload,
        )
    }

    #[test]
    fn degrade_diagnostic_uses_details_severity_and_message() {
        let event = broadcast_event(&envelope(AgentEvent::Diagnostic {
            code: "degrade.tasks_finish_failed".into(),
            details: json!({
                "severity": "error",
                "message": "persist failed",
                "kind": "tasks_finish_failed",
            }),
        }))
        .expect("mapped");
        assert_eq!(
            event,
            AppEvent::Diagnostic {
                level: DiagnosticLevel::Error,
                code: "degrade.tasks_finish_failed".into(),
                message: "persist failed".into(),
            }
        );
    }

    #[test]
    fn degrade_diagnostic_falls_back_when_keys_missing() {
        let details = json!({"task_id": "t1"});
        let event = broadcast_event(&envelope(AgentEvent::Diagnostic {
            code: "degrade.home_dir_fallback".into(),
            details: details.clone(),
        }))
        .expect("mapped");
        assert_eq!(
            event,
            AppEvent::Diagnostic {
                level: DiagnosticLevel::Info,
                code: "degrade.home_dir_fallback".into(),
                message: details.to_string(),
            }
        );
    }

    #[test]
    fn non_degrade_diagnostic_keeps_info_and_details_string() {
        let details = json!({"from": {"model": "a"}});
        let event = broadcast_event(&envelope(AgentEvent::Diagnostic {
            code: "model.switched".into(),
            details: details.clone(),
        }))
        .expect("mapped");
        assert_eq!(
            event,
            AppEvent::Diagnostic {
                level: DiagnosticLevel::Info,
                code: "model.switched".into(),
                message: details.to_string(),
            }
        );
    }
}
