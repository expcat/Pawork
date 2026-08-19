//! Worker 生命周期状态机（P12-1 core）。
//!
//! 纯逻辑模块：不执行 IO。状态流转：
//!
//! ```text
//! Created --Admit--> Admitted --Start--> Starting --BeginRunning--> Running
//! Running --BeginWaiting--> Waiting --Resume--> Running
//! Starting | Running | Waiting --Complete--> Completed
//! (any active) --BeginCancel--> Cancelling --Cancel--> Cancelled
//! (any active) --Fail--> Failed
//! ```
//!
//! 终态（Completed / Cancelled / Failed）拒绝一切转换。每次转换都可映射为
//! [`OrchestrationEvent`]，供事件溯源重放（ADR-016）。

use std::collections::BTreeMap;
use std::fmt;

use pawork_domain::{AgentId, SessionId, TenantId};
use serde::{Deserialize, Serialize};

use crate::identity::WorkerRole;
use crate::task_graph::TaskId;

/// Worker 生命周期状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// 刚创建，尚未准入。
    #[default]
    Created,
    /// 已通过准入（policy / 预算），等待启动。
    Admitted,
    /// 正在启动（lease 已持有）。
    Starting,
    /// 正在运行。
    Running,
    /// 等待外部输入（用户 / 依赖）。
    Waiting,
    /// 正常完成（终态）。
    Completed,
    /// 取消进行中。
    Cancelling,
    /// 已取消（终态）。
    Cancelled,
    /// 失败（终态）。
    Failed,
}

impl WorkerState {
    /// 是否为终态（不可再转换）。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    /// 是否为活动态（非终态）。
    pub const fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

impl fmt::Display for WorkerState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Created => "created",
            Self::Admitted => "admitted",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        };
        formatter.write_str(name)
    }
}

/// 状态机命名转换。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerTransition {
    /// Created → Admitted。
    Admit,
    /// Admitted → Starting。
    Start,
    /// Starting → Running。
    BeginRunning,
    /// Running → Waiting。
    BeginWaiting,
    /// Waiting → Running。
    Resume,
    /// Starting / Running / Waiting → Completed。
    Complete,
    /// 任意活动态 → Cancelling。
    BeginCancel,
    /// Cancelling → Cancelled。
    Cancel,
    /// 任意活动态 → Failed。
    Fail,
}

/// 生命周期错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    /// 非法转换（来源状态不允许该转换）。
    #[error("illegal transition {transition:?} from {from:?}")]
    IllegalTransition {
        /// 来源状态。
        from: WorkerState,
        /// 被拒绝的转换。
        transition: WorkerTransition,
    },
    /// 终态拒绝一切转换。
    #[error("transition from terminal state")]
    FromTerminal,
}

/// 状态机纯函数：给定来源状态与转换，返回目标状态。
pub fn transition(from: WorkerState, t: WorkerTransition) -> Result<WorkerState, LifecycleError> {
    if from.is_terminal() {
        return Err(LifecycleError::FromTerminal);
    }
    use WorkerState::*;
    use WorkerTransition::*;
    let to = match (from, t) {
        (Created, Admit) => Admitted,
        (Admitted, Start) => Starting,
        (Starting, BeginRunning) => Running,
        (Running, BeginWaiting) => Waiting,
        (Waiting, Resume) => Running,
        (Starting | Running | Waiting, Complete) => Completed,
        (state, BeginCancel) if state.is_active() => Cancelling,
        (Cancelling, Cancel) => Cancelled,
        (state, Fail) if state.is_active() => Failed,
        _ => {
            return Err(LifecycleError::IllegalTransition {
                from,
                transition: t,
            });
        }
    };
    Ok(to)
}

/// 一次成功转换对应的事件提示，供调用方发出可重放事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventHint {
    /// 无对应事件。
    None,
    /// WorkerAdmitted。
    WorkerAdmitted,
    /// WorkerStarted。
    WorkerStarted,
    /// WorkerRunning。
    WorkerRunning,
    /// WorkerWaiting。
    WorkerWaiting,
    /// WorkerCompleted。
    WorkerCompleted,
    /// WorkerCancelled。
    WorkerCancelled,
    /// WorkerFailed。
    WorkerFailed,
}

/// Worker 状态机封装：持有当前状态，逐次应用转换。
#[derive(Clone, Debug)]
pub struct WorkerStateMachine {
    state: WorkerState,
}

