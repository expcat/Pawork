//! 审批宿主：CLI / `--json` / 测试注入决策；engine 只看到 [`ApprovalDecision`]。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_api::ToolRequest;
use pawork_domain::{ApprovalDecision, CancellationToken, RunId, ToolCallId};
use pawork_policy::{ApprovalMode, RiskLevel};
use pawork_tools::{ApprovalOutcome, ApprovalResolver};
use tokio::sync::oneshot;

/// 一次需要用户确认的写操作摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalAsk {
    pub run_id: RunId,
    pub session_id: Option<pawork_domain::SessionId>,
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

/// Snapshot / Desktop 卡片用的待审批摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingToolApproval {
    pub run_id: RunId,
    pub session_id: Option<pawork_domain::SessionId>,
    pub tool_call_id: ToolCallId,
    pub tool_name: String,
    pub relative_path: Option<String>,
    pub risk: RiskLevel,
    pub message: String,
    pub preview: Option<String>,
}

#[derive(Debug)]
struct PendingAsk {
    ask: ApprovalAsk,
    sender: oneshot::Sender<ApprovalDecision>,
}

/// GUI 审批宿主：`decide` 挂起 oneshot，`ToolApprove` 唤醒。
///
/// 决策先到时入队，注册时立即解析；关窗不断开 oneshot，也不自动允许。
#[derive(Default)]
pub struct GuiApprovalHost {
    pending: Mutex<HashMap<String, PendingAsk>>,
    queued: Mutex<HashMap<String, ApprovalDecision>>,
    on_pending: Mutex<Option<Arc<dyn Fn(&ApprovalAsk) + Send + Sync>>>,
}

impl GuiApprovalHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_on_pending(&self, callback: impl Fn(&ApprovalAsk) + Send + Sync + 'static) {
        *self
            .on_pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(callback));
    }

    fn key(tool_call_id: &ToolCallId) -> String {
        tool_call_id.as_str().to_string()
    }

    /// `ToolApprove` 入口：映射后的 domain 决策唤醒等待项，或先入队。
    pub fn resolve(
        &self,
        run_id: &RunId,
        tool_call_id: &ToolCallId,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let key = Self::key(tool_call_id);
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.contains_key(&key) {
            if pending
                .get(&key)
                .is_some_and(|entry| entry.ask.run_id.as_str() != run_id.as_str())
            {
                return Err(format!(
                    "approval {} belongs to a different run",
                    tool_call_id.as_str()
                ));
            }
            let entry = pending.remove(&key).expect("pending key exists");
            drop(pending);
            let _ = entry.sender.send(decision);
            return Ok(());
        }
        drop(pending);
        self.queued
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, decision);
        Ok(())
    }

    pub fn pending(&self) -> Vec<PendingToolApproval> {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut list: Vec<_> = pending
            .values()
            .map(|entry| PendingToolApproval {
                run_id: entry.ask.run_id.clone(),
                session_id: entry.ask.session_id.clone(),
                tool_call_id: entry.ask.tool_call_id.clone(),
                tool_name: entry.ask.tool_name.clone(),
                relative_path: entry.ask.relative_path.clone(),
                risk: entry.ask.risk,
                message: entry.ask.message.clone(),
                preview: entry.ask.preview.clone(),
            })
            .collect();
        list.sort_by(|a, b| {
            a.run_id
                .as_str()
                .cmp(b.run_id.as_str())
                .then_with(|| a.tool_call_id.as_str().cmp(b.tool_call_id.as_str()))
        });
        list
    }

    /// run 终态清理，避免 snapshot 泄漏陈旧 pending。
    pub fn clear_run(&self, run_id: &RunId) {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, entry| entry.ask.run_id.as_str() != run_id.as_str());
    }
}

#[async_trait]
impl ApprovalPromptHost for GuiApprovalHost {
    async fn decide(&self, ask: &ApprovalAsk, cancel: CancellationToken) -> ApprovalDecision {
        let key = Self::key(&ask.tool_call_id);
        if let Some(decision) = self
            .queued
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&key)
        {
            return decision;
        }
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                key.clone(),
                PendingAsk {
                    ask: ask.clone(),
                    sender,
                },
            );
        let listener = self
            .on_pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(listener) = listener {
            listener(ask);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&key);
                ApprovalDecision::Cancelled
            }
            result = receiver => result.unwrap_or(ApprovalDecision::Cancelled),
        }
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

const PREVIEW_LINE_CAP: usize = 40;

pub(crate) fn relative_path_from_input(input: &serde_json::Value) -> Option<String> {
    for key in ["path", "file", "file_path"] {
        if let Some(path) = input.get(key).and_then(|value| value.as_str()) {
            if !path.is_empty() {
                return Some(path.to_string());
            }
        }
    }
    input
        .get("ops")
        .and_then(|value| value.as_array())
        .and_then(|ops| ops.first())
        .and_then(|op| op.get("path").and_then(|value| value.as_str()))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
}

pub(crate) fn preview_for_tool(
    tool_name: &str,
    input: &serde_json::Value,
    roots: &[std::path::PathBuf],
) -> Option<String> {
    let raw = match tool_name {
        "write_file" => preview_write_file(input, roots)?,
        "edit_file" => preview_edit_file(input)?,
        "apply_patch" => preview_apply_patch(input)?,
        _ => preview_generic_content(input)?,
    };
    Some(cap_preview(&raw, PREVIEW_LINE_CAP))
}

