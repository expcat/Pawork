//! Phase 16 Modern Agent Workflow 的 canonical 领域类型与事件载荷（P16-1～P16-8）。
//!
//! 本模块只承载纯领域类型：状态枚举、快照结构与各 service 的 canonical 事件
//! 载荷（`PlanEvent` / `GoalEvent` / `TaskEvent` / `AutomationEvent` /
//! `MonitorEvent` / `MemoryEvent` / `ReviewEvent`）。它们是 [`crate::ids`]
//! 中 ID 的值载体，也是 `agent-events::AgentEvent` 各 wrapping 变量的载荷，
//! 保证「状态变化必须 canonical event 化、可重放」。
//!
//! 设计遵循 event-sourcing：事件本身是轻量「事实」（ID + 关键转移），富快照由
//! 各 service 在重放时重建并向外查询暴露，事件载荷不内联大体积正文。
//!
//! 架构约束：本模块不依赖任何 infra / Provider / GUI；Plan / Review 不携带写
//! 权限；所有外部平台差异（Forge / Trigger）只经 adapter 输入，core 不含平台
//! 名称分支。

use serde::{Deserialize, Serialize};

use crate::ids::{
    ArtifactId, AutomationId, BackgroundTaskId, CheckpointId, EventId, GoalId, MemoryId, MonitorId,
    PlanId, PlanStepId, PlanVersionId, ReviewFindingId, ReviewSessionId, RunId, SessionId,
    WorkspaceId,
};

// =========================================================================
// P16-1 / P16-2 —— Plan Mode 与 Plan Review / Approval
// =========================================================================

/// Plan 步骤状态机：`pending → in_progress → completed | blocked`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// Plan 评审状态机：`draft → in_review → changes_requested → approved | rejected`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanReviewStatus {
    #[default]
    Draft,
    InReview,
    ChangesRequested,
    Approved,
    Rejected,
}

/// Plan 步骤的不可变快照（用于 `Created` / `Replaced` 与查询面）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStepSnapshot {
    pub step_id: PlanStepId,
    pub text: String,
    pub status: PlanStepStatus,
}

/// Plan 评审意见的稳定锚点：`plan_version + step_id` 为主锚，`line_offset`
/// 与可选 `file:line` 为辅助定位；后续可无损转换为通用 Review Finding。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanCommentAnchor {
    pub step_id: PlanStepId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_offset: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_line: Option<u32>,
}

/// Plan 的 canonical 事件载荷（P16-1 基础 + P16-2 评审/审批）。
///
/// 计划本身是只读建议，不触发工具 / 文件变更；审批仅作为执行 gate 放行，不扩权。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanEvent {
    /// Agent 生成首版 Plan。
    Created {
        plan_id: PlanId,
        version: PlanVersionId,
        title: String,
        steps: Vec<PlanStepSnapshot>,
    },
    /// 单步状态转移（须经合法状态机）。
    StepUpdated {
        plan_id: PlanId,
        step_id: PlanStepId,
        status: PlanStepStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Agent 整体替换 Plan，新版本带 `parent_version`，旧版本保留。
    Replaced {
        plan_id: PlanId,
        version: PlanVersionId,
        parent_version: PlanVersionId,
        title: String,
        steps: Vec<PlanStepSnapshot>,
    },
    /// 提交评审。
    ReviewRequested {
        plan_id: PlanId,
        version: PlanVersionId,
    },
    /// `changes_requested` 触发的新版本修订（保留修订链）。
    Revised {
        plan_id: PlanId,
        version: PlanVersionId,
        parent_version: PlanVersionId,
    },
    /// 审批通过；`checkpoint_id` 标记批准点（可回滚）。
    Approved {
        plan_id: PlanId,
        version: PlanVersionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<CheckpointId>,
    },
    /// 审批拒绝。
    Rejected {
        plan_id: PlanId,
        version: PlanVersionId,
        reason: String,
    },
    /// 行锚点评审意见。
    CommentAdded {
        plan_id: PlanId,
        version: PlanVersionId,
        anchor: PlanCommentAnchor,
        body: String,
    },
}

// =========================================================================
// P16-3 —— Goal Mode
// =========================================================================

/// Goal 生命周期状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    #[default]
    Active,
    Paused,
    Achieved,
    Abandoned,
}

/// 成功标准的可检性：`Auto` 可机检、`Human` 需人确认。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionKind {
    Auto,
    Human,
}