impl WorkerStateMachine {
    /// 从既有状态构造（恢复 / 重放用）。
    pub fn from_state(state: WorkerState) -> Self {
        Self { state }
    }

    /// 当前状态。
    pub fn state(&self) -> WorkerState {
        self.state
    }

    /// 应用一次转换；成功时返回（新状态，事件提示）并更新内部状态。
    pub fn apply(
        &mut self,
        t: WorkerTransition,
    ) -> Result<(WorkerState, EventHint), LifecycleError> {
        let next = transition(self.state, t)?;
        let hint = match t {
            WorkerTransition::Admit => EventHint::WorkerAdmitted,
            WorkerTransition::Start => EventHint::WorkerStarted,
            WorkerTransition::BeginRunning => EventHint::WorkerRunning,
            WorkerTransition::BeginWaiting => EventHint::WorkerWaiting,
            WorkerTransition::Complete => EventHint::WorkerCompleted,
            WorkerTransition::Cancel => EventHint::WorkerCancelled,
            WorkerTransition::Fail => EventHint::WorkerFailed,
            WorkerTransition::BeginCancel | WorkerTransition::Resume => EventHint::None,
        };
        self.state = next;
        Ok((next, hint))
    }
}

/// 编排事件（可持久化、可重放，ADR-016）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum OrchestrationEvent {
    /// worker 注册表条目创建。
    WorkerCreated {
        /// agent 标识。
        agent_id: AgentId,
        /// 租户。
        tenant_id: TenantId,
        /// 父代理。
        parent_id: Option<AgentId>,
        /// 角色。
        role: WorkerRole,
        /// 会话。
        session_id: SessionId,
        /// worktree 路径（可选）。
        worktree_path: Option<String>,
        /// 创建时间。
        created_at_ms: u64,
    },
    /// 已准入。
    WorkerAdmitted {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 已启动。
    WorkerStarted {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 进入运行。
    WorkerRunning {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 进入等待。
    WorkerWaiting {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 正常完成。
    WorkerCompleted {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 开始取消。
    WorkerCancelling {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 已取消。
    WorkerCancelled {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
    },
    /// 失败。
    WorkerFailed {
        /// agent 标识。
        agent_id: AgentId,
        /// 时间。
        at_ms: u64,
        /// 失败原因。
        reason: String,
    },
    /// 任务创建。
    TaskCreated {
        /// 任务标识。
        task_id: TaskId,
        /// 负责 agent。
        agent_id: AgentId,
        /// 租户。
        tenant_id: TenantId,
    },
    /// 任务就绪。
    TaskReady {
        /// 任务标识。
        task_id: TaskId,
    },
    /// 任务指派。
    TaskAssigned {
        /// 任务标识。
        task_id: TaskId,
        /// 被指派 agent。
        agent_id: AgentId,
    },
    /// 任务完成。
    TaskCompleted {
        /// 任务标识。
        task_id: TaskId,
    },
    /// 任务失败。
    TaskFailed {
        /// 任务标识。
        task_id: TaskId,
        /// 失败原因。
        reason: String,
    },
    /// 任务重试（attempt 从 1 起）。
    TaskRetried {
        /// 任务标识。
        task_id: TaskId,
        /// 本次重试序号。
        attempt: u32,
    },
    /// 任务取消。
    TaskCancelled {
        /// 任务标识。
        task_id: TaskId,
    },
    /// worker 预算超限。
    BudgetExceeded {
        /// agent 标识。
        agent_id: AgentId,
        /// 维度（input_tokens / output_tokens / cost_micros）。
        dimension: String,
        /// 已使用量。
        used: u64,
        /// 上限。
        limit: u64,
    },
    /// 并发被拒。
    ConcurrencyDenied {
        /// 维度（agents / requests / leases）。
        kind: String,
        /// 当前并发。
        current: u64,
        /// 上限。
        limit: u64,
    },
    /// worker 提出 patch。
    PatchProposed {
        /// agent 标识。
        agent_id: AgentId,
        /// 改动文件列表。
        files: Vec<String>,
    },
    /// patch 已合并。
    PatchMerged {
        /// agent 标识。
        agent_id: AgentId,
        /// 合并的文件。
        files: Vec<String>,
    },
    /// patch 冲突，等待 parent 决定。
    PatchConflict {
        /// agent 标识。
        agent_id: AgentId,
        /// 冲突文件。
        files: Vec<String>,
    },
}

/// 从事件流重建每个 agent 的 worker 状态（事件溯源，ADR-016）。
///
/// 未知事件（任务 / 预算 / patch 事件以及未来新增变体）被忽略。
pub fn replay_workers(events: &[OrchestrationEvent]) -> BTreeMap<AgentId, WorkerState> {
    let mut states: BTreeMap<AgentId, WorkerState> = BTreeMap::new();
    for event in events {
        match event {
            OrchestrationEvent::WorkerCreated { agent_id, .. } => {
                states.insert(agent_id.clone(), WorkerState::Created);
            }
            OrchestrationEvent::WorkerAdmitted { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::Admit);
            }
            OrchestrationEvent::WorkerStarted { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::Start);
            }
            OrchestrationEvent::WorkerRunning { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::BeginRunning);
            }
            OrchestrationEvent::WorkerWaiting { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::BeginWaiting);
            }
            OrchestrationEvent::WorkerCompleted { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::Complete);
            }
            OrchestrationEvent::WorkerCancelling { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::BeginCancel);
            }
            OrchestrationEvent::WorkerCancelled { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::Cancel);
            }
            OrchestrationEvent::WorkerFailed { agent_id, .. } => {
                apply_replay(&mut states, agent_id, WorkerTransition::Fail);
            }
            // 任务 / 预算 / patch 事件与未知事件：与 worker 状态无关，忽略。
            _ => {}
        }
    }
    states
}