fn preview_write_file(input: &serde_json::Value, roots: &[std::path::PathBuf]) -> Option<String> {
    let path = relative_path_from_input(input)?;
    let new = input.get("content").and_then(|value| value.as_str())?;
    let old = read_existing(roots, &path);
    Some(match old {
        Some(old) => unified_replacement(&path, &old, new),
        None => all_additions(&path, new),
    })
}

fn preview_edit_file(input: &serde_json::Value) -> Option<String> {
    let path = relative_path_from_input(input).unwrap_or_else(|| "-".into());
    let edits = collect_edits(input);
    if edits.is_empty() {
        return None;
    }
    let mut out = format!("--- {path}\n+++ {path}\n");
    for (old, new) in edits {
        for line in old.lines() {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
        for line in new.lines() {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
    }
    Some(out)
}

fn preview_apply_patch(input: &serde_json::Value) -> Option<String> {
    let ops = input.get("ops").and_then(|value| value.as_array())?;
    if ops.is_empty() {
        return None;
    }
    let mut out = String::new();
    for op in ops {
        let kind = op.get("op").and_then(|value| value.as_str()).unwrap_or("update");
        let path = op.get("path").and_then(|value| value.as_str()).unwrap_or("-");
        match kind {
            "rename" => {
                let to = op.get("to").and_then(|value| value.as_str()).unwrap_or("-");
                out.push_str(&format!("--- {path}\n+++ {to}\n"));
            }
            "delete" => {
                out.push_str(&format!("--- {path}\n+++ /dev/null\n"));
            }
            _ => {
                out.push_str(&format!("--- {path}\n+++ {path}\n"));
                if let Some(content) = op.get("content").and_then(|value| value.as_str()) {
                    for line in content.lines() {
                        out.push('+');
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
    }
    Some(out)
}

fn preview_generic_content(input: &serde_json::Value) -> Option<String> {
    let content = input.get("content").and_then(|value| value.as_str())?;
    if content.is_empty() {
        return None;
    }
    Some(all_additions("-", content))
}

fn collect_edits(input: &serde_json::Value) -> Vec<(String, String)> {
    if let Some(arr) = input.get("edits").and_then(|value| value.as_array()) {
        return arr
            .iter()
            .filter_map(|item| {
                let old = item.get("old_string").and_then(|value| value.as_str())?;
                let new = item.get("new_string").and_then(|value| value.as_str())?;
                Some((old.to_string(), new.to_string()))
            })
            .collect();
    }
    match (
        input.get("old_string").and_then(|value| value.as_str()),
        input.get("new_string").and_then(|value| value.as_str()),
    ) {
        (Some(old), Some(new)) => vec![(old.to_string(), new.to_string())],
        _ => Vec::new(),
    }
}

fn read_existing(roots: &[std::path::PathBuf], relative: &str) -> Option<String> {
    for root in roots {
        let candidate = root.join(relative);
        if candidate.is_file() {
            return std::fs::read_to_string(&candidate).ok();
        }
    }
    None
}

fn unified_replacement(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("--- {path}\n+++ {path}\n");
    for line in old.lines() {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in new.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn all_additions(path: &str, content: &str) -> String {
    let mut out = format!("--- /dev/null\n+++ {path}\n");
    for line in content.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn cap_preview(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.trim_end_matches('\n').to_string();
    }
    let mut out = lines[..max_lines].join("\n");
    out.push_str("\n…");
    out
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
        let preview = preview_for_tool("write_file", &input, &[]).expect("preview");
        assert!(preview.contains("+++ src/demo.rs"));
        assert!(preview.contains("+one"));
        assert!(preview.contains("+two"));
    }

    #[test]
    fn apply_patch_uses_first_op_path() {
        let input = serde_json::json!({
            "ops": [{ "op": "update", "path": "lib.rs", "content": "fn x() {}" }]
        });
        assert_eq!(relative_path_from_input(&input).as_deref(), Some("lib.rs"));
        let preview = preview_for_tool("apply_patch", &input, &[]).expect("preview");
        assert!(preview.contains("--- lib.rs"));
        assert!(preview.contains("+fn x() {}"));
    }

    #[test]
    fn edit_file_preview_is_hunk() {
        let input = serde_json::json!({
            "path": "a.txt",
            "old_string": "hello",
            "new_string": "world"
        });
        let preview = preview_for_tool("edit_file", &input, &[]).expect("preview");
        assert!(preview.contains("--- a.txt"));
        assert!(preview.contains("-hello"));
        assert!(preview.contains("+world"));
    }

    #[test]
    fn write_file_preview_uses_existing_file() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("notes.txt"), "old\nline").expect("write");
        let input = serde_json::json!({
            "path": "notes.txt",
            "content": "new\nline"
        });
        let preview =
            preview_for_tool("write_file", &input, &[dir.path().to_path_buf()]).expect("preview");
        assert!(preview.contains("-old"));
        assert!(preview.contains("+new"));
    }

    #[tokio::test]
    async fn queued_decision_resolves_immediately() {
        let host = GuiApprovalHost::new();
        let ask = ApprovalAsk {
            run_id: RunId::from("run-1"),
            session_id: Some(pawork_domain::SessionId::from("ses-1")),
            tool_name: "write_file".into(),
            tool_call_id: ToolCallId::from("call-1"),
            relative_path: Some("notes.txt".into()),
            message: "Approve workspace file write".into(),
            risk: RiskLevel::Moderate,
            preview: None,
        };
        host.resolve(
            &ask.run_id,
            &ask.tool_call_id,
            ApprovalDecision::ApprovedOnce,
        )
        .expect("queue");
        let decision = host
            .decide(&ask, CancellationToken::new())
            .await;
        assert_eq!(decision, ApprovalDecision::ApprovedOnce);
        assert!(host.pending().is_empty());
    }
}
