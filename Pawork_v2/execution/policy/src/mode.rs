//! 审批模式定义。

use serde::{Deserialize, Serialize};

/// 审批模式，决定 [`crate::PolicyEngine`] 在不同能力下如何裁决。
///
/// 严格程度大致递增：`NeverAsk` < `OnFailure` < `AskForDangerous` <
/// `AskForWrites` < `AlwaysAsk` < `ReadOnly`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// 任何执行都需要用户确认。
    AlwaysAsk,
    /// 仅对写操作（含进程/网络等副作用）询问。
    AskForWrites,
    /// 仅对危险命令（如 `rm -rf`、`sudo`）询问。
    AskForDangerous,
    /// 默认放行，失败后再处理。
    OnFailure,
    /// 从不询问（自动放行）。
    NeverAsk,
    /// 只读模式：拒绝一切非只读能力。
    #[default]
    ReadOnly,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_read_only() {
        assert_eq!(ApprovalMode::default(), ApprovalMode::ReadOnly);
    }

    #[test]
    fn serializes_snake_case() {
        let json = serde_json::to_string(&ApprovalMode::AskForWrites).expect("serialize");
        assert_eq!(json, "\"ask_for_writes\"");
    }

    #[test]
    fn deserializes_all_variants() {
        for (text, expected) in [
            ("\"always_ask\"", ApprovalMode::AlwaysAsk),
            ("\"ask_for_writes\"", ApprovalMode::AskForWrites),
            ("\"ask_for_dangerous\"", ApprovalMode::AskForDangerous),
            ("\"on_failure\"", ApprovalMode::OnFailure),
            ("\"never_ask\"", ApprovalMode::NeverAsk),
            ("\"read_only\"", ApprovalMode::ReadOnly),
        ] {
            let got: ApprovalMode = serde_json::from_str(text).expect("deserialize");
            assert_eq!(got, expected, "{text}");
        }
    }
}
