//! 审批宿主：CLI / `--json` / 测试注入决策；engine 只看到 [`ApprovalDecision`]。

use async_trait::async_trait;
use pawork_api::ToolRequest;
use pawork_domain::{ApprovalDecision, CancellationToken, ToolCallId};
use pawork_policy::{ApprovalMode, RiskLevel};
use pawork_tools::{ApprovalOutcome, ApprovalResolver};

/// 一次需要用户确认的写操作摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalAsk {
    pub tool_name: String,
    pub tool_call_id: ToolCallId,
    pub relative_path: Option<String>,
    pub message: String,
    pub risk: RiskLevel,
    pub preview: Option<String>,
}

/// 终端或无人值守通道。`DenyAllApprovals` 用于 `--json` 与缺省 fail-closed。
#[async_trait]
pub trait ApprovalPromptHost: Send + Sync {
    async fn decide(&self, ask: &ApprovalAsk, cancel: CancellationToken) -> ApprovalDecision;
}

/// 任何 AskUser 一律拒绝（无人值守 / 无 TTY）。
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAllApprovals;

#[async_trait]
impl ApprovalPromptHost for DenyAllApprovals {
    async fn decide(&self, _ask: &ApprovalAsk, _cancel: CancellationToken) -> ApprovalDecision {
        ApprovalDecision::Denied
    }
}

/// 已在 engine 问过的调用：满足 scheduler 的 AskUser，不再弹第二次。
pub(crate) struct PreApprovedResolver;

#[async_trait]
impl ApprovalResolver for PreApprovedResolver {
    fn can_resolve_policy_prompt(&self) -> bool {
        true
    }

    async fn resolve(&self, requests: &[ToolRequest]) -> Vec<ApprovalOutcome> {
        requests.iter().map(|_| ApprovalOutcome::Approved).collect()
    }
}

/// 解析 CLI kebab（兼收 serde snake_case）。
pub fn parse_approval_mode(value: &str) -> Result<ApprovalMode, String> {
    match value.trim() {
        "always-ask" | "always_ask" => Ok(ApprovalMode::AlwaysAsk),
        "ask-for-writes" | "ask_for_writes" => Ok(ApprovalMode::AskForWrites),
        "ask-for-dangerous" | "ask_for_dangerous" => Ok(ApprovalMode::AskForDangerous),
        "on-failure" | "on_failure" => Ok(ApprovalMode::OnFailure),
        "never-ask" | "never_ask" => Ok(ApprovalMode::NeverAsk),
        "read-only" | "read_only" => Ok(ApprovalMode::ReadOnly),
        other => Err(format!(
            "unknown approval mode `{other}`; expected always-ask|ask-for-writes|ask-for-dangerous|on-failure|never-ask|read-only"
        )),
    }
}

pub(crate) fn relative_path_from_input(input: &serde_json::Value) -> Option<String> {
    for key in ["path", "file", "file_path"] {
        if let Some(path) = input.get(key).and_then(|value| value.as_str()) {
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    None
}

pub(crate) fn preview_from_input(input: &serde_json::Value) -> Option<String> {
    let content = input.get("content").and_then(|value| value.as_str())?;
    if content.is_empty() {
        return None;
    }
    let lines: Vec<&str> = content.lines().collect();
    let shown: Vec<&str> = lines.iter().copied().take(8).collect();
    let mut preview = format!("{} lines", lines.len());
    if !shown.is_empty() {
        preview.push('\n');
        preview.push_str(&shown.join("\n"));
        if lines.len() > 8 {
            preview.push_str("\n…");
        }
    }
    Some(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kebab_and_snake() {
        assert_eq!(
            parse_approval_mode("ask-for-writes").expect("kebab"),
            ApprovalMode::AskForWrites
        );
        assert_eq!(
            parse_approval_mode("read_only").expect("snake"),
            ApprovalMode::ReadOnly
        );
        assert!(parse_approval_mode("yolo").is_err());
    }

    #[test]
    fn extracts_path_and_preview() {
        let input = serde_json::json!({
            "path": "src/demo.rs",
            "content": "one\ntwo\nthree"
        });
        assert_eq!(
            relative_path_from_input(&input).as_deref(),
            Some("src/demo.rs")
        );
        let preview = preview_from_input(&input).expect("preview");
        assert!(preview.starts_with("3 lines"));
        assert!(preview.contains("one"));
    }
}
