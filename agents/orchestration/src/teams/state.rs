//! Team 聚合状态 + event-sourcing `apply` / `replay`。
//!
//! [`TeamAggregate`] 是单个 team 的可重放投影：成员、共享任务板、mailbox、
//! presence、peer fan-out 计数、plan 审批。[`apply`] 是纯函数折叠，是崩溃
//! 恢复的唯一入口（与 plan-service / agent-events 的 event-sourcing 约定一致，
//! by-reference + 非 Copy 字段 `.clone()`）。plan 审批复用
//! `pawork_workflow::plan::PlanState` 的状态机折叠，避免重复实现 P16-2 review 状态机。

use std::collections::{BTreeMap, BTreeSet};

use pawork_domain::{AgentId, PlanId, TenantId};
use crate::{TaskId, TaskState};
use pawork_workflow::plan::PlanState;

use crate::teams::event::{BoardTask, Recipients, TeamEvent, TeamEventEnvelope};
use crate::teams::ids::{MailboxMessageId, MemberRole, TeamId};
use crate::teams::presence::Presence;

/// mailbox 中一条消息的投影。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxEntry {
    pub message_id: MailboxMessageId,
    pub sender: AgentId,
    pub recipients: Recipients,
    pub body: String,
    /// 已投递（拉取）的收件人集合。
    pub delivered_to: BTreeSet<AgentId>,
    /// 已读的收件人集合。
    pub read_by: BTreeSet<AgentId>,
    /// peer fan-out 路由消息的精确目标集；`None` = 普通 mailbox 消息或空目标
    /// 路由（空目标不计数，也不参与完成判定）。
    ///
    /// 全部目标 delivered 后 [`TeamAggregate::active_fan_out`] 对发送者递减
    /// **恰好一次**（fold 内判定，durable / replay deterministic）；目标因
    /// 成员移除收窄为空（消息已无人可收）同样视为完成。目标集在路由时由
    /// [`crate::teams::peer::PeerPolicy`] 展开并随事件持久化。
    pub routed_targets: Option<BTreeSet<AgentId>>,
    /// 该 fan-out 是否已结算（计数已递减）。恰好一次的判定位：完成即置位，
    /// 后续同消息事件（重复 mark_read、成员移除后到达的投递）不再重复递减。
    pub fan_out_complete: bool,
}

/// plan 审批投影：复用 P16-2 `PlanState` + 当前 version。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanApprovalEntry {
    pub state: PlanState,
}

/// Team 聚合状态（`apply` 折叠结果；不执行 IO）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TeamAggregate {
    pub team_id: Option<TeamId>,
    pub tenant_id: Option<TenantId>,
    pub supervisor: Option<AgentId>,
    pub name: Option<String>,
    pub members: BTreeMap<AgentId, MemberRole>,
    pub dissolved: bool,
    pub board: BTreeMap<TaskId, BoardTask>,
    pub mailbox: BTreeMap<MailboxMessageId, MailboxEntry>,
    pub presence: BTreeMap<AgentId, Presence>,
    /// 每成员当前活跃（已路由、尚未全部投递）的 peer fan-out 计数。
    pub active_fan_out: BTreeMap<AgentId, u64>,
    pub plans: BTreeMap<PlanId, PlanApprovalEntry>,
    /// 下一个事件序列（持久化由 service 分配，state 仅在 apply 时跟随推进）。
    pub next_sequence: u64,
}

impl TeamAggregate {
    /// team 是否已创建（含 team_id）。
    pub fn exists(&self) -> bool {
        self.team_id.is_some()
    }

    /// 成员集合（用于 policy 鉴权）。
    pub fn member_set(&self) -> BTreeSet<AgentId> {
        self.members.keys().cloned().collect()
    }

    /// 解析某消息对指定成员是否可见（直连包含 / 广播非发送者）。
    pub fn is_recipient(&self, entry: &MailboxEntry, agent: &AgentId) -> bool {
        match &entry.recipients {
            Recipients::Direct { members } => members.contains(agent),
            Recipients::Broadcast => &entry.sender != agent,
        }
    }