/// 可验证成功标准快照。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessCriterionSnapshot {
    pub criterion_id: String,
    pub description: String,
    pub kind: CriterionKind,
    /// 自动可检项是否已命中（人审项恒为 false，避免 Agent 自行宣布达成）。
    #[serde(default)]
    pub satisfied: bool,
}

/// Goal 的 canonical 事件载荷。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalEvent {
    Created {
        goal_id: GoalId,
        title: String,
        criteria: Vec<SuccessCriterionSnapshot>,
    },
    /// progress 基于 completed Plan 步骤与 criteria 命中率，`progress ∈ [0,1]`。
    ProgressUpdated {
        goal_id: GoalId,
        progress: f64,
    },
    /// 单项成功标准被满足（`Auto` 由 Agent、`Human` 由人审入口）。
    ///
    /// 与 `ProgressUpdated` 配合：本变体持久化并恢复单项 criterion 的满足位，
    /// 后者刷新命中率进度。二者同时产出，保证 replay 后 criteria 与 progress
    /// 不再自相矛盾（修复 ADR-016：满足位必须可重放）。
    CriterionSatisfied {
        goal_id: GoalId,
        criterion_id: String,
    },
    Paused {
        goal_id: GoalId,
    },
    /// resume 时复算剩余预算而非沿用旧值。
    Resumed {
        goal_id: GoalId,
        remaining_budget_tokens: u64,
    },
    /// 运行中转向输入（修正方向 / 约束 / 新优先级），事后可回溯。
    Steered {
        goal_id: GoalId,
        input: String,
    },
    Achieved {
        goal_id: GoalId,
    },
    Abandoned {
        goal_id: GoalId,
        reason: String,
    },
}

// =========================================================================
// P16-4 —— Background Task Manager
// =========================================================================

/// 统一后台任务种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// 子进程 / PTY，必须经 Sandbox Runtime → Process Runtime。
    Process,
    /// 子 Agent / Worker（Multi-Agent）。
    Agent,
    /// 监视循环（P16-6）。
    Monitor,
    /// 定时 / 事件触发的自动化（P16-5）。
    Automation,
}

/// 后台任务状态机：`queued → running → suspended → completed | failed | canceled`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[default]
    Queued,
    Running,
    Suspended,
    Completed,
    Failed,
    Canceled,
}

/// Background task 的 canonical 事件载荷。
///
/// 所有转移可持久化可重放；取消按取消树（P12-6）传播到子任务 / 子进程。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskEvent {
    Started {
        task_id: BackgroundTaskId,
        task_kind: TaskKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_task_id: Option<BackgroundTaskId>,
    },
    Suspended {
        task_id: BackgroundTaskId,
    },
    Resumed {
        task_id: BackgroundTaskId,
    },
    Finished {
        task_id: BackgroundTaskId,
        status: TaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

// =========================================================================
// P16-5 —— Scheduled Automation
// =========================================================================

/// 自动化触发器种类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTriggerKind {
    /// 五 / 六字段 cron 表达式（自实现最小子集）。
    Cron,
    /// 固定间隔。
    Interval,
    /// 一次性延时。
    Once,
    /// 订阅 canonical event 流做模式匹配。
    Event,
}

/// Automation 的 canonical 事件载荷。
///
/// 外部触发器（Webhook / GitHub / GitLab / MCP）只能经认证 adapter 转为
/// canonical event，Automation core 不含平台分支。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationEvent {
    Registered {
        automation_id: AutomationId,
        trigger: AutomationTriggerKind,
    },
    /// 触发后经 task-manager 派发为 background task。
    Triggered {
        automation_id: AutomationId,
        task_id: BackgroundTaskId,
    },
    /// 执行产出归档进 result inbox（artifact）。
    ResultArchived {
        automation_id: AutomationId,
        artifact_id: ArtifactId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
    },
    /// 连续失败暂停并告警，不静默吞错。
    Suspended {
        automation_id: AutomationId,
        reason: String,
    },
}

// =========================================================================
// P16-6 —— Persistent Process / Monitor
// =========================================================================

/// Monitor 命中来源类型（文件变化 / 进程退出 / 正则命中 / 端口状态）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorSourceKind {
    FileChange,
    ProcessExit,
    RegexMatch,
    PortState,
}

