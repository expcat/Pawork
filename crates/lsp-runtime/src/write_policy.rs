//! rename / code_action 的写操作策略契约。
//!
//! 语言服务产生的 `WorkspaceEdit` 不允许直接写盘；必须经注入的 [`WriteEditPolicy`]
//! 审批与 [`EditApplier`] 落盘（后者桥接到既有 edit-file / apply-patch + checkpoint，
//! P4-9/P4-11）。lsp-runtime 只产出规范化编辑 + 调用注入策略，自身不触碰文件系统。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::LspError;
use crate::protocol::WorkspaceEdit;

/// 写操作来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditOrigin {
    Rename,
    CodeAction,
}

/// 一次写编辑审批请求。
#[derive(Debug, Clone)]
pub struct EditRequest {
    pub origin: EditOrigin,
    /// 语言服务描述符 id（诊断归属）。
    pub descriptor_id: String,
    pub workspace: Vec<String>,
    pub edit: WorkspaceEdit,
}

impl EditRequest {
    pub fn total_edits(&self) -> usize {
        self.edit.total_edits()
    }
}

/// 策略裁决。
#[derive(Debug, Clone)]
pub enum PolicyVerdict {
    Allow,
    Deny {
        reason: String,
    },
    /// 需要用户确认；lsp-runtime 视为「不自动应用」，把 edit 返回给调用方。
    Ask,
}

/// 写编辑策略：把 WorkspaceEdit 交给既有 policy-engine + checkpoint 评估。
#[async_trait]
pub trait WriteEditPolicy: Send + Sync {
    async fn authorize(&self, request: &EditRequest) -> PolicyVerdict;
}

/// 编辑落盘器：桥接到 edit-file / apply-patch + checkpoint。
#[async_trait]
pub trait EditApplier: Send + Sync {
    /// 应用编辑，返回实际应用的编辑总数。
    async fn apply(&self, request: &EditRequest) -> Result<usize, LspError>;
}

/// 审批 + 落盘的结果：显式区分「实际应用」与「等待用户确认」，
/// 防止 Ask 或无 applier 时被误报为成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// 已实际应用的编辑总数（text edits + file operations）。
    Applied(usize),
    /// 策略返回 Ask：编辑未自动应用，调用方应把 edit 呈现给用户确认后另行处理。
    PendingUserConfirmation,
}

/// 只读策略：永不放行任何写编辑。用于纯诊断 / 查询场景与测试。
#[derive(Debug, Clone, Default)]
pub struct DenyAllPolicy;

#[async_trait]
impl WriteEditPolicy for DenyAllPolicy {
    async fn authorize(&self, _request: &EditRequest) -> PolicyVerdict {
        PolicyVerdict::Deny {
            reason: "write edits disabled (deny-all policy)".into(),
        }
    }
}

/// 调用方负责落盘的策略：审批直接通过，但 lsp-runtime 仍不直接写盘；
/// 实际落盘必须由注入的 [`EditApplier`] 完成。
#[derive(Debug, Clone, Default)]
pub struct AllowThenApplyPolicy;

#[async_trait]
impl WriteEditPolicy for AllowThenApplyPolicy {
    async fn authorize(&self, _request: &EditRequest) -> PolicyVerdict {
        PolicyVerdict::Allow
    }
}

/// 协调策略 + 落盘：审批通过时调用 applier；`Allow` 但未注入 applier 时返回
/// [`LspError::NoEditApplier`]（绝不假成功）；`Ask` 返回
/// [`EditOutcome::PendingUserConfirmation`]（编辑未应用，由调用方呈现给用户）；
/// `Deny` 映射为 [`LspError::PolicyDenied`]。
pub async fn authorize_and_apply(
    policy: &(dyn WriteEditPolicy + Send + Sync),
    applier: Option<&(dyn EditApplier + Send + Sync)>,
    request: &EditRequest,
) -> Result<EditOutcome, LspError> {
    match policy.authorize(request).await {
        PolicyVerdict::Allow => match applier {
            Some(applier) => applier.apply(request).await.map(EditOutcome::Applied),
            None => Err(LspError::NoEditApplier),
        },
        PolicyVerdict::Ask => Ok(EditOutcome::PendingUserConfirmation),
        PolicyVerdict::Deny { reason } => Err(LspError::PolicyDenied(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Position, Range};
    use crate::protocol::{TextDocumentEdit, TextEdit, WorkspaceEdit};

    fn sample_edit() -> WorkspaceEdit {
        WorkspaceEdit {
            document_changes: vec![TextDocumentEdit {
                uri: "file:///a.rs".into(),
                version: None,
                edits: vec![TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                    new_text: "bar".into(),
                }],
            }],
            file_operations: vec![],
        }
    }

    #[tokio::test]
    async fn deny_all_blocks_write() {
        let req = EditRequest {
            origin: EditOrigin::Rename,
            descriptor_id: "rust-analyzer".into(),
            workspace: vec!["file:///".into()],
            edit: sample_edit(),
        };
        let err = authorize_and_apply(&DenyAllPolicy, None, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, LspError::PolicyDenied(_)));
    }

    #[tokio::test]
    async fn allow_without_applier_is_rejected_not_fake_success() {
        let req = EditRequest {
            origin: EditOrigin::CodeAction,
            descriptor_id: "rust-analyzer".into(),
            workspace: vec![],
            edit: sample_edit(),
        };
        let err = authorize_and_apply(&AllowThenApplyPolicy, None, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, LspError::NoEditApplier));
    }

    #[derive(Debug, Default)]
    struct AskPolicy;

    #[async_trait]
    impl WriteEditPolicy for AskPolicy {
        async fn authorize(&self, _request: &EditRequest) -> PolicyVerdict {
            PolicyVerdict::Ask
        }
    }

    #[tokio::test]
    async fn ask_verdict_returns_pending_confirmation() {
        let req = EditRequest {
            origin: EditOrigin::Rename,
            descriptor_id: "rust-analyzer".into(),
            workspace: vec!["file:///".into()],
            edit: sample_edit(),
        };
        let outcome = authorize_and_apply(&AskPolicy, None, &req)
            .await
            .expect("ask must not error");
        assert_eq!(outcome, EditOutcome::PendingUserConfirmation);
    }

    struct CountingApplier(usize);

    #[async_trait]
    impl EditApplier for CountingApplier {
        async fn apply(&self, request: &EditRequest) -> Result<usize, LspError> {
            Ok(request.total_edits() + self.0)
        }
    }

    #[tokio::test]
    async fn allow_with_applier_reports_applied_count() {
        let req = EditRequest {
            origin: EditOrigin::Rename,
            descriptor_id: "rust-analyzer".into(),
            workspace: vec!["file:///".into()],
            edit: sample_edit(),
        };
        let outcome = authorize_and_apply(&AllowThenApplyPolicy, Some(&CountingApplier(10)), &req)
            .await
            .unwrap();
        assert_eq!(outcome, EditOutcome::Applied(11));
    }
}
