//! 把 `AgentEventEnvelope` 打到终端。
//!
//! 文本模式：`AssistantTextDelta` → stdout，thinking / 工具活动 / 失败 → stderr。
//! `--json` 由 chat/run/headless 走 `HeadlessResponse` JSONL，不经本模块。

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, ContentPart, ToolCallId, ToolOutputStream,
    ToolResultContent,
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
    started: bool,
    stderr_opened: bool,
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
                self.tools.lock().expect("sink tools mutex").insert(
                    tool_call_id,
                    ToolActivity {
                        name,
                        ..Default::default()
                    },
                );
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
            AgentEvent::ToolExecutionStarted { tool_call_id } => {
                close_thinking(self)?;
                let mut tools = self.tools.lock().expect("sink tools mutex");
                if let Some(activity) = tools.get_mut(&tool_call_id) {
                    if activity.name == "run_command" && !activity.started {
                        activity.started = true;
                        let detail = tool_detail(&activity.args);
                        eprintln!("{}", format_command_cancel_hint(&activity.name, &detail));
                        io::stderr()
                            .flush()
                            .map_err(|error| EngineError::sink(error.to_string()))?;
                    }
                }
            }
            AgentEvent::ToolOutputDelta {
                tool_call_id,
                stream,
                delta,
            } => {
                close_thinking(self)?;
                let color = io::stderr().is_terminal();
                let mut prefix = String::new();
                {
                    let mut tools = self.tools.lock().expect("sink tools mutex");
                    if let Some(activity) = tools.get_mut(&tool_call_id) {
                        activity.bytes = activity.bytes.saturating_add(delta.len() as u64);
                        if stream == ToolOutputStream::Stderr && !activity.stderr_opened {
                            activity.stderr_opened = true;
                            prefix.push_str("[stderr]\n");
                        }
                    } else if stream == ToolOutputStream::Stderr {
                        prefix.push_str("[stderr]\n");
                    }
                }
                eprint!("{}{}", prefix, paint_stream(stream, &delta, color));
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
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
                if result_truncated(&result) {
                    eprintln!("{}", TRUNCATED_LINE);
                }
                if let Some(notice) = sandbox_fallback_notice(&result.metadata) {
                    eprintln!("{notice}");
                }
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
            AgentEvent::Diagnostic { code, details } if code == "sandbox.fallback" => {
                close_thinking(self)?;
                let notice = diagnostic_sandbox_fallback_notice(&details);
                eprintln!("{notice}");
                io::stderr()
                    .flush()
                    .map_err(|error| EngineError::sink(error.to_string()))?;
            }
            _ => {}
        }
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

const TRUNCATED_LINE: &str = "已截断";

pub(crate) fn format_command_cancel_hint(name: &str, detail: &str) -> String {
    let body = if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name} {detail}")
    };
    format!("⚙ {body}  （Ctrl-C 取消当轮）")
}

pub(crate) fn paint_stream(stream: ToolOutputStream, delta: &str, color: bool) -> String {
    match stream {
        ToolOutputStream::Stderr if color => format!("\x1b[31m{delta}\x1b[0m"),
        _ => delta.to_string(),
    }
}

