//! Team 协作层的 canonical 事件、信封与事件 sink 契约。
//!
//! # 设计要点
//! - [`TeamEvent`] 是 team 协作语义的最小「事实」，与 P16 各 service 一样采用
//!   event-sourcing：命令面校验后构造事件 → `apply` 折叠进 [`crate::state::TeamAggregate`]
//!   → 经 [`TeamEventSink`] 投递给上游。事件 JSON 可往返、可重放。
//! - **不另建 `tokio::broadcast`**（ADR-024 统一 Event Hub）。本 crate 通过注入
//!   [`TeamEventSink`] 把 canonical team 事件交还 `app-service`，由 app-service
//!   的唯一 EventHub（`subscription-hub::EventHub`）统一序列化、ring buffer
//!   化与扇出。teams 只拥有协作语义，不拥有传输。
//! - `automation` / 执行权威统一归 `task-manager`；teams 不执行 run loop、不
//!   派发后台任务，仅产出协作事件。

use std::sync::{Arc, Mutex};

use agent_domain::{
    AgentId, CheckpointId, EventId, PlanCommentAnchor, PlanId, PlanStepSnapshot, PlanVersionId,
    TenantId, Timestamp,
};
use orchestration::{TaskId, TaskState};
use serde::{Deserialize, Serialize};

use crate::ids::{FanOutId, MailboxMessageId, MemberRole, TeamId};
use crate::presence::Presence;

/// 共享任务板上的一个任务条目（事件快照；与 [`orchestration::AgentTask`] 同构，
/// 但 owner 由「团队认领」语义决定，可为 `None` 表示未认领）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardTask {
    /// 任务标识（复用 P12 TaskId）。
    pub task_id: TaskId,
    /// 张贴者。
    pub poster: AgentId,
    /// 当前认领者；`None` 表示未认领、可被任何成员认领。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AgentId>,
    /// 任务描述。
    pub description: String,
    /// 依赖的任务（复用 P12 任务图依赖语义）。
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    /// 当前状态（复用 P12 TaskState）。
    pub state: TaskState,
    /// 已重试次数。
    #[serde(default)]
    pub retry_count: u32,
    /// 最大重试次数。
    #[serde(default)]
    pub max_retries: u32,
}

/// mailbox 投递范围。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recipients {
    /// 点对点：精确的成员列表。
    Direct { members: Vec<AgentId> },
    /// 广播：除发送者外的全部成员。
    Broadcast,
}

/// Team 协作的 canonical 事件载荷。
///
/// 每个变体都是一次不可变的协作事实；[`crate::state::apply`] 是其纯函数折叠，
/// 重放事件序列即可无损重建 team 状态（ADR-016：状态变化必须可重放）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamEvent {
    /// team 创建。
    TeamCreated {
        team_id: TeamId,
        tenant_id: TenantId,
        supervisor: AgentId,
        name: String,
    },
    /// 成员加入。
    MemberAdded {
        team_id: TeamId,
        agent_id: AgentId,
        role: MemberRole,
    },
    /// 成员移除。
    MemberRemoved { team_id: TeamId, agent_id: AgentId },
    /// team 解散（终态）。
    TeamDissolved { team_id: TeamId },

    /// 共享任务板上张贴任务。
    TaskPosted { team_id: TeamId, task: BoardTask },
    /// 成员认领任务（owner 由 None → claimer，state → Assigned）。
    TaskClaimed {
        team_id: TeamId,
        task_id: TaskId,
        claimer: AgentId,
    },
    /// 认领者释放任务（owner → None）。
    TaskReleased {
        team_id: TeamId,
        task_id: TaskId,
        by: AgentId,
    },
    /// 任务板状态推进（复用 P12 TaskState 状态机）。
    TaskAdvanced {
        team_id: TeamId,
        task_id: TaskId,
        state: TaskState,
    },

    /// mailbox 消息投递（持久化）。
    MailboxPosted {
        team_id: TeamId,
        message_id: MailboxMessageId,
        sender: AgentId,
        recipients: Recipients,
        body: String,
    },
    /// 消息被某收件人拉取投递。
    MailboxDelivered {
        team_id: TeamId,
        message_id: MailboxMessageId,
        recipient: AgentId,
    },
    /// 消息被某收件人标记已读。
    MailboxRead {
        team_id: TeamId,
        message_id: MailboxMessageId,
        by: AgentId,
    },

    /// 成员 presence 变化（由 worker 生命周期派生）。
    PresenceChanged {
        team_id: TeamId,
        agent_id: AgentId,
        presence: Presence,
    },

    /// 受控 peer messaging fan-out 通过策略并被路由。
    PeerMessageRouted {
        team_id: TeamId,
        message_id: MailboxMessageId,
        fan_out_id: FanOutId,
        sender: AgentId,
        recipients: Recipients,
        body: String,
    },
    /// peer messaging fan-out 被策略拒绝（不路由）。
    FanOutDenied {
        team_id: TeamId,
        sender: AgentId,
        recipients: Recipients,
        reason: String,
    },

    /// worker 向 team 提交 Plan（复用 P16-1 plan；提交即进入评审）。
    PlanSubmitted {
        team_id: TeamId,
        plan_id: PlanId,
        version: PlanVersionId,
        title: String,
        steps: Vec<PlanStepSnapshot>,
    },
    /// team / parent 审批通过（复用 P16-2 review 状态机）。
    PlanApproved {
        team_id: TeamId,
        plan_id: PlanId,
        version: PlanVersionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_id: Option<CheckpointId>,
    },
    /// team / parent 审批拒绝。
    PlanRejected {
        team_id: TeamId,
        plan_id: PlanId,
        version: PlanVersionId,
        reason: String,
    },
    /// 行锚点评审意见（复用 P16-2 comment）。
    PlanCommented {
        team_id: TeamId,
        plan_id: PlanId,
        version: PlanVersionId,
        anchor: PlanCommentAnchor,
        body: String,
    },
}