/// Monitor 的 canonical 事件载荷。
///
/// Monitor 命中产出 canonical event，可作为 P16-5 `event` 触发器来源；同时作为
/// Plugin Package Monitors（P17-2）声明的唯一运行时执行点。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonitorEvent {
    Started {
        monitor_id: MonitorId,
        source: MonitorSourceKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<WorkspaceId>,
    },
    /// 命中产出事件，可被 automation `event` 触发器消费。
    Triggered {
        monitor_id: MonitorId,
        detail: String,
    },
    Stopped {
        monitor_id: MonitorId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

// =========================================================================
// P16-7 —— Long-term Memory
// =========================================================================

/// 记忆隐私标签，控制跨 workspace 共享与脱敏。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPrivacy {
    #[default]
    WorkspaceLocal,
    Shareable,
}

/// Memory 的 canonical 事件载荷。
///
/// 记忆从历史 canonical event 只读提炼，不修改 / 删除任何事件；含 Secret /
/// 敏感内容的 event 不进入记忆。失效为 `invalidated` 而非删除，保留可追溯。
/// `embedding` / `confidence` 为浮点，故只 impl `PartialEq`（不要求 `Eq`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryEvent {
    Recorded {
        memory_id: MemoryId,
        summary: String,
        /// 来源 event 引用（只读提炼线索）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_event_id: Option<EventId>,
        privacy: MemoryPrivacy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<WorkspaceId>,
        /// Provider-neutral embedding。新流事件持久化向量以支持完整 replay；
        /// 旧流缺字段时为 serde 兼容默认空向量，仍可反序列化，但检索层会过滤
        /// 空 embedding，需重新嵌入后才可检索。
        #[serde(default)]
        embedding: Vec<f32>,
        /// 记录置信度。旧流事件未携带时默认 `0.0`。
        #[serde(default)]
        confidence: f32,
    },
    Invalidated {
        memory_id: MemoryId,
        reason: String,
    },
}

// =========================================================================
// P16-8 —— Review Engine
// =========================================================================

/// Review Finding 严重度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSeverity {
    #[default]
    Info,
    Minor,
    Major,
    Critical,
}

/// resolution 生命周期：`open → addressed → resolved | wontfix`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewResolution {
    #[default]
    Open,
    Addressed,
    Resolved,
    Wontfix,
}

/// Review Engine 的行锚点（`file:line` + 范围）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewAnchor {
    pub file: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}

/// 建议补丁（canonical）：评审引擎只做 dry-run（校验 / 解析 / 内存试应用），
/// 实际应用交既有工具 + policy（checkpoint / sandbox），引擎本身不写文件。
///
/// 放在 canonical domain 以便 `ReviewEvent::FindingOpened` 携带并完整重放。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedPatch {
    pub file: String,
    pub payload: String,
}

/// Review Engine 的 canonical 事件载荷。
///
/// 评审引擎对工作区只读；写动作交既有工具并受 policy 约束；PR comment 仅在
/// 用户显式 publish 后经 ForgeAdapter 发送，Review core 不产生外部副作用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewEvent {
    SessionCreated {
        session_id: ReviewSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<WorkspaceId>,
    },
    FindingOpened {
        session_id: ReviewSessionId,
        finding_id: ReviewFindingId,
        anchor: ReviewAnchor,
        severity: ReviewSeverity,
        body: String,
        /// 佐证（diff 行 / 日志片段）。旧流事件未携带时默认空（serde 兼容）。
        #[serde(default)]
        evidence: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assignee: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        suggested_patch: Option<SuggestedPatch>,
        /// 打开时锚点上下文指纹（re-anchor 用）；文件不可读时为 `None`。
        /// 旧流事件未携带时默认 `None`（serde 兼容）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    /// resolution 转移，可关联修复 commit / patch / Run。
    FindingResolved {
        finding_id: ReviewFindingId,
        resolution: ReviewResolution,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix_ref: Option<String>,
    },
    CommentPublished {
        session_id: ReviewSessionId,
        finding_id: ReviewFindingId,
        /// 目标平台经 adapter 表达，core 不含 GitHub / GitLab 名称分支。
        forge: String,
    },
}

// 引用占位：保持 session/run 关联类型在文档语义中可见，避免未使用 import 警告。
const _: fn() = || {
    let _ = (
        std::marker::PhantomData::<SessionId>,
        std::marker::PhantomData::<RunId>,
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_memory_recorded_defaults_embedding_and_confidence() {
        let legacy = r#"{"kind":"recorded","memory_id":"memory_1","summary":"legacy","privacy":"workspace_local"}"#;

        let event: MemoryEvent = serde_json::from_str(legacy).expect("deserialize legacy event");
        let MemoryEvent::Recorded {
            embedding,
            confidence,
            ..
        } = event
        else {
            panic!("expected recorded event");
        };

        assert!(embedding.is_empty());
        assert_eq!(confidence, 0.0);
    }
}