fn result_truncated(result: &ToolResultContent) -> bool {
    result
        .metadata
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn sandbox_fallback_notice(metadata: &Value) -> Option<String> {
    let sandbox = metadata
        .get("sandbox")
        .or_else(|| metadata.get("fallback").and_then(|_| Some(metadata)))?;
    let fallback = sandbox.get("fallback").and_then(Value::as_bool)?;
    if !fallback {
        return None;
    }
    let isolation = sandbox
        .get("isolation")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let backend = sandbox
        .get("backend")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let note = sandbox.get("note").and_then(Value::as_str).unwrap_or("");
    Some(if note.is_empty() {
        format!("沙箱回退：isolation={isolation} backend={backend}")
    } else {
        format!("沙箱回退：isolation={isolation} backend={backend}（{note}）")
    })
}

/// Diagnostic `sandbox.fallback` 的 notice 决策（纯函数）：message 优先直传，
/// 其次 fallback 元数据，最后落「隔离已降级」默认串。
fn diagnostic_sandbox_fallback_notice(details: &Value) -> String {
    details
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| sandbox_fallback_notice(details))
        .unwrap_or_else(|| "沙箱回退：隔离已降级".into())
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
    for key in ["path", "pattern", "command"] {
        if let Some(value) = parsed.get(key).and_then(Value::as_str) {
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    if let Some(argv) = parsed.get("argv").and_then(Value::as_array) {
        let parts: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
        if !parts.is_empty() {
            return parts.join(" ");
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

    #[test]
    fn tool_detail_reads_command_and_argv() {
        assert_eq!(
            tool_detail(r#"{"command":"cargo test -p pawork-exec"}"#),
            "cargo test -p pawork-exec"
        );
        assert_eq!(
            tool_detail(r#"{"argv":["cargo","test","-p","pawork-exec"]}"#),
            "cargo test -p pawork-exec"
        );
    }

    #[test]
    fn command_cancel_hint_includes_ctrl_c() {
        assert_eq!(
            format_command_cancel_hint("run_command", "cargo test"),
            "⚙ run_command cargo test  （Ctrl-C 取消当轮）"
        );
    }

    #[test]
    fn paint_stderr_uses_red_only_when_color_enabled() {
        assert_eq!(
            paint_stream(ToolOutputStream::Stderr, "boom\n", true),
            "\x1b[31mboom\n\x1b[0m"
        );
        assert_eq!(
            paint_stream(ToolOutputStream::Stderr, "boom\n", false),
            "boom\n"
        );
        assert_eq!(paint_stream(ToolOutputStream::Stdout, "ok\n", true), "ok\n");
    }

    #[test]
    fn truncated_metadata_is_detected() {
        let result = ToolResultContent {
            tool_call_id: ToolCallId::from("t1"),
            tool_name: Some("run_command".into()),
            content: Vec::new(),
            is_error: false,
            metadata: serde_json::json!({"truncated": true}),
            artifacts: Vec::new(),
        };
        assert!(result_truncated(&result));
        assert_eq!(TRUNCATED_LINE, "已截断");
    }

    #[test]
    fn sandbox_fallback_metadata_is_detected() {
        assert_eq!(
            sandbox_fallback_notice(&serde_json::json!({
                "sandbox": {
                    "fallback": true,
                    "isolation": "soft",
                    "backend": "native_restricted",
                    "note": "seatbelt unavailable"
                }
            })),
            Some(
                "沙箱回退：isolation=soft backend=native_restricted（seatbelt unavailable）".into()
            )
        );
        assert_eq!(
            sandbox_fallback_notice(&serde_json::json!({
                "sandbox": { "fallback": false, "isolation": "hard" }
            })),
            None
        );
    }

    #[test]
    fn sandbox_fallback_notice_omits_empty_note() {
        assert_eq!(
            sandbox_fallback_notice(&serde_json::json!({
                "sandbox": {
                    "fallback": true,
                    "isolation": "soft",
                    "backend": "native_restricted",
                    "note": ""
                }
            })),
            Some("沙箱回退：isolation=soft backend=native_restricted".into())
        );
        assert_eq!(
            sandbox_fallback_notice(&serde_json::json!({
                "sandbox": {
                    "fallback": true,
                    "isolation": "soft",
                    "backend": "native_restricted"
                }
            })),
            Some("沙箱回退：isolation=soft backend=native_restricted".into())
        );
    }

    #[test]
    fn sandbox_fallback_notice_supports_legacy_diagnostic_shape() {
        // 旧版 Diagnostic details 直接携带 fallback 字段（无 sandbox 包裹）。
        assert_eq!(
            sandbox_fallback_notice(&serde_json::json!({
                "fallback": true,
                "isolation": "soft",
                "backend": "native_restricted"
            })),
            Some("沙箱回退：isolation=soft backend=native_restricted".into())
        );
    }

    /// Diagnostic `sandbox.fallback` 分支：message 优先直传；无 message 且无
    /// fallback 元数据时走「沙箱回退：隔离已降级」默认串（该字面量与 protocol
    /// 投影 golden 同源，由 protocol 侧精确钉死）。
    /// Diagnostic 路径输出走 stderr 不进 sink 文本，故直接断言 notice 决策纯函数
    ///（emit 分支消费同一函数，两条分支语义在此钉死）。
    #[test]
    fn diagnostic_sandbox_fallback_renders_message_or_default() {
        assert_eq!(
            diagnostic_sandbox_fallback_notice(&serde_json::json!({
                "message": "自定义回退说明"
            })),
            "自定义回退说明"
        );
        assert_eq!(
            diagnostic_sandbox_fallback_notice(&serde_json::json!({})),
            "沙箱回退：隔离已降级"
        );
    }
}