    /// 某任务当前依赖是否全部完成（用于认领就绪判定）。
    pub fn dependencies_satisfied(&self, task: &BoardTask) -> bool {
        task.depends_on.iter().all(|dep| {
            self.board
                .get(dep)
                .is_some_and(|d| d.state == TaskState::Completed)
        })
    }
}

/// 把一个 canonical team 事件折叠进聚合（恢复入口，纯函数）。
///
/// `apply` 视事件为已校验事实，不再重复命令面校验；service 层完成校验后调用。
pub fn apply(state: &mut TeamAggregate, event: TeamEvent) {
    match event {
        TeamEvent::TeamCreated {
            team_id,
            tenant_id,
            supervisor,
            name,
        } => {
            state.team_id = Some(team_id.clone());
            state.tenant_id = Some(tenant_id.clone());
            state.supervisor = Some(supervisor.clone());
            state.name = Some(name.clone());
            state.members.insert(supervisor, MemberRole::Supervisor);
            state.next_sequence = state.next_sequence.max(1);
        }
        TeamEvent::MemberAdded {
            team_id: _,
            agent_id,
            role,
        } => {
            state.members.insert(agent_id, role);
        }
        TeamEvent::MemberRemoved {
            team_id: _,
            agent_id,
        } => {
            state.members.remove(&agent_id);
            state.presence.remove(&agent_id);
            // 已离开成员的进行中 fan-out 一并作废：计数留在表里只会让重入的
            // 成员被陈旧计数误伤（policy 并发上限），且这些消息已无人推进。
            state.active_fan_out.remove(&agent_id);
            // 以离开成员为目标的进行中 route：从冻结目标集剔除。目标收窄后
            // 若已无人可收（目标清空）或剩余目标已全部投递，则本次移除即
            // 完成点——发送者计数递减一次，避免「永不投递的目标」导致的
            // 永久泄漏。重放同一事件序列得到同一结算点。
            let affected: Vec<MailboxMessageId> = {
                let mut ids = Vec::new();
                for entry in state.mailbox.values_mut() {
                    if let Some(targets) = entry.routed_targets.as_mut() {
                        if targets.remove(&agent_id) {
                            ids.push(entry.message_id.clone());
                        }
                    }
                }
                ids
            };
            for id in affected {
                maybe_complete_fan_out(state, &id);
            }
        }
        TeamEvent::TeamDissolved { team_id: _ } => {
            state.dissolved = true;
        }

        TeamEvent::TaskPosted { team_id: _, task } => {
            state.board.insert(task.task_id.clone(), task.clone());
        }
        TeamEvent::TaskClaimed {
            team_id: _,
            task_id,
            claimer,
        } => {
            if let Some(task) = state.board.get_mut(&task_id) {
                task.owner = Some(claimer);
                task.state = TaskState::Assigned;
            }
        }
        TeamEvent::TaskReleased {
            team_id: _,
            task_id,
            by: _,
        } => {
            let deps_ok = state
                .board
                .get(&task_id)
                .is_some_and(|task| state.dependencies_satisfied(task));
            if let Some(task) = state.board.get_mut(&task_id) {
                task.owner = None;
                task.state = if deps_ok {
                    TaskState::Ready
                } else {
                    TaskState::Created
                };
            }
        }
        TeamEvent::TaskAdvanced {
            team_id: _,
            task_id,
            state: new_state,
        } => {
            if let Some(task) = state.board.get_mut(&task_id) {
                // auto-retry 是两条原子事件：TaskAdvanced{Failed} 之后紧跟
                // TaskAdvanced{Ready}。折叠只做纯函数翻译——Failed 后出现的
                // Ready（且仍有重试预算）即一次重试，计入 retry_count。
                if new_state == TaskState::Ready
                    && task.state == TaskState::Failed
                    && task.retry_count < task.max_retries
                {
                    task.retry_count += 1;
                }
                task.state = new_state;
            }
        }

        TeamEvent::MailboxPosted {
            team_id: _,
            message_id,
            sender,
            recipients,
            body,
        } => {
            state.mailbox.insert(
                message_id.clone(),
                MailboxEntry {
                    message_id: message_id.clone(),
                    sender,
                    recipients,
                    body,
                    delivered_to: BTreeSet::new(),
                    read_by: BTreeSet::new(),
                    routed_targets: None,
                    fan_out_complete: false,
                },
            );
        }
        TeamEvent::MailboxDelivered {
            team_id: _,
            message_id,
            recipient,
        } => {
            if let Some(entry) = state.mailbox.get_mut(&message_id) {
                entry.delivered_to.insert(recipient);
            }
            maybe_complete_fan_out(state, &message_id);
        }
        TeamEvent::MailboxRead {
            team_id: _,
            message_id,
            by,
        } => {
            if let Some(entry) = state.mailbox.get_mut(&message_id) {
                entry.read_by.insert(by.clone());
                entry.delivered_to.insert(by);
            }
            maybe_complete_fan_out(state, &message_id);
        }

        TeamEvent::PresenceChanged {
            team_id: _,
            agent_id,
            presence,
        } => {
            state.presence.insert(agent_id, presence);
        }

        TeamEvent::PeerMessageRouted {
            team_id: _,
            message_id,
            sender,
            recipients,
            body,
            ..
        } => {
            // 路由事件的目标集是策略展开后的精确事实：直连即成员表，广播在
            // 路由时已展开为 Direct（旧事件的 Broadcast 按当前成员折叠，
            // 重放确定性不变）。fan-out 完成判定只看这个冻结目标集。
            let targets: BTreeSet<AgentId> = match &recipients {
                Recipients::Direct { members } => members.iter().cloned().collect(),
                Recipients::Broadcast => state
                    .member_set()
                    .into_iter()
                    .filter(|m| m != &sender)
                    .collect(),
            };
            // 空目标（策略放行但无人可收）没有可完成的投递：不计入并发，
            // 避免永不递减的陈旧计数；`routed_targets` 置 `None`（`Some`
            // 恒为非空目标集），完成判定自然跳过该条目。
            let routed_targets = if targets.is_empty() {
                None
            } else {
                Some(targets.clone())
            };
            state.mailbox.insert(
                message_id.clone(),
                MailboxEntry {
                    message_id: message_id.clone(),
                    sender: sender.clone(),
                    recipients,
                    body,
                    delivered_to: BTreeSet::new(),
                    read_by: BTreeSet::new(),
                    routed_targets,
                    fan_out_complete: false,
                },
            );
            if !targets.is_empty() {
                *state.active_fan_out.entry(sender).or_insert(0) += 1;
            }
        }
        TeamEvent::FanOutDenied { .. } => {
            // 拒绝仅作为审计事实，不改投影。
        }

        TeamEvent::PlanSubmitted {
            team_id: _,
            plan_id,
            version,
            title,
            steps,
        } => {
            use pawork_domain::PlanEvent;
            let entry = state.plans.entry(plan_id.clone()).or_default();
            pawork_workflow::plan::apply(
                &mut entry.state,
                &PlanEvent::Created {
                    plan_id: plan_id.clone(),
                    version: version.clone(),
                    title: title.clone(),
                    steps: steps.clone(),
                },
            );
            pawork_workflow::plan::apply(
                &mut entry.state,
                &PlanEvent::ReviewRequested {
                    plan_id: plan_id.clone(),
                    version: version.clone(),
                },
            );
        }
        TeamEvent::PlanApproved {
            team_id: _,
            plan_id,
            version,
            checkpoint_id,
        } => {
            use pawork_domain::PlanEvent;
            let entry = state.plans.entry(plan_id.clone()).or_default();
            pawork_workflow::plan::apply(
                &mut entry.state,
                &PlanEvent::Approved {
                    plan_id: plan_id.clone(),
                    version: version.clone(),
                    checkpoint_id,
                },
            );
        }
        TeamEvent::PlanRejected {
            team_id: _,
            plan_id,
            version,
            reason,
        } => {
            use pawork_domain::PlanEvent;
            let entry = state.plans.entry(plan_id.clone()).or_default();
            pawork_workflow::plan::apply(
                &mut entry.state,
                &PlanEvent::Rejected {
                    plan_id: plan_id.clone(),
                    version: version.clone(),
                    reason: reason.clone(),
                },
            );
        }
        TeamEvent::PlanCommented {
            team_id: _,
            plan_id,
            version,
            anchor,
            body,
        } => {
            use pawork_domain::PlanEvent;
            let entry = state.plans.entry(plan_id.clone()).or_default();
            pawork_workflow::plan::apply(
                &mut entry.state,
                &PlanEvent::CommentAdded {
                    plan_id: plan_id.clone(),
                    version: version.clone(),
                    anchor: anchor.clone(),
                    body: body.clone(),
                },
            );
        }
    }
}

