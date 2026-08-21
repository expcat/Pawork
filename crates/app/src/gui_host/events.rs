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
        AgentEvent::Diagnostic { code, details } => AppEvent::Diagnostic {
            level: DiagnosticLevel::Info,
            code: code.clone(),
            message: details.to_string(),
        },
        _ => return None,
    })
}

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
