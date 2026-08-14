//! 把 `AgentEventEnvelope` 打到终端。
//!
//! 文本模式：`AssistantTextDelta` → stdout，thinking / 工具活动 / 失败 → stderr。
//! `--json`：每行一个信封 JSON，只写 stdout。

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, ContentPart, ToolCallId,
};
use pawork_engine::{AgentEventSink, EngineError};
use serde_json::Value;

pub struct TextSink {
    text: Mutex<String>,
    thinking_open: AtomicBool,
    tools: Mutex<HashMap<ToolCallId, ToolActivity>>,
}

#[derive(Clone, Debug, Default)]
struct ToolActivity {
    name: String,
    args: String,
    bytes: u64,
}

impl Default for TextSink {
    fn default() -> Self {
        Self {
            text: Mutex::new(String::new()),
            thinking_open: AtomicBool::new(false),
            tools: Mutex::new(HashMap::new()),
        }
    }
}

impl TextSink {
    #[allow(dead_code)]
    pub fn collected_text(&self) -> String {
        self.text.lock().expect("sink text mutex").clone()
    }
}

#[async_trait]
impl AgentEventSink for TextSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        match envelope.payload {
            AgentEvent::AssistantTextDelta { delta, .. } => {
                close_thinking(self)?;
                print!("{delta}");
                io::stdout()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
                self.text.lock().expect("sink text mutex").push_str(&delta);
            }
            AgentEvent::AssistantThinkingDelta { delta, .. } => {
                if !self.thinking_open.swap(true, Ordering::AcqRel) {
                    eprint!("thinking: ");
                }
                eprint!("{delta}");
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
            }
            AgentEvent::ToolCallStarted { tool_call_id, name } => {
                self.tools
                    .lock()
                    .expect("sink tools mutex")
                    .insert(tool_call_id, ToolActivity { name, ..Default::default() });
            }
            AgentEvent::ToolCallArgumentsDelta {
                tool_call_id,
                json_delta,
            } => {
                if let Some(activity) = self
                    .tools
                    .lock()
                    .expect("sink tools mutex")
                    .get_mut(&tool_call_id)
                {
                    activity.args.push_str(&json_delta);
                }
            }
            AgentEvent::ToolApprovalRequested {
                tool_call_id,
                reason,
            } => {
                close_thinking(self)?;
                let name = self
                    .tools
                    .lock()
                    .expect("sink tools mutex")
                    .get(&tool_call_id)
                    .map(|item| item.name.clone())
                    .unwrap_or_else(|| tool_call_id.as_str().to_string());
                eprintln!("? approve {name}: {reason}");
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
            }
            AgentEvent::ToolApprovalResponded {
                tool_call_id,
                decision,
                comment,
            } => {
                close_thinking(self)?;
                let name = self
                    .tools
                    .lock()
                    .expect("sink tools mutex")
                    .get(&tool_call_id)
                    .map(|item| item.name.clone())
                    .unwrap_or_else(|| tool_call_id.as_str().to_string());
                let mut line = format!("  → {name} {}", decision_label(decision));
                if let Some(comment) = comment {
                    if !comment.is_empty() {
                        line.push_str(&format!(" ({comment})"));
                    }
                }
                eprintln!("{line}");
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
            }
            AgentEvent::ToolOutputDelta {
                tool_call_id,
                delta,
                ..
            } => {
                if let Some(activity) = self
                    .tools
                    .lock()
                    .expect("sink tools mutex")
                    .get_mut(&tool_call_id)
                {
                    activity.bytes = activity.bytes.saturating_add(delta.len() as u64);
                }
            }
            AgentEvent::ToolExecutionCompleted {
                tool_call_id,
                result,
            } => {
                close_thinking(self)?;
                let activity = self
                    .tools
                    .lock()
                    .expect("sink tools mutex")
                    .remove(&tool_call_id);
                let name = activity
                    .as_ref()
                    .map(|item| item.name.as_str())
                    .or(result.tool_name.as_deref())
                    .unwrap_or(tool_call_id.as_str());
                let detail = activity
                    .as_ref()
                    .map(|item| tool_detail(&item.args))
                    .unwrap_or_default();
                let bytes = activity.as_ref().map(|item| item.bytes).unwrap_or(0)
                    + content_bytes(&result.content);
                let error = if result.is_error {
                    Some(error_text(&result.content).unwrap_or_else(|| "failed".into()))
                } else {
                    None
                };
                let line = format_tool_activity_line(
                    name,
                    &detail,
                    if result.is_error { None } else { Some(bytes) },
                    error.as_deref(),
                );
                eprintln!("{line}");
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
            }
            _ => {}
        }
        Ok(())
    }
}