/// 判定一次 peer fan-out 是否已结算；是则对发送者递减计数。
///
/// 结算条件：`routed_targets` 为 `Some`（路由时非空目标集，已计入计数）且
/// 目标全部 delivered，或目标因成员移除收窄为空（无人可收）。`fan_out_complete`
/// 置位保证同一 fan-out 恰好递减一次——`delivered_to` 是超集判定、只增不减，
/// 没有该位时消息完成后的后续 `MailboxRead` / 重复 `mark_read` 会重复递减
/// 发送者计数。重放同一事件序列得到同一递减点（durable / replay
/// deterministic）；递减饱和防下溢。
fn maybe_complete_fan_out(state: &mut TeamAggregate, message_id: &MailboxMessageId) {
    // 先做不可变判定再修改，避免聚合借用冲突。
    let complete = state.mailbox.get(message_id).and_then(|entry| {
        if entry.fan_out_complete {
            return None;
        }
        entry.routed_targets.as_ref().map(|targets| {
            (
                entry.sender.clone(),
                // `Some` 恒为非空目标集；空集只可能来自成员移除后的收窄。
                targets.is_empty() || entry.delivered_to.is_superset(targets),
            )
        })
    });
    if let Some((sender, true)) = complete {
        if let Some(entry) = state.mailbox.get_mut(message_id) {
            entry.fan_out_complete = true;
        }
        decrement_fan_out(state, &sender);
    }
}

