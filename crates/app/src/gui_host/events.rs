use pawork_domain::{AgentEvent, AgentEventEnvelope, CommandId};
use pawork_protocol::{AppCommandEnvelope, AppEvent, CommandSource, DiagnosticLevel, RunState};

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
            // Live 卡片由 GuiApprovalHost::decide 注册时广播；engine 在
            // decide 返回后才发 Requested/Responded 对，再映射会把已决
            // 策的卡片重新点亮。
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

/// 幂等按 GUI 客户端隔离：各连接独立生成 `gui-cmd-N`，不得把 A 的
/// SessionCreate 重放成 B 的 RunCancel。
pub(in crate::gui_host) fn scoped_idempotency(
    envelope: &AppCommandEnvelope,
) -> (CommandId, Option<String>) {
    let client_id = match &envelope.source {
        CommandSource::LocalGui { client_id } | CommandSource::RemoteGui { client_id, .. } => {
            Some(client_id.as_str())
        }
        _ => None,
    };
    match client_id {
        Some(client_id) => (
            CommandId::from(format!("{client_id}/{}", envelope.command_id.as_str())),
            envelope
                .idempotency_key
                .as_ref()
                .map(|key| format!("{client_id}/{key}")),
        ),
        None if matches!(envelope.source, CommandSource::Automation) => (
            CommandId::from(format!("automation/{}", envelope.command_id.as_str())),
            envelope
                .idempotency_key
                .as_ref()
                .map(|key| format!("automation/{key}")),
        ),
        None => (
            envelope.command_id.clone(),
            envelope.idempotency_key.clone(),
        ),
    }
}