pub struct JsonlSink;

#[async_trait]
impl AgentEventSink for JsonlSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        let line = serde_json::to_string(&envelope)
            .map_err(|error| EngineError::sink(error.to_string()))?;
        println!("{line}");
        io::stdout()
            .flush()
            .map_err(|error| EngineError::sink(error.to_string()))?;
        Ok(())
    }
}

fn close_thinking(sink: &TextSink) -> Result<(), EngineError> {
    if sink.thinking_open.swap(false, Ordering::AcqRel) {
        eprintln!();
        io::stderr()
            .flush()
            .map_err(|error| EngineError::sink(error.to_string()))?;
    }
    Ok(())
}

pub(crate) fn format_tool_activity_line(
    name: &str,
    detail: &str,
    bytes: Option<u64>,
    error: Option<&str>,
) -> String {
    let mark = if error.is_some() { "✗" } else { "⚙" };
    let mut line = if detail.is_empty() {
        format!("{mark} {name}")
    } else {
        format!("{mark} {name} {detail}")
    };
    if let Some(message) = error {
        line.push_str(&format!(" ({message})"));
    } else if let Some(n) = bytes {
        line.push_str(&format!(" ({})", format_size(n)));
    }
    line
}

fn decision_label(decision: ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::ApprovedOnce => "approved once",
        ApprovalDecision::ApprovedForRun => "approved for run",
        ApprovalDecision::Denied => "denied",
        ApprovalDecision::Cancelled => "cancelled",
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        let kb = bytes as f64 / 1024.0;
        if kb >= 10.0 {
            format!("{kb:.0}KB")
        } else {
            format!("{kb:.1}KB")
        }
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn tool_detail(raw_args: &str) -> String {
    let parsed: Value = serde_json::from_str(raw_args).unwrap_or(Value::Null);
    for key in ["path", "pattern"] {
        if let Some(value) = parsed.get(key).and_then(Value::as_str) {
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    String::new()
}

fn content_bytes(parts: &[ContentPart]) -> u64 {
    parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => text.text.len() as u64,
            _ => 0,
        })
        .sum()
}

fn error_text(parts: &[ContentPart]) -> Option<String> {
    parts.iter().find_map(|part| match part {
        ContentPart::Text(text) if !text.text.is_empty() => Some(text.text.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_success_line_with_path_and_size() {
        assert_eq!(
            format_tool_activity_line("read_file", "src/lib.rs", Some(1229), None),
            "⚙ read_file src/lib.rs (1.2KB)"
        );
        assert_eq!(
            format_tool_activity_line("find_files", "**/.gitkeep", Some(80), None),
            "⚙ find_files **/.gitkeep (80B)"
        );
    }

    #[test]
    fn formats_failure_line() {
        assert_eq!(
            format_tool_activity_line(
                "read_file",
                "../..",
                None,
                Some("absolute path not allowed"),
            ),
            "✗ read_file ../.. (absolute path not allowed)"
        );
    }

    #[test]
    fn decision_labels_are_stable() {
        assert_eq!(
            decision_label(ApprovalDecision::ApprovedOnce),
            "approved once"
        );
        assert_eq!(decision_label(ApprovalDecision::Denied), "denied");
    }

    #[test]
    fn tool_detail_prefers_path_then_pattern() {
        assert_eq!(
            tool_detail(r#"{"path":"Pawork_v2/ROADMAP.md","offset":1}"#),
            "Pawork_v2/ROADMAP.md"
        );
        assert_eq!(
            tool_detail(r#"{"pattern":"CURRENT_SCHEMA_VERSION"}"#),
            "CURRENT_SCHEMA_VERSION"
        );
        assert_eq!(tool_detail("{"), "");
    }
}