/// 对发送者递减一次活跃 fan-out 计数（饱和防下溢，归零移除）。
fn decrement_fan_out(state: &mut TeamAggregate, sender: &AgentId) {
    let counter = state.active_fan_out.entry(sender.clone()).or_insert(0);
    *counter = counter.saturating_sub(1);
    if *counter == 0 {
        state.active_fan_out.remove(sender);
    }
}

/// 从事件信封序列重放重建聚合（恢复入口）。
pub fn replay<'a>(envelopes: impl IntoIterator<Item = &'a TeamEventEnvelope>) -> TeamAggregate {
    let mut state = TeamAggregate::default();
    for envelope in envelopes {
        apply(&mut state, envelope.payload.clone());
        // checked 语义：序列接近 u64 上限时饱和推进，溢出留给 service 层
        // （`commit` 的 checked 预检）显式报 `IdSpaceExhausted`，重放本身不 panic。
        state.next_sequence = state
            .next_sequence
            .max(envelope.sequence.value().saturating_add(1));
    }
    state
}

/// 任务板状态机合法转移判定（命令面校验用；复用 P12 TaskState 语义）。
pub fn is_legal_task_transition(from: TaskState, to: TaskState) -> bool {
    use TaskState::*;
    matches!(
        (from, to),
        (Created, Ready)
            | (Created, Assigned)
            | (Ready, Assigned)
            | (Assigned, Running)
            | (Running, Blocked)
            | (Running, Completed)
            | (Running, Failed)
            | (Blocked, Running)
            | (Blocked, Ready)
            | (Assigned, Ready)
            | (Ready, Cancelled)
            | (Assigned, Cancelled)
            | (Running, Cancelled)
            | (Blocked, Cancelled)
            | (Failed, Ready)
    )
}
