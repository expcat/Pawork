//! 终端审批：`y` 一次 / `a` 本 run / `n` 拒绝；取消则 `Cancelled`。

use std::io::{self, Write};

use async_trait::async_trait;
use pawork_app::{ApprovalAsk, ApprovalPromptHost, RiskLevel};
use pawork_domain::{ApprovalDecision, CancellationToken};
use tokio::io::{AsyncBufReadExt, BufReader};

/// 交互宿主：在 stderr 打印摘要，从 stdin 读 `y`/`a`/`n`。
#[derive(Debug, Default, Clone, Copy)]
pub struct InteractiveApprovals;

#[async_trait]
impl ApprovalPromptHost for InteractiveApprovals {
    async fn decide(&self, ask: &ApprovalAsk, cancel: CancellationToken) -> ApprovalDecision {
        eprint!("{}", format_approval_prompt(ask));
        let _ = io::stderr().flush();
        loop {
            eprint!("[y] once  [a] this run  [n] deny > ");
            let _ = io::stderr().flush();
            let mut line = String::new();
            let mut reader = BufReader::new(tokio::io::stdin());
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return ApprovalDecision::Cancelled,
                result = reader.read_line(&mut line) => result,
            };
            match result {
                Ok(0) => return ApprovalDecision::Denied,
                Ok(_) => match line.trim() {
                    "y" | "Y" => return ApprovalDecision::ApprovedOnce,
                    "a" | "A" => return ApprovalDecision::ApprovedForRun,
                    "n" | "N" => return ApprovalDecision::Denied,
                    _ => eprintln!("请输入 y / a / n"),
                },
                Err(_) => return ApprovalDecision::Denied,
            }
        }
    }
}

pub(crate) fn format_approval_prompt(ask: &ApprovalAsk) -> String {
    let mut out = String::new();
    let path = ask.relative_path.as_deref().unwrap_or("-");
    out.push_str(&format!(
        "approve {} {}  [{}]\n{}\n",
        ask.tool_name,
        path,
        risk_label(ask.risk),
        ask.message
    ));
    if let Some(preview) = &ask.preview {
        out.push_str(preview);
        out.push('\n');
    }
    out
}

fn risk_label(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Safe => "safe",
        RiskLevel::Moderate => "moderate",
        RiskLevel::Dangerous => "dangerous",
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::ToolCallId;

    use super::*;

    #[test]
    fn formats_tool_path_risk_and_preview() {
        let text = format_approval_prompt(&ApprovalAsk {
            run_id: pawork_domain::RunId::from("run-1"),
            session_id: Some(pawork_domain::SessionId::from("ses-1")),
            tool_name: "write_file".into(),
            tool_call_id: ToolCallId::from("call-1"),
            relative_path: Some("src/demo.rs".into()),
            message: "Approve workspace file write".into(),
            risk: RiskLevel::Moderate,
            preview: Some("3 lines\none\ntwo\nthree".into()),
        });
        assert!(text.contains("approve write_file src/demo.rs  [moderate]"));
        assert!(text.contains("Approve workspace file write"));
        assert!(text.contains("one"));
    }

    #[test]
    fn formats_edit_and_apply_patch_hunk_preview() {
        let edit = format_approval_prompt(&ApprovalAsk {
            run_id: pawork_domain::RunId::from("run-1"),
            session_id: Some(pawork_domain::SessionId::from("ses-1")),
            tool_name: "edit_file".into(),
            tool_call_id: ToolCallId::from("call-2"),
            relative_path: Some("a.txt".into()),
            message: "Approve workspace file edit".into(),
            risk: RiskLevel::Moderate,
            preview: Some("--- a.txt\n+++ a.txt\n-old\n+new".into()),
        });
        assert!(edit.contains("approve edit_file a.txt"));
        assert!(edit.contains("-old"));
        assert!(edit.contains("+new"));

        let patch = format_approval_prompt(&ApprovalAsk {
            run_id: pawork_domain::RunId::from("run-1"),
            session_id: Some(pawork_domain::SessionId::from("ses-1")),
            tool_name: "apply_patch".into(),
            tool_call_id: ToolCallId::from("call-3"),
            relative_path: Some("lib.rs".into()),
            message: "Approve workspace file patch".into(),
            risk: RiskLevel::Moderate,
            preview: Some("--- lib.rs\n+++ lib.rs\n+fn x() {}".into()),
        });
        assert!(patch.contains("approve apply_patch lib.rs"));
        assert!(patch.contains("+fn x() {}"));
    }
}