/// 重放辅助：对已知 agent 应用一次转换；非法转换（如终态后的迟到事件）静默跳过。
///
/// 重放是容错的：终态事件（Complete / Cancel / Fail）直接落到对应终态，
/// 不要求中间事件完整（事件日志可能因崩溃截断）；非终态事件仍走严格状态机，
/// 非法时静默跳过。终态之后的迟到事件一律忽略。
fn apply_replay(
    states: &mut BTreeMap<AgentId, WorkerState>,
    agent_id: &AgentId,
    t: WorkerTransition,
) {
    let Some(state) = states.get_mut(agent_id) else {
        return;
    };
    let next = match t {
        WorkerTransition::Complete => Some(WorkerState::Completed),
        WorkerTransition::Cancel => Some(WorkerState::Cancelled),
        WorkerTransition::Fail => Some(WorkerState::Failed),
        _ => transition(*state, t).ok(),
    };
    if let Some(next) = next {
        *state = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::WorkerRole;

    fn created_event(agent: &str) -> OrchestrationEvent {
        OrchestrationEvent::WorkerCreated {
            agent_id: AgentId::new(agent),
            tenant_id: TenantId::new("tenant-a"),
            parent_id: None,
            role: WorkerRole::Parent,
            session_id: SessionId::new("session-1"),
            worktree_path: None,
            created_at_ms: 1_000,
        }
    }

    #[test]
    fn happy_path_transitions() {
        assert_eq!(
            transition(WorkerState::Created, WorkerTransition::Admit),
            Ok(WorkerState::Admitted)
        );
        assert_eq!(
            transition(WorkerState::Admitted, WorkerTransition::Start),
            Ok(WorkerState::Starting)
        );
        assert_eq!(
            transition(WorkerState::Starting, WorkerTransition::BeginRunning),
            Ok(WorkerState::Running)
        );
        assert_eq!(
            transition(WorkerState::Running, WorkerTransition::BeginWaiting),
            Ok(WorkerState::Waiting)
        );
        assert_eq!(
            transition(WorkerState::Waiting, WorkerTransition::Resume),
            Ok(WorkerState::Running)
        );
    }

    #[test]
    fn complete_allowed_from_starting_running_waiting() {
        for from in [
            WorkerState::Starting,
            WorkerState::Running,
            WorkerState::Waiting,
        ] {
            assert_eq!(
                transition(from, WorkerTransition::Complete),
                Ok(WorkerState::Completed)
            );
        }
        assert!(matches!(
            transition(WorkerState::Created, WorkerTransition::Complete),
            Err(LifecycleError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn cancel_and_fail_from_any_active_state() {
        for from in [
            WorkerState::Created,
            WorkerState::Admitted,
            WorkerState::Starting,
            WorkerState::Running,
            WorkerState::Waiting,
            WorkerState::Cancelling,
        ] {
            assert_eq!(
                transition(from, WorkerTransition::BeginCancel),
                Ok(WorkerState::Cancelling)
            );
            assert_eq!(
                transition(from, WorkerTransition::Fail),
                Ok(WorkerState::Failed)
            );
        }
        assert_eq!(
            transition(WorkerState::Cancelling, WorkerTransition::Cancel),
            Ok(WorkerState::Cancelled)
        );
    }

    #[test]
    fn terminal_rejects_everything() {
        for terminal in [
            WorkerState::Completed,
            WorkerState::Cancelled,
            WorkerState::Failed,
        ] {
            for t in [
                WorkerTransition::Admit,
                WorkerTransition::Start,
                WorkerTransition::BeginRunning,
                WorkerTransition::BeginWaiting,
                WorkerTransition::Resume,
                WorkerTransition::Complete,
                WorkerTransition::BeginCancel,
                WorkerTransition::Cancel,
                WorkerTransition::Fail,
            ] {
                assert_eq!(
                    transition(terminal, t),
                    Err(LifecycleError::FromTerminal),
                    "terminal {terminal:?} must reject {t:?}"
                );
            }
        }
    }

    #[test]
    fn machine_apply_reports_hints() {
        let mut machine = WorkerStateMachine::from_state(WorkerState::Created);
        let (state, hint) = machine.apply(WorkerTransition::Admit).unwrap();
        assert_eq!(state, WorkerState::Admitted);
        assert_eq!(hint, EventHint::WorkerAdmitted);
        let (state, hint) = machine.apply(WorkerTransition::Start).unwrap();
        assert_eq!(state, WorkerState::Starting);
        assert_eq!(hint, EventHint::WorkerStarted);
        assert_eq!(machine.state(), WorkerState::Starting);
        let err = machine.apply(WorkerTransition::Admit).unwrap_err();
        assert!(matches!(err, LifecycleError::IllegalTransition { .. }));
    }

    #[test]
    fn replay_rebuilds_worker_states_and_ignores_unknown_events() {
        let events = vec![
            created_event("a"),
            OrchestrationEvent::WorkerAdmitted {
                agent_id: AgentId::new("a"),
                at_ms: 1,
            },
            OrchestrationEvent::WorkerStarted {
                agent_id: AgentId::new("a"),
                at_ms: 2,
            },
            OrchestrationEvent::WorkerRunning {
                agent_id: AgentId::new("a"),
                at_ms: 3,
            },
            created_event("b"),
            OrchestrationEvent::WorkerAdmitted {
                agent_id: AgentId::new("b"),
                at_ms: 4,
            },
            // 与 worker 状态无关的事件应被忽略。
            OrchestrationEvent::TaskCreated {
                task_id: TaskId::new("t1"),
                agent_id: AgentId::new("a"),
                tenant_id: TenantId::new("tenant-a"),
            },
            OrchestrationEvent::BudgetExceeded {
                agent_id: AgentId::new("a"),
                dimension: "input_tokens".into(),
                used: 10,
                limit: 5,
            },
        ];
        let states = replay_workers(&events);
        assert_eq!(states.len(), 2);
        assert_eq!(states[&AgentId::new("a")], WorkerState::Running);
        assert_eq!(states[&AgentId::new("b")], WorkerState::Admitted);
    }

    #[test]
    fn replay_ignores_late_events_after_terminal() {
        let events = vec![
            created_event("a"),
            OrchestrationEvent::WorkerCompleted {
                agent_id: AgentId::new("a"),
                at_ms: 1,
            },
            // 终态后的迟到事件必须被忽略（静默跳过）。
            OrchestrationEvent::WorkerRunning {
                agent_id: AgentId::new("a"),
                at_ms: 2,
            },
        ];
        let states = replay_workers(&events);
        assert_eq!(states[&AgentId::new("a")], WorkerState::Completed);
    }

    #[test]
    fn events_serialize_snake_case_tagged() {
        let event = OrchestrationEvent::WorkerFailed {
            agent_id: AgentId::new("a"),
            at_ms: 5,
            reason: "boom".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "worker_failed");
        assert_eq!(json["data"]["reason"], "boom");
        let back: OrchestrationEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, event);
    }
}
