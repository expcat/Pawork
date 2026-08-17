//! TeamService：team 协作层的命令面与查询面 facade。
//!
//! 装配一个 [`TeamService`] 即获得全部协作语义：team 生命周期 / 共享任务板 /
//! 持久 mailbox / presence / 受控 peer messaging / plan 审批。所有命令统一
//! 流程：**校验 → 构造 canonical [`TeamEvent`] → durable append → `apply`
//! 折叠 + 推进序列 → 经注入 [`TeamEventSink`] 镜像投递**。持久化失败时
//! **内存状态与序列完全不改变**（persist-first，可安全重试）。不写 run loop、
//! 不自建 broadcast、不派发后台任务（执行权威归 `task-manager`）。
//!
//! # 重放
//! [`TeamService::from_store`]（重启恢复入口）从注入的 durable
//! [`TeamEventStore`] 全量重放并重建内存状态（校验每 team 序列连续）；
//! [`TeamService::from_envelopes`] 是同一重建逻辑的内存版（测试 / 自省用）。
//! 重放同时恢复 `next_local_id`（取已用 `msg-N` / `fanout-N` 后缀的最大值），
//! 重启后 ID 继续递增、绝不归零复用；序列 / ID 分配全部 checked 溢出，
//! 到达 u64 上限显式报 [`TeamError::IdSpaceExhausted`] 且不落盘。
//! 与 app-service 唯一 EventHub 的崩溃恢复共享同一事实源。

use std::collections::BTreeMap;
use std::sync::Arc;

use pawork_domain::{
    AgentId, CheckpointId, EventId, PlanCommentAnchor, PlanId, PlanStepSnapshot, PlanVersionId,
    TenantId, Timestamp,
};
use crate::{OrchestrationEvent, TaskId, TaskState, WorkerState};
use parking_lot::Mutex;

use crate::teams::approval;
use crate::teams::error::TeamError;
use crate::teams::event::{
    NullTeamSink, Recipients, TeamEvent, TeamEventEnvelope, TeamEventSequence, TeamEventSink,
};
use crate::teams::ids::{FanOutId, MailboxMessageId, MemberRole, TeamId};
use crate::teams::mailbox;
use crate::teams::peer::PeerPolicy;
use crate::teams::presence::{self, Presence};
use crate::teams::state::{replay, TeamAggregate};
use crate::teams::store::{shared_store, validate_sequence_contiguity, MemoryTeamStore, TeamEventStore};
use crate::teams::task_board;

/// 单 team 的运行态：聚合投影 + 序列号分配 + 本地计数源。
struct TeamRuntime {
    aggregate: TeamAggregate,
    /// 下一条事件的序列号（唯一权威，独立于 aggregate 的快照字段）。
    next_sequence: u64,
    /// 本地计数源（mailbox message ID 与 fan-out ID 共用同一计数器）。
    /// 重放时从已持久化事件恢复（取已用 `msg-N` / `fanout-N` 后缀的最大值），
    /// 重启后继续递增，**绝不归零复用**（归零会导致 msg / fan-out ID 重用）。
    next_local_id: u64,
}

/// Team 协作服务（线程安全；内部 `Mutex`）。
pub struct TeamService {
    inner: Mutex<Inner>,
    store: Arc<dyn TeamEventStore>,
    sink: Arc<dyn TeamEventSink>,
    peer_policy: PeerPolicy,
}

struct Inner {
    teams: BTreeMap<TeamId, TeamRuntime>,
}

impl Default for TeamService {
    fn default() -> Self {
        Self::with_sink_and_policy(Arc::new(NullTeamSink), PeerPolicy::default())
    }
}

impl TeamService {
    /// 创建空服务（NullTeamSink，默认 peer 策略）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入事件 sink（app-service 唯一 EventHub 适配器）与 peer 策略。
    pub fn with_sink_and_policy(sink: Arc<dyn TeamEventSink>, peer_policy: PeerPolicy) -> Self {
        Self::with_store_sink_and_policy(shared_store(MemoryTeamStore::new()), sink, peer_policy)
    }

