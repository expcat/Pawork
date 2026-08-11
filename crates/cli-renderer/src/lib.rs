//! CLI 输出渲染；JSON 路径保持单行且可稳定解析。

use app_service::ServiceResponse;
use core_api::{AppEvent, AppEventEnvelope};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn render(response: &ServiceResponse, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string(response).expect("response is serializable"),
        OutputFormat::Text if response.data.is_null() => response.message.clone(),
        OutputFormat::Text => format!("{}\n{}", response.message, response.data),
    }
}

/// 流式渲染单条应用事件（run / watch 模式从 EventHub 订阅后逐条调用）。
///
/// JSON 路径为单行完整信封；文本路径按事件类型给出人类可读行（增量类事件
/// 直接透出内容，便于 watch 拼成连续流）。
pub fn render_event(envelope: &AppEventEnvelope, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => serde_json::to_string(envelope).expect("event is serializable"),
        OutputFormat::Text => render_event_text(envelope),
    }
}

fn render_event_text(envelope: &AppEventEnvelope) -> String {
    let prefix = format!(
        "[#{} {}]",
        envelope.global_sequence.0,
        envelope.timestamp.as_unix_millis()
    );
    match &envelope.payload {
        AppEvent::CoreReady { handle } => {
            format!("{prefix} core ready: instance {}", handle.instance_id)
        }
        AppEvent::WorkspaceChanged {
            workspace_id,
            revision,
        } => format!("{prefix} workspace {workspace_id} changed (revision {revision})"),
        AppEvent::SessionChanged {
            session_id,
            revision,
        } => format!("{prefix} session {session_id} changed (revision {revision})"),
        AppEvent::RunChanged { run_id, state } => {
            format!("{prefix} run {run_id}: {state:?}")
        }
        AppEvent::AssistantDelta { delta, .. } => delta.clone(),
        AppEvent::ThinkingDelta { delta, .. } => format!("{prefix} thinking: {delta}"),
        AppEvent::ToolStarted {
            tool_call_id, name, ..
        } => format!("{prefix} tool {name} started ({tool_call_id})"),
        AppEvent::ToolOutput { delta, .. } => delta.clone(),
        AppEvent::ToolApprovalRequired {
            tool_call_id,
            reason,
            ..
        } => format!("{prefix} approval required for {tool_call_id}: {reason}"),
        AppEvent::ToolCompleted {
            tool_call_id,
            success,
            ..
        } => format!("{prefix} tool {tool_call_id} completed success={success}"),
        AppEvent::DiffChanged { workspace_id } => {
            format!("{prefix} diff changed in {workspace_id}")
        }
        AppEvent::TerminalOutput {
            terminal_session_id,
            delta,
        } => format!("{prefix} terminal {terminal_session_id}: {delta}"),
        AppEvent::AuthChanged {
            provider_id,
            authenticated,
        } => format!("{prefix} auth {provider_id} authenticated={authenticated}"),
        AppEvent::ProviderStatus {
            provider_id,
            status,
        } => {
            format!("{prefix} provider {provider_id}: {status:?}")
        }
        AppEvent::PluginError { plugin_id, error } => {
            format!("{prefix} plugin {plugin_id} error: {}", error.message)
        }
        AppEvent::Diagnostic {
            level,
            code,
            message,
            ..
        } => format!("{prefix} diagnostic {level:?} {code}: {message}"),
        AppEvent::GuiClientConnected {
            client_id,
            connection_id,
        } => format!("{prefix} gui client {client_id} connected ({connection_id})"),
        AppEvent::GuiClientDisconnected {
            client_id,
            connection_id,
        } => format!("{prefix} gui client {client_id} disconnected ({connection_id})"),
        AppEvent::QuotaChanged { view } => format!(
            "{prefix} quota changed: {tenant}/{account} (cache={cache})",
            tenant = view.scope.tenant_id.as_str(),
            account = view.scope.account_id,
            cache = if view.from_cache { "hit" } else { "miss" }
        ),
        AppEvent::QuotaAlert { alert } => format!(
            "{prefix} quota alert {}: {} (window={:?})",
            format!("{:?}", alert.severity).to_lowercase(),
            alert.message,
            alert.window
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_api::RunState;
    use serde_json::{json, Value};

    #[test]
    fn json_output_is_single_line_and_parseable() {
        let response = ServiceResponse {
            ok: true,
            kind: "status".into(),
            message: "ready".into(),
            data: json!({ "b": 2, "a": 1 }),
        };
        let output = render(&response, OutputFormat::Json);
        assert!(!output.contains('\n'));
        let decoded: Value = serde_json::from_str(&output).expect("parse JSON output");
        assert_eq!(decoded["kind"], "status");
    }

    #[test]
    fn event_json_is_single_line_and_parseable() {
        let envelope = event(RunState::StreamingResponse);
        let output = render_event(&envelope, OutputFormat::Json);
        assert!(!output.contains('\n'));
        let decoded: Value = serde_json::from_str(&output).expect("parse JSON event");
        assert_eq!(decoded["payload"]["type"], "run_changed");
    }

    #[test]
    fn event_text_is_human_readable_per_variant() {
        let envelope = event(RunState::StreamingResponse);
        let text = render_event(&envelope, OutputFormat::Text);
        assert!(text.contains("run run-1: StreamingResponse"));

        let delta = AppEventEnvelope {
            payload: AppEvent::AssistantDelta {
                run_id: "run-1".into(),
                message_id: "message-1".into(),
                delta: "hello".into(),
            },
            ..envelope.clone()
        };
        assert_eq!(render_event(&delta, OutputFormat::Text), "hello");
    }

    fn event(state: RunState) -> AppEventEnvelope {
        AppEventEnvelope {
            api_version: core_api::API_VERSION,
            instance_id: agent_domain::CoreInstanceId::from("instance-1"),
            event_id: agent_domain::EventId::from("event-1"),
            global_sequence: core_api::GlobalSequence(1),
            stream: core_api::EventStream::Run(agent_domain::RunId::from("run-1")),
            stream_sequence: 1,
            timestamp: agent_domain::Timestamp::from_unix_millis(1),
            source: core_api::EventSource::Core,
            payload: AppEvent::RunChanged {
                run_id: agent_domain::RunId::from("run-1"),
                state,
            },
        }
    }
}
