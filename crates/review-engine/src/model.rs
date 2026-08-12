//! Review Engine 领域模型。

use std::collections::BTreeMap;

use agent_domain::{
    ReviewAnchor, ReviewFindingId, ReviewResolution, ReviewSessionId, ReviewSeverity, WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// `SuggestedPatch` 已移至 canonical domain（`agent_domain`），以便
/// `ReviewEvent::FindingOpened` 携带并完整重放；此处仅做透传再导出。
pub use agent_domain::SuggestedPatch;

/// 评审会话：持有 findings、已发布评论记录。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSession {
    pub session_id: ReviewSessionId,
    pub workspace_id: Option<WorkspaceId>,
    pub findings: BTreeMap<ReviewFindingId, ReviewFinding>,
    /// 已发布评论记录（`forge` 字符串由 adapter 层提供，core 不分支）。
    pub published_comments: Vec<PublishedCommentRecord>,
}

/// 一条评审意见（finding）。
///
/// canonical 事件流（`ReviewEvent`）承载完整可重放状态：`FindingOpened` 携带
/// evidence / assignee / suggested_patch / fingerprint，`FindingResolved` 携带
/// resolution / fix_ref。replay 后 finding 与实时路径完整一致（ADR-016）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub finding_id: ReviewFindingId,
    /// 行锚点（`file:line` + 可选范围）。
    pub anchor: ReviewAnchor,
    pub severity: ReviewSeverity,
    pub body: String,
    /// 佐证（如 diff 行、日志片段）。
    pub evidence: Vec<String>,
    pub assignee: Option<String>,
    pub resolution: ReviewResolution,
    /// 修复引用（commit / patch / Run），随 `FindingResolved` 事件持久化。
    pub fix_ref: Option<String>,
    pub suggested_patch: Option<SuggestedPatch>,
    /// 打开时锚点上下文指纹（re-anchor 用）；文件不可读时为 `None`。
    pub anchor_fingerprint: Option<String>,
}

impl ReviewFinding {
    /// finding 是否处于可评审（未终结）状态。
    pub fn is_open(&self) -> bool {
        self.resolution == ReviewResolution::Open || self.resolution == ReviewResolution::Addressed
    }

    /// 锚点范围尾行（与 `anchor.end_line` 一致）。
    pub fn end_line(&self) -> Option<u32> {
        self.anchor.end_line
    }
}

/// resolution 结果：状态 + 修复引用，形成「finding → suggestion → fix → resolution」链。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub status: ReviewResolution,
    pub fix_ref: Option<String>,
}

/// 平台无关的 PR 上下文（ForgeAdapter 映射平台字段得到）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PRContext {
    pub repo: String,
    pub pr_number: u64,
    pub title: String,
    pub files: Vec<String>,
    pub head_sha: Option<String>,
    pub base_ref: Option<String>,
    /// 平台原始元数据（可选，透传不解析）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// 平台无关的 PR 评论。生成（`export_comments`）≠ 发布（`publish_comment`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PRComment {
    pub id: Option<String>,
    pub anchor: Option<ReviewAnchor>,
    pub body: String,
    pub published: bool,
}

/// 已发布评论记录（由 `CommentPublished` 事件折叠而来）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedCommentRecord {
    pub finding_id: ReviewFindingId,
    pub forge: String,
}

/// 待发布评论（导入 diff 范围生成；未自动发布）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingComment {
    pub finding_id: ReviewFindingId,
    pub anchor: ReviewAnchor,
    pub body: String,
}

/// 聚合分组计数（按 file / severity / status）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCount {
    pub key: String,
    pub total: u32,
    pub open: u32,
    pub addressed: u32,
    pub resolved: u32,
    pub wontfix: u32,
}

/// 会话快照（含 re-anchor 派生结果与聚合视图）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSessionSnapshot {
    pub session_id: ReviewSessionId,
    pub workspace_id: Option<WorkspaceId>,
    pub findings: Vec<FindingSnapshot>,
    pub published_comments: Vec<PublishedCommentRecord>,
    pub pending_comments: Vec<PendingComment>,
    pub aggregate: AggregateSnapshot,
}

/// finding 快照：`anchor` 为 re-anchor 后的当前位置（失败时保留原锚点），
/// `stale` 标记漂移而非静默失效。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingSnapshot {
    pub finding_id: ReviewFindingId,
    pub anchor: ReviewAnchor,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub severity: ReviewSeverity,
    pub body: String,
    pub evidence: Vec<String>,
    pub assignee: Option<String>,
    pub resolution: ReviewResolution,
    pub fix_ref: Option<String>,
    pub suggested_patch: Option<SuggestedPatch>,
}

/// 聚合快照。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateSnapshot {
    pub by_file: Vec<GroupCount>,
    pub by_severity: Vec<GroupCount>,
    pub by_status: Vec<GroupCount>,
}