impl TeamEvent {
    /// 该事件归属的 team。
    pub fn team_id(&self) -> &TeamId {
        match self {
            Self::TeamCreated { team_id, .. }
            | Self::MemberAdded { team_id, .. }
            | Self::MemberRemoved { team_id, .. }
            | Self::TeamDissolved { team_id }
            | Self::TaskPosted { team_id, .. }
            | Self::TaskClaimed { team_id, .. }
            | Self::TaskReleased { team_id, .. }
            | Self::TaskAdvanced { team_id, .. }
            | Self::MailboxPosted { team_id, .. }
            | Self::MailboxDelivered { team_id, .. }
            | Self::MailboxRead { team_id, .. }
            | Self::PresenceChanged { team_id, .. }
            | Self::PeerMessageRouted { team_id, .. }
            | Self::FanOutDenied { team_id, .. }
            | Self::PlanSubmitted { team_id, .. }
            | Self::PlanApproved { team_id, .. }
            | Self::PlanRejected { team_id, .. }
            | Self::PlanCommented { team_id, .. } => team_id,
        }
    }
}

/// 团队内单调递增的事件序列（ADR-016 重放不变量）。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct TeamEventSequence(pub u64);

impl TeamEventSequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn value(self) -> u64 {
        self.0
    }
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
    pub fn is_immediately_after(self, previous: Self) -> bool {
        previous.0.checked_add(1) == Some(self.0)
    }
}

/// Team 事件信封：携带协作事件、归属、序列与因果 parent（可重放 / 可追溯）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamEventEnvelope {
    pub team_id: TeamId,
    pub sequence: TeamEventSequence,
    pub event_id: EventId,
    pub timestamp: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_event_id: Option<EventId>,
    pub payload: TeamEvent,
}

impl TeamEventEnvelope {
    pub fn new(
        team_id: TeamId,
        sequence: TeamEventSequence,
        event_id: EventId,
        timestamp: Timestamp,
        payload: TeamEvent,
    ) -> Self {
        Self {
            team_id,
            sequence,
            event_id,
            timestamp,
            parent_event_id: None,
            payload,
        }
    }

    pub fn with_parent(mut self, parent_event_id: EventId) -> Self {
        self.parent_event_id = Some(parent_event_id);
        self
    }
}

/// 事件 sink 契约：team 协作事件的唯一外发出口。
///
/// # app-service 接线契约（ADR-024 统一 Event Hub）
///
/// `app-service` 在装配层实现本 trait，把每条 [`TeamEventEnvelope`] 适配为
/// `core_api::AppEvent`（team 协作分支）后调用 `subscription_hub::EventHub::publish`
/// 统一全局序列化、ring buffer 化与 `broadcast` 扇出。本 crate **不**自建
/// `tokio::broadcast`、**不**直接持有 EventHub，只通过注入 sink 把「事实」交还。
///
/// 实现要点：
/// 1. `record` 应非阻塞、幂等可重入（上游 Hub 已保证全局连续序列）。
/// 2. team 事件经此 sink 后即等价于 canonical app 事件，可被 GUI / CLI watch
///    订阅、可崩溃重放（重放源仍为 team 事件流，本 sink 仅做对外镜像）。
pub trait TeamEventSink: Send + Sync {
    /// 投递一条 canonical team 事件。
    fn record(&self, envelope: TeamEventEnvelope);
}

/// 空实现（默认装配 / 测试占位）。
#[derive(Debug, Default)]
pub struct NullTeamSink;

impl TeamEventSink for NullTeamSink {
    fn record(&self, _envelope: TeamEventEnvelope) {}
}

/// 录制实现（测试 / 自省 / app-service 暂存适配）。
#[derive(Default)]
pub struct RecordingTeamSink {
    events: Mutex<Vec<TeamEventEnvelope>>,
}

impl RecordingTeamSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已录制的事件（按投递顺序）。
    pub fn events(&self) -> Vec<TeamEventEnvelope> {
        self.events.lock().expect("sink poisoned").clone()
    }

    /// 仅保留 payload 的事件流（重放入口常用形态）。
    pub fn payloads(&self) -> Vec<TeamEvent> {
        self.events().into_iter().map(|env| env.payload).collect()
    }
}

impl TeamEventSink for RecordingTeamSink {
    fn record(&self, envelope: TeamEventEnvelope) {
        self.events.lock().expect("sink poisoned").push(envelope);
    }
}

/// 把任意 `TeamEventSink` 提升为共享句柄。
pub fn shared<S: TeamEventSink + 'static>(sink: S) -> Arc<dyn TeamEventSink> {
    Arc::new(sink)
}
