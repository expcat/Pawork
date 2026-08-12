//! 平台无关的 ForgeAdapter：GitHub / GitLab / Generic 枚举 + trait。
//!
//! Review core 不含平台名称 match 分支（有测试断言）：core 只依赖
//! [`ForgeAdapter`] 抽象与 [`ForgeKind::as_str`] 透传标签；平台差异全部收敛
//! 在本模块。`publish_comment` 是唯一会产生外部副作用的入口，仅在用户显式
//! 调用 `ReviewEngine::publish_comment` 时触发。

use agent_domain::ReviewAnchor;
use serde::{Deserialize, Serialize};

use crate::{
    error::ReviewError,
    model::{PRComment, PRContext, ReviewFinding},
};

/// 平台种类（adapter 层自描述；core 不分支）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeKind {
    GitHub,
    GitLab,
    Generic,
}

impl ForgeKind {
    /// 自描述标签，core 只透传进 `CommentPublished.forge`，不做行为分支。
    pub fn as_str(self) -> &'static str {
        match self {
            ForgeKind::GitHub => "github",
            ForgeKind::GitLab => "gitlab",
            ForgeKind::Generic => "generic",
        }
    }
}

/// PR 引用（平台无关）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrReference {
    pub repo: String,
    pub pr_number: u64,
    pub head_sha: Option<String>,
}

/// ForgeAdapter：拉取 PR context、映射平台字段为 [`PRContext`]。
///
/// - `fetch_pr_context`：读取平台 PR 元数据（读）；
/// - `map_comment`：把 finding 映射为待发布评论（纯函数）；
/// - `publish_comment`：唯一外部副作用入口，仅用户显式发布时调用。
pub trait ForgeAdapter: Send + Sync {
    fn kind(&self) -> ForgeKind;
    fn fetch_pr_context(&self, pr: &PrReference) -> Result<PRContext, ReviewError>;
    fn map_comment(&self, context: &PRContext, finding: &ReviewFinding) -> PRComment;
    fn publish_comment(
        &self,
        context: &PRContext,
        comment: &PRComment,
    ) -> Result<PRComment, ReviewError>;
}

/// 占位实现：不访问网络，字段透传 / 本地生成 id。
#[derive(Clone, Debug, Default)]
pub struct GenericForgeAdapter {
    pub base_ref: Option<String>,
}

impl ForgeAdapter for GenericForgeAdapter {
    fn kind(&self) -> ForgeKind {
        ForgeKind::Generic
    }

    fn fetch_pr_context(&self, pr: &PrReference) -> Result<PRContext, ReviewError> {
        Ok(PRContext {
            repo: pr.repo.clone(),
            pr_number: pr.pr_number,
            title: format!("PR #{}", pr.pr_number),
            files: Vec::new(),
            head_sha: pr.head_sha.clone(),
            base_ref: self.base_ref.clone(),
            raw: None,
        })
    }

    fn map_comment(&self, _context: &PRContext, finding: &ReviewFinding) -> PRComment {
        let mut body = format!(
            "[{}] {}\n锚点：{}:{}",
            severity_label(finding.severity),
            finding.body,
            finding.anchor.file,
            finding.anchor.line
        );
        if !finding.evidence.is_empty() {
            body.push_str("\n\n证据：");
            for evidence in &finding.evidence {
                body.push_str(&format!("\n- {evidence}"));
            }
        }
        PRComment {
            id: None,
            anchor: Some(finding.anchor.clone()),
            body,
            published: false,
        }
    }

    fn publish_comment(
        &self,
        context: &PRContext,
        comment: &PRComment,
    ) -> Result<PRComment, ReviewError> {
        // 占位实现：真实平台会调用其 API；此处仅生成本地 id，不产生网络副作用。
        let line = comment.anchor.as_ref().map(|a| a.line).unwrap_or_default();
        Ok(PRComment {
            id: Some(format!("generic:{}:{}", context.pr_number, line)),
            published: true,
            ..comment.clone()
        })
    }
}

fn severity_label(severity: agent_domain::ReviewSeverity) -> &'static str {
    match severity {
        agent_domain::ReviewSeverity::Info => "info",
        agent_domain::ReviewSeverity::Minor => "minor",
        agent_domain::ReviewSeverity::Major => "major",
        agent_domain::ReviewSeverity::Critical => "critical",
    }
}

/// 把锚点映射为平台注释位置字符串（adapter 层工具）。
pub fn anchor_position(anchor: &ReviewAnchor) -> String {
    match anchor.end_line {
        Some(end) => format!("{}:{}-{}", anchor.file, anchor.line, end),
        None => format!("{}:{}", anchor.file, anchor.line),
    }
}