    /// 注入 durable [`TeamEventStore`]、事件 sink 与 peer 策略。
    ///
    /// 所有命令先经 `store.append` 落盘，成功后才折叠状态 / 推进序列 / 镜像
    /// sink；失败返回错误且状态不变。
    pub fn with_store_sink_and_policy(
        store: Arc<dyn TeamEventStore>,
        sink: Arc<dyn TeamEventSink>,
        peer_policy: PeerPolicy,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner {
                teams: BTreeMap::new(),
            }),
            store,
            sink,
            peer_policy,
        }
    }

    /// 从已持久化事件信封重放重建服务（恢复入口）。
    pub fn from_envelopes(
        envelopes: Vec<TeamEventEnvelope>,
        sink: Arc<dyn TeamEventSink>,
        peer_policy: PeerPolicy,
    ) -> Self {
        Self::from_envelopes_with_store(
            envelopes,
            shared_store(MemoryTeamStore::new()),
            sink,
            peer_policy,
        )
    }

    /// 以既有 durable store 从信封重建（重放 + 继续追加用同一 store）。
    pub fn from_envelopes_with_store(
        envelopes: Vec<TeamEventEnvelope>,
        store: Arc<dyn TeamEventStore>,
        sink: Arc<dyn TeamEventSink>,
        peer_policy: PeerPolicy,
    ) -> Self {
        let mut teams: BTreeMap<TeamId, TeamRuntime> = BTreeMap::new();
        let mut grouped: BTreeMap<TeamId, Vec<TeamEventEnvelope>> = BTreeMap::new();
        for env in envelopes {
            grouped.entry(env.team_id.clone()).or_default().push(env);
        }
        for (team_id, envs) in grouped {
            let aggregate = replay(envs.iter());
            let next_sequence = envs
                .iter()
                .map(|e| e.sequence)
                .max()
                .map(|s| s.value().saturating_add(1))
                .unwrap_or(1);
            teams.insert(
                team_id,
                TeamRuntime {
                    aggregate,
                    next_sequence,
                    next_local_id: max_local_id(&envs),
                },
            );
        }
        Self {
            inner: Mutex::new(Inner { teams }),
            store,
            sink,
            peer_policy,
        }
    }

    /// 重启恢复入口：从 durable store 全量重放并重建服务。
    ///
    /// 重放前校验每 team 序列连续（append-only 不变量）；失败返回错误，
    /// 不构造半初始化服务。恢复后的服务继续向同一 store 追加。
    pub fn from_store(
        store: Arc<dyn TeamEventStore>,
        sink: Arc<dyn TeamEventSink>,
        peer_policy: PeerPolicy,
    ) -> Result<Self, TeamError> {
        let envelopes = store.replay()?;
        validate_sequence_contiguity(&envelopes)?;
        Ok(Self::from_envelopes_with_store(
            envelopes,
            store,
            sink,
            peer_policy,
        ))
    }

    // ---------------- 生命周期 ----------------

    /// 创建 team；`supervisor` 自动成为首个成员（Supervisor 角色）。
    pub fn create_team(
        &self,
        team_id: TeamId,
        tenant_id: TenantId,
        supervisor: &AgentId,
        name: String,
    ) -> Result<TeamEvent, TeamError> {
        if name.trim().is_empty() {
            return Err(TeamError::EmptyText);
        }
        let mut inner = self.inner.lock();
        if inner.teams.contains_key(&team_id) {
            return Err(TeamError::AlreadyMember {
                team_id: team_id.clone(),
                agent_id: (*supervisor).clone(),
            });
        }
        let event = TeamEvent::TeamCreated {
            team_id: team_id.clone(),
            tenant_id,
            supervisor: supervisor.clone(),
            name,
        };
        let mut rt = TeamRuntime {
            aggregate: TeamAggregate::default(),
            next_sequence: 1,
            next_local_id: 0,
        };
        self.commit(&team_id, &mut rt, event.clone())?;
        inner.teams.insert(team_id, rt);
        Ok(event)
    }

    /// 增加成员；仅 supervisor 可调用，成员不能已存在。
    pub fn add_member(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        agent_id: &AgentId,
        role: MemberRole,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_supervisor(rt, by)?;
        if rt.aggregate.members.contains_key(agent_id) {
            return Err(TeamError::AlreadyMember {
                team_id: team_id.clone(),
                agent_id: (*agent_id).clone(),
            });
        }
        let event = TeamEvent::MemberAdded {
            team_id: team_id.clone(),
            agent_id: agent_id.clone(),
            role,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 移除成员；仅 supervisor 可调用。
    pub fn remove_member(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        agent_id: &AgentId,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_supervisor(rt, by)?;
        if !rt.aggregate.members.contains_key(agent_id) {
            return Err(TeamError::NotMember {
                team_id: team_id.clone(),
                agent_id: (*agent_id).clone(),
            });
        }
        // 防孤儿：最后一个 supervisor 不可移除（否则 team 无人可审批 /
        // 管理 / 解散）。先添加第二个 supervisor，再移除原 supervisor。
        if rt.aggregate.members[agent_id].is_supervisor() {
            let supervisor_count = rt
                .aggregate
                .members
                .values()
                .filter(|role| role.is_supervisor())
                .count();
            if supervisor_count <= 1 {
                return Err(TeamError::LastSupervisor(team_id.clone()));
            }
        }
        let event = TeamEvent::MemberRemoved {
            team_id: team_id.clone(),
            agent_id: agent_id.clone(),
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 解散 team；仅 supervisor 可调用。终态后拒绝一切写命令。
    pub fn dissolve_team(&self, team_id: &TeamId, by: &AgentId) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_supervisor(rt, by)?;
        let event = TeamEvent::TeamDissolved {
            team_id: team_id.clone(),
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    // ---------------- 共享任务板 ----------------

    /// 张贴任务到共享任务板。
    pub fn post_task(
        &self,
        team_id: &TeamId,
        poster: &AgentId,
        task_id: TaskId,
        description: String,
        depends_on: Vec<TaskId>,
        max_retries: u32,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, poster)?;
        let task = task_board::build_task(
            &rt.aggregate,
            task_id,
            poster.clone(),
            description,
            depends_on,
            max_retries,
        )?;
        let event = TeamEvent::TaskPosted {
            team_id: team_id.clone(),
            task,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 认领任务：任务未认领、依赖满足。
    pub fn claim_task(
        &self,
        team_id: &TeamId,
        claimer: &AgentId,
        task_id: TaskId,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, claimer)?;
        task_board::validate_claim(&rt.aggregate, &task_id)?;
        let event = TeamEvent::TaskClaimed {
            team_id: team_id.clone(),
            task_id,
            claimer: claimer.clone(),
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 释放任务（仅 owner）。
    pub fn release_task(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        task_id: TaskId,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, by)?;
        task_board::validate_release(&rt.aggregate, &task_id, by)?;
        let event = TeamEvent::TaskReleased {
            team_id: team_id.clone(),
            task_id,
            by: by.clone(),
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 推进任务状态（**严格 owner**：含终态在内只有认领者可推进；复用 P12
    /// TaskState 状态机）。owner 失联 / 任务搁浅时走显式 Supervisor override
    /// （[`Self::supervisor_advance_task`]）。
    ///
    /// 推进到 `Failed` 且仍有重试预算（`retry_count < max_retries`）时，命令
    /// 原子产出两条事件 `TaskAdvanced{Failed}` + `TaskAdvanced{Ready}`——
    /// 失败与自动重排队是两个事实，事件流与投影在每步一致（旧实现把重试
    /// 折叠在 Failed 事件内部，投影直接跳到 Ready，事件与投影矛盾）。
    pub fn advance_task(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        task_id: TaskId,
        to: TaskState,
    ) -> Result<Vec<TeamEvent>, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, by)?;
        task_board::validate_advance(&rt.aggregate, &task_id, by, to)?;
        self.commit_advance(team_id, rt, &task_id, to)
    }

    /// 显式 Supervisor override 的任务状态推进：仅 supervisor 可调用，
    /// 跳过 owner 校验（不扩大普通成员的权限面）。
    pub fn supervisor_advance_task(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        task_id: TaskId,
        to: TaskState,
    ) -> Result<Vec<TeamEvent>, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_supervisor(rt, by)?;
        task_board::validate_supervisor_advance(&rt.aggregate, &task_id, to)?;
        self.commit_advance(team_id, rt, &task_id, to)
    }

    /// 推进共享路径：构造（可能成对的）canonical 事件并原子落盘。
    fn commit_advance(
        &self,
        team_id: &TeamId,
        rt: &mut TeamRuntime,
        task_id: &TaskId,
        to: TaskState,
    ) -> Result<Vec<TeamEvent>, TeamError> {
        let mut events = vec![TeamEvent::TaskAdvanced {
            team_id: team_id.clone(),
            task_id: task_id.clone(),
            state: to,
        }];
        if to == TaskState::Failed {
            let task = rt
                .aggregate
                .board
                .get(task_id)
                .expect("transition validated");
            if task.retry_count < task.max_retries {
                // 自动重排队：第二条 Ready 事件与 Failed 同批原子落盘。
                events.push(TeamEvent::TaskAdvanced {
                    team_id: team_id.clone(),
                    task_id: task_id.clone(),
                    state: TaskState::Ready,
                });
            }
        }
        self.commit_batch(team_id, rt, events.clone())?;
        Ok(events)
    }

    // ---------------- mailbox ----------------

    /// 投递一条 mailbox 消息（点对点 / 广播）；持久化、可重放。
    pub fn post_message(
        &self,
        team_id: &TeamId,
        sender: &AgentId,
        recipients: Recipients,
        body: String,
    ) -> Result<TeamEvent, TeamError> {
        if body.trim().is_empty() {
            return Err(TeamError::EmptyText);
        }
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, sender)?;
        mailbox::resolve_recipients(&rt.aggregate, sender, &recipients)?;
        let message_id = self.next_message_id(rt)?;
        let event = TeamEvent::MailboxPosted {
            team_id: team_id.clone(),
            message_id,
            sender: sender.clone(),
            recipients,
            body,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 拉取指定成员尚未投递的消息；返回事件序列（每条对应一次 MailboxDelivered）。
    ///
    /// 全部投递事件经 [`TeamEventStore::append_batch`] **原子落盘**（persist-first）：
    /// 任一条失败则整批不落盘、状态不变、可安全重试；不会出现「部分消息已
    /// 投递、部分丢失」的中间状态。
    pub fn pull_mailbox(
        &self,
        team_id: &TeamId,
        agent: &AgentId,
    ) -> Result<Vec<TeamEvent>, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, agent)?;
        let ids = mailbox::pull(&rt.aggregate, agent);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut events = Vec::with_capacity(ids.len());
        for message_id in ids {
            let event = TeamEvent::MailboxDelivered {
                team_id: team_id.clone(),
                message_id,
                recipient: agent.clone(),
            };
            events.push(event);
        }
        self.commit_batch(team_id, rt, events.clone())?;
        Ok(events)
    }

    /// 标记消息已读。
    pub fn mark_read(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        message_id: MailboxMessageId,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, by)?;
        mailbox::validate_read(&rt.aggregate, &message_id, by)?;
        let event = TeamEvent::MailboxRead {
            team_id: team_id.clone(),
            message_id,
            by: by.clone(),
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    // ---------------- presence ----------------

    /// 观察成员的 P12 worker 生命周期状态，派生 presence 并在变化时发事件。
    ///
    /// 这是 presence 的唯一入口：supervisor 是 worker 状态的事实源，team 只
    /// 把它翻译为协作层 presence（P3-1 run 状态对齐）。
    pub fn observe_worker_state(
        &self,
        team_id: &TeamId,
        agent_id: &AgentId,
        worker_state: WorkerState,
    ) -> Result<Option<TeamEvent>, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        if !rt.aggregate.members.contains_key(agent_id) {
            return Err(TeamError::NotMember {
                team_id: team_id.clone(),
                agent_id: (*agent_id).clone(),
            });
        }
        let new_presence = presence::derive_from_worker_state(worker_state);
        if rt.aggregate.presence.get(agent_id) == Some(&new_presence) {
            return Ok(None);
        }
        let event = TeamEvent::PresenceChanged {
            team_id: team_id.clone(),
            agent_id: agent_id.clone(),
            presence: new_presence,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(Some(event))
    }

    /// 从既有 P12 worker 生命周期事件流（[`OrchestrationEvent`]）派生 presence。
    ///
    /// 复用 `crate::replay_workers`——P12 既有的 worker 状态源
    /// （`AgentSupervisor` 恢复 / 查询共用同一折叠）——把事件流折叠成每个
    /// agent 的 [`WorkerState`] 后再翻译为 presence。teams **不复制 worker
    /// 状态机、不复制 run loop**，只做协作层翻译；app-service 生产桥把
    /// orchestration 事件流喂给本方法（见 `app_service::team::TeamHost`）。
    ///
    /// 变化事件单批原子落盘（persist-first）；非成员事件忽略（桥可接收
    /// 全局 worker 事件流，不因其他 worker 的状态变化而失败）。
    pub fn observe_worker_events(
        &self,
        team_id: &TeamId,
        events: &[OrchestrationEvent],
    ) -> Result<Vec<TeamEvent>, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        let states = crate::replay_workers(events);
        let mut pending: Vec<TeamEvent> = Vec::new();
        for (agent_id, worker_state) in &states {
            if !rt.aggregate.members.contains_key(agent_id) {
                continue;
            }
            let new_presence = presence::derive_from_worker_state(*worker_state);
            if rt.aggregate.presence.get(agent_id) != Some(&new_presence) {
                pending.push(TeamEvent::PresenceChanged {
                    team_id: team_id.clone(),
                    agent_id: agent_id.clone(),
                    presence: new_presence,
                });
            }
        }
        if pending.is_empty() {
            return Ok(Vec::new());
        }
        self.commit_batch(team_id, rt, pending.clone())?;
        Ok(pending)
    }

    // ---------------- 受控 peer messaging ----------------

    /// 发起一次受控 peer 消息：经 [`PeerPolicy`] 鉴权后路由（落 mailbox），
    /// 否则产出 [`TeamEvent::FanOutDenied`] 审计事实。
    pub fn route_peer_message(
        &self,
        team_id: &TeamId,
        sender: &AgentId,
        recipients: Recipients,
        body: String,
    ) -> Result<TeamEvent, TeamError> {
        if body.trim().is_empty() {
            return Err(TeamError::EmptyText);
        }
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, sender)?;
        let members = rt.aggregate.member_set();
        let active = rt
            .aggregate
            .active_fan_out
            .get(sender)
            .copied()
            .unwrap_or(0) as usize;
        let policy = self.peer_policy.clone();
        match policy.authorize(sender, &recipients, &members, active) {
            Ok(targets) => {
                let message_id = self.next_message_id(rt)?;
                let fan_out_id = self.next_fan_out_id(rt)?;
                // 路由事件携带策略展开后的精确目标集（Direct）：fan-out
                // 完成判定、重放与镜像都以该冻结集合为准（广播在路由时
                // 已展开，不受后续成员变更影响）。
                let recipients = Recipients::Direct {
                    members: targets.into_iter().collect(),
                };
                let event = TeamEvent::PeerMessageRouted {
                    team_id: team_id.clone(),
                    message_id,
                    fan_out_id,
                    sender: sender.clone(),
                    recipients,
                    body,
                };
                self.commit(team_id, rt, event.clone())?;
                Ok(event)
            }
            Err(err) => {
                let reason = err.to_string();
                let event = TeamEvent::FanOutDenied {
                    team_id: team_id.clone(),
                    sender: sender.clone(),
                    recipients,
                    reason,
                };
                // FanOutDenied 也持久化（审计可重放），但不计入 active_fan_out。
                self.commit(team_id, rt, event)?;
                Err(err)
            }
        }
    }

    // ---------------- plan 审批 ----------------

    /// worker 向 team 提交 Plan（进入评审；未审批前执行 gate 返回 false）。
    pub fn submit_plan(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        plan_id: PlanId,
        version: PlanVersionId,
        title: String,
        steps: Vec<PlanStepSnapshot>,
    ) -> Result<TeamEvent, TeamError> {
        if title.trim().is_empty() || steps.is_empty() {
            return Err(TeamError::EmptyText);
        }
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, by)?;
        // 构造一次以校验 + 折叠到 per-plan PlanState（复用 P16-2）。
        let _ = approval::build_submitted_state(
            plan_id.clone(),
            version.clone(),
            title.clone(),
            steps.clone(),
        );
        let event = TeamEvent::PlanSubmitted {
            team_id: team_id.clone(),
            plan_id,
            version,
            title,
            steps,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 审批通过（仅 supervisor）。
    pub fn approve_plan(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        plan_id: PlanId,
        version: PlanVersionId,
        checkpoint_id: Option<CheckpointId>,
    ) -> Result<TeamEvent, TeamError> {
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_supervisor(rt, by)?;
        let entry = rt
            .aggregate
            .plans
            .get(&plan_id)
            .ok_or_else(|| TeamError::PlanNotSubmitted(plan_id.clone()))?;
        approval::validate_approve(&entry.state, &plan_id, &version)?;
        let event = TeamEvent::PlanApproved {
            team_id: team_id.clone(),
            plan_id,
            version,
            checkpoint_id,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 审批拒绝（仅 supervisor）。
    pub fn reject_plan(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        plan_id: PlanId,
        version: PlanVersionId,
        reason: String,
    ) -> Result<TeamEvent, TeamError> {
        if reason.trim().is_empty() {
            return Err(TeamError::EmptyText);
        }
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_supervisor(rt, by)?;
        let entry = rt
            .aggregate
            .plans
            .get(&plan_id)
            .ok_or_else(|| TeamError::PlanNotSubmitted(plan_id.clone()))?;
        approval::validate_reject(&entry.state, &plan_id, &version)?;
        let event = TeamEvent::PlanRejected {
            team_id: team_id.clone(),
            plan_id,
            version,
            reason,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 追加行锚点评审意见（任意成员）。
    pub fn comment_plan(
        &self,
        team_id: &TeamId,
        by: &AgentId,
        plan_id: PlanId,
        version: PlanVersionId,
        anchor: PlanCommentAnchor,
        body: String,
    ) -> Result<TeamEvent, TeamError> {
        if body.trim().is_empty() {
            return Err(TeamError::EmptyText);
        }
        let mut inner = self.inner.lock();
        let rt = self.mutable_team(&mut inner, team_id)?;
        self.require_member(rt, by)?;
        let entry = rt
            .aggregate
            .plans
            .get(&plan_id)
            .ok_or_else(|| TeamError::PlanNotSubmitted(plan_id.clone()))?;
        approval::validate_comment(&entry.state, &plan_id, &version, &anchor)?;
        let event = TeamEvent::PlanCommented {
            team_id: team_id.clone(),
            plan_id,
            version,
            anchor,
            body,
        };
        self.commit(team_id, rt, event.clone())?;
        Ok(event)
    }

    /// 执行 gate：plan/version 已审批通过才放行（未批准 plan 不执行）。
    pub fn is_approved_for_execution(
        &self,
        team_id: &TeamId,
        plan_id: &PlanId,
        version: &PlanVersionId,
    ) -> bool {
        let inner = self.inner.lock();
        let Some(rt) = inner.teams.get(team_id) else {
            return false;
        };
        rt.aggregate
            .plans
            .get(plan_id)
            .is_some_and(|e| approval::is_approved_for_execution(&e.state, plan_id, version))
    }

    // ---------------- 查询面（只读快照） ----------------

    /// team 当前投影快照（用于查询 / GUI）。
    pub fn snapshot(&self, team_id: &TeamId) -> Option<TeamAggregate> {
        let inner = self.inner.lock();
        inner.teams.get(team_id).map(|rt| rt.aggregate.clone())
    }

    /// 成员 presence 快照。
    pub fn presence(&self, team_id: &TeamId) -> Option<BTreeMap<AgentId, Presence>> {
        self.snapshot(team_id).map(|a| a.presence)
    }

    /// 指定成员的未读 mailbox 消息数。
    pub fn unread_count(&self, team_id: &TeamId, agent: &AgentId) -> usize {
        let Some(agg) = self.snapshot(team_id) else {
            return 0;
        };
        agg.mailbox
            .values()
            .filter(|e| agg.is_recipient(e, agent) && !e.read_by.contains(agent))
            .count()
    }

    // ---------------- 内部 ----------------

    /// 取可变 team runtime；校验存在、未解散。
    fn mutable_team<'a>(
        &self,
        inner: &'a mut Inner,
        team_id: &TeamId,
    ) -> Result<&'a mut TeamRuntime, TeamError> {
        let rt = inner
            .teams
            .get_mut(team_id)
            .ok_or_else(|| TeamError::TeamNotFound(team_id.clone()))?;
        if rt.aggregate.dissolved {
            return Err(TeamError::TeamDissolved(team_id.clone()));
        }
        Ok(rt)
    }

    fn require_member(&self, rt: &TeamRuntime, agent: &AgentId) -> Result<(), TeamError> {
        if rt.aggregate.members.contains_key(agent) {
            Ok(())
        } else {
            Err(TeamError::NotMember {
                team_id: rt.aggregate.team_id.clone().unwrap_or_default(),
                agent_id: (*agent).clone(),
            })
        }
    }

    fn require_supervisor(&self, rt: &TeamRuntime, agent: &AgentId) -> Result<(), TeamError> {
        match rt.aggregate.members.get(agent) {
            Some(MemberRole::Supervisor) => Ok(()),
            Some(_) => Err(TeamError::NotSupervisor {
                team_id: rt.aggregate.team_id.clone().unwrap_or_default(),
                agent_id: (*agent).clone(),
            }),
            None => Err(TeamError::NotMember {
                team_id: rt.aggregate.team_id.clone().unwrap_or_default(),
                agent_id: (*agent).clone(),
            }),
        }
    }

    fn next_message_id(&self, rt: &mut TeamRuntime) -> Result<MailboxMessageId, TeamError> {
        let n = rt
            .next_local_id
            .checked_add(1)
            .ok_or(TeamError::IdSpaceExhausted("mailbox message id"))?;
        Ok(MailboxMessageId::new(format!("msg-{n}")))
    }

    fn next_fan_out_id(&self, rt: &mut TeamRuntime) -> Result<FanOutId, TeamError> {
        let n = rt
            .next_local_id
            .checked_add(1)
            .ok_or(TeamError::IdSpaceExhausted("fan-out id"))?;
        Ok(FanOutId::new(format!("fanout-{n}")))
    }

    /// 单事件提交：委托批量核心（见 [`Self::commit_batch`]）。
    fn commit(
        &self,
        team_id: &TeamId,
        rt: &mut TeamRuntime,
        payload: TeamEvent,
    ) -> Result<(), TeamError> {
        self.commit_batch(team_id, rt, vec![payload]).map(|_| ())
    }

    /// 批量核心：构造信封 → **durable append_batch（可失败，原子）** →
    /// 成功后逐条 apply 折叠 + 推进序列 → 经 sink 逐条镜像投递。
    ///
    /// persist-first 语义：`append_batch` 失败时序列 / 本地计数 / 聚合 / sink
    /// 全部保持不变，调用方可安全重试（重试会复用同一组 sequence / event_id，
    /// store 幂等去重保证整批要么全部落盘要么全部不落盘）。
    fn commit_batch(
        &self,
        team_id: &TeamId,
        rt: &mut TeamRuntime,
        events: Vec<TeamEvent>,
    ) -> Result<Vec<TeamEvent>, TeamError> {
        if events.is_empty() {
            return Ok(events);
        }
        let count = events.len() as u64;
        // checked 预检：在 append 之前确认序列 / 本地计数可推进，溢出时
        // 直接报错且**不落盘任何事实**（避免已追加但状态未推进的不一致）。
        let next_sequence = rt
            .next_sequence
            .checked_add(count)
            .ok_or(TeamError::IdSpaceExhausted("event sequence"))?;
        let next_local_id = rt
            .next_local_id
            .checked_add(count)
            .ok_or(TeamError::IdSpaceExhausted("local id"))?;
        let mut envelopes = Vec::with_capacity(events.len());
        for (sequence, payload) in (rt.next_sequence..).zip(events) {
            let seq = TeamEventSequence::new(sequence);
            let event_id = EventId::new(format!(
                "{team}-evt-{seq}",
                team = team_id.as_str(),
                seq = sequence
            ));
            let timestamp = Timestamp::from_unix_millis(now_millis());
            envelopes.push(TeamEventEnvelope::new(
                team_id.clone(),
                seq,
                event_id,
                timestamp,
                payload,
            ));
        }
        // 持久化成功才变更状态：append → 序列推进 + 聚合折叠 → 镜像 EventHub。
        self.store.append_batch(&envelopes)?;
        rt.next_sequence = next_sequence;
        rt.next_local_id = next_local_id;
        for envelope in &envelopes {
            crate::teams::state::apply(&mut rt.aggregate, envelope.payload.clone());
            self.sink.record(envelope.clone());
        }
        Ok(envelopes
            .into_iter()
            .map(|envelope| envelope.payload)
            .collect())
    }
}

/// 从已重放事件恢复本地计数源：message 与 fan-out ID 共享同一计数器
/// （本 crate 生成形态 `msg-N` / `fanout-N`），取已用后缀的最大值。
/// 重启后 `next_local_id` 从该值继续，杜绝重放归零导致的 ID 复用。
/// 非本 crate 形态（无法解析）的后缀忽略，不影响单调性。
fn max_local_id(envelopes: &[TeamEventEnvelope]) -> u64 {
    fn numeric_suffix(id: &str, prefix: &str) -> Option<u64> {
        id.strip_prefix(prefix).and_then(|rest| rest.parse().ok())
    }
    let mut max = 0u64;
    for envelope in envelopes {
        match &envelope.payload {
            TeamEvent::MailboxPosted { message_id, .. } => {
                if let Some(n) = numeric_suffix(message_id.as_str(), "msg-") {
                    max = max.max(n);
                }
            }
            TeamEvent::PeerMessageRouted {
                message_id,
                fan_out_id,
                ..
            } => {
                if let Some(n) = numeric_suffix(message_id.as_str(), "msg-") {
                    max = max.max(n);
                }
                if let Some(n) = numeric_suffix(fan_out_id.as_str(), "fanout-") {
                    max = max.max(n);
                }
            }
            _ => {}
        }
    }
    max
}

fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
