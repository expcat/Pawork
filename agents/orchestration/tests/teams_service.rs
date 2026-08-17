// 集成测试：TeamService 端到端覆盖 P17-6 验收点。
// - team 生命周期（创建 / 成员 / 解散）全程 canonical event 可重放
// - shared task board 认领与依赖流转
// - mailbox 异步投递与持久化（经事件 sink 镜像）
// - presence 基于 worker 状态派生
// - 受控 peer messaging fan-out 策略（拒绝 / 放行）
// - plan approval 未批准阻断执行 gate
// - 注入 sink 是唯一外发出口（不另建 broadcast）

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use pawork_domain::{
    AgentId, PlanId, PlanStepId, PlanStepSnapshot, PlanStepStatus, PlanVersionId, TenantId,
};
use pawork_orchestration::{OrchestrationEvent, TaskId, TaskState, WorkerRole, WorkerState};
use pawork_orchestration::{
    MemberRole, PeerPolicy, Presence, Recipients, RecordingTeamSink, TeamError, TeamEvent,
    TeamEventEnvelope, TeamEventStore, TeamId, TeamService, TeamStoreError,
};

fn plan_steps() -> Vec<PlanStepSnapshot> {
    vec![PlanStepSnapshot {
        step_id: PlanStepId::from("s1"),
        text: "step one".into(),
        status: PlanStepStatus::Pending,
    }]
}

struct Env {
    svc: TeamService,
    sink: Arc<RecordingTeamSink>,
    team: TeamId,
    sup: AgentId,
    w1: AgentId,
    w2: AgentId,
}

fn env() -> Env {
    let sink = Arc::new(RecordingTeamSink::new());
    let svc = TeamService::with_sink_and_policy(sink.clone(), PeerPolicy::default());
    let team = TeamId::from("team-1");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    let w2 = AgentId::from("w2");
    svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    svc.add_member(&team, &sup, &w1, MemberRole::Worker)
        .unwrap();
    svc.add_member(&team, &sup, &w2, MemberRole::Worker)
        .unwrap();
    Env {
        svc,
        sink,
        team,
        sup,
        w1,
        w2,
    }
}

#[test]
fn lifecycle_events_are_canonical_and_replayable() {
    let e = env();
    let envelopes = e.sink.events();
    // TeamCreated + 2x MemberAdded = 3 events；全部 JSON 可往返。
    assert_eq!(envelopes.len(), 3);
    for env in &envelopes {
        let json = serde_json::to_string(env).unwrap();
        let back: TeamEventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, env);
    }
    // 序列在 team 内单调连续。
    let seqs: Vec<u64> = envelopes.iter().map(|e| e.sequence.value()).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
}

#[test]
fn task_claim_requires_dependencies_then_flows_to_completion() {
    let e = env();
    let team = e.team.clone();
    // 张贴 dep 与 child（child 依赖 dep）。
    e.svc
        .post_task(&team, &e.sup, TaskId::new("dep"), "dep".into(), vec![], 0)
        .unwrap();
    e.svc
        .post_task(
            &team,
            &e.sup,
            TaskId::new("child"),
            "child".into(),
            vec![TaskId::new("dep")],
            0,
        )
        .unwrap();
    // 依赖未完成 → 认领 child 被拒。
    let err = e
        .svc
        .claim_task(&team, &e.w1, TaskId::new("child"))
        .unwrap_err();
    assert!(matches!(err, TeamError::UnmetDependencies { .. }));
    // 完成 dep → child 可认领并流转到 Completed。
    e.svc.claim_task(&team, &e.w1, TaskId::new("dep")).unwrap();
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("dep"), TaskState::Running)
        .unwrap();
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("dep"), TaskState::Completed)
        .unwrap();
    e.svc
        .claim_task(&team, &e.w2, TaskId::new("child"))
        .unwrap();
    e.svc
        .advance_task(&team, &e.w2, TaskId::new("child"), TaskState::Running)
        .unwrap();
    e.svc
        .advance_task(&team, &e.w2, TaskId::new("child"), TaskState::Completed)
        .unwrap();
    let snap = e.svc.snapshot(&team).unwrap();
    assert_eq!(
        snap.board.get(&TaskId::new("child")).unwrap().state,
        TaskState::Completed
    );
}

#[test]
fn task_already_claimed_cannot_be_reclaimed() {
    let e = env();
    let team = e.team.clone();
    e.svc
        .post_task(&team, &e.sup, TaskId::new("t"), "t".into(), vec![], 0)
        .unwrap();
    e.svc.claim_task(&team, &e.w1, TaskId::new("t")).unwrap();
    let err = e
        .svc
        .claim_task(&team, &e.w2, TaskId::new("t"))
        .unwrap_err();
    assert!(matches!(err, TeamError::TaskAlreadyClaimed { .. }));
}

#[test]
fn mailbox_async_delivery_persists_via_sink() {
    let e = env();
    let team = e.team.clone();
    let before = e.sink.events().len();
    e.svc
        .post_message(
            &team,
            &e.sup,
            Recipients::Direct {
                members: vec![e.w1.clone()],
            },
            "hello".into(),
        )
        .unwrap();
    // 投递即镜像到 sink（持久化）。
    assert_eq!(e.sink.events().len(), before + 1);
    // w1 拉取 → 产生 MailboxDelivered 事件；再次拉取为空。
    let delivered = e.svc.pull_mailbox(&team, &e.w1).unwrap();
    assert_eq!(delivered.len(), 1);
    assert!(e.svc.pull_mailbox(&team, &e.w1).unwrap().is_empty());
    // w2 不是收件人，拉取为空。
    assert!(e.svc.pull_mailbox(&team, &e.w2).unwrap().is_empty());
    // 未读计数：w1 在 mark_read 前 = 1。
    assert_eq!(e.svc.unread_count(&team, &e.w1), 1);
}

#[test]
fn presence_derives_from_worker_state_and_dedups() {
    let e = env();
    let team = e.team.clone();
    let ev1 = e
        .svc
        .observe_worker_state(&team, &e.w1, WorkerState::Running)
        .unwrap()
        .unwrap();
    assert!(matches!(
        ev1,
        TeamEvent::PresenceChanged {
            presence: Presence::Busy,
            ..
        }
    ));
    let ev2 = e
        .svc
        .observe_worker_state(&team, &e.w1, WorkerState::Waiting)
        .unwrap()
        .unwrap();
    assert!(matches!(
        ev2,
        TeamEvent::PresenceChanged {
            presence: Presence::Idle,
            ..
        }
    ));
    // 同状态不重复发事件。
    assert!(e
        .svc
        .observe_worker_state(&team, &e.w1, WorkerState::Waiting)
        .unwrap()
        .is_none());
    let snap = e.svc.presence(&team).unwrap();
    assert_eq!(snap.get(&e.w1), Some(&Presence::Idle));
}

#[test]
fn peer_fan_out_is_policy_gated() {
    let e = env();
    let team = e.team.clone();
    // 默认策略禁止广播 → 拒绝（且产出 FanOutDenied 审计事件）。
    let before = e.sink.events().len();
    let err = e
        .svc
        .route_peer_message(&team, &e.sup, Recipients::Broadcast, "boom".into())
        .unwrap_err();
    assert!(matches!(err, TeamError::FanOutDenied { .. }));
    assert!(e.sink.events().len() > before);
    assert!(matches!(
        e.sink.events().last().unwrap().payload,
        TeamEvent::FanOutDenied { .. }
    ));
    // 直连在限内 → 路由成功（PeerMessageRouted）。
    let routed = e
        .svc
        .route_peer_message(
            &team,
            &e.w1,
            Recipients::Direct {
                members: vec![e.w2.clone()],
            },
            "hi".into(),
        )
        .unwrap();
    assert!(matches!(routed, TeamEvent::PeerMessageRouted { .. }));
}

#[test]
fn unapproved_plan_blocks_execution_until_supervisor_approves() {
    let e = env();
    let team = e.team.clone();
    let plan = PlanId::from("p1");
    let ver = PlanVersionId::from("v1");
    e.svc
        .submit_plan(
            &team,
            &e.w1,
            plan.clone(),
            ver.clone(),
            "Plan".into(),
            plan_steps(),
        )
        .unwrap();
    // 未审批 → gate 阻断。
    assert!(!e.svc.is_approved_for_execution(&team, &plan, &ver));
    // worker 不能审批（NotSupervisor）。
    let err = e
        .svc
        .approve_plan(&team, &e.w1, plan.clone(), ver.clone(), None)
        .unwrap_err();
    assert!(matches!(err, TeamError::NotSupervisor { .. }));
    // supervisor 审批 → gate 放行。
    e.svc
        .approve_plan(&team, &e.sup, plan.clone(), ver.clone(), None)
        .unwrap();
    assert!(e.svc.is_approved_for_execution(&team, &plan, &ver));
}

#[test]
fn rejected_plan_stays_blocked() {
    let e = env();
    let team = e.team.clone();
    let plan = PlanId::from("p1");
    let ver = PlanVersionId::from("v1");
    e.svc
        .submit_plan(
            &team,
            &e.w1,
            plan.clone(),
            ver.clone(),
            "Plan".into(),
            plan_steps(),
        )
        .unwrap();
    e.svc
        .reject_plan(&team, &e.sup, plan.clone(), ver.clone(), "nope".into())
        .unwrap();
    assert!(!e.svc.is_approved_for_execution(&team, &plan, &ver));
}

#[test]
fn replay_restores_full_team_state() {
    let e = env();
    let team = e.team.clone();
    e.svc
        .post_task(&team, &e.sup, TaskId::new("t"), "t".into(), vec![], 0)
        .unwrap();
    e.svc.claim_task(&team, &e.w1, TaskId::new("t")).unwrap();
    e.svc
        .observe_worker_state(&team, &e.w1, WorkerState::Running)
        .unwrap();
    let envelopes = e.sink.events();

    // 用录制的事件重放重建（新 sink），状态应与原服务一致。
    let rebuilt = TeamService::from_envelopes(
        envelopes,
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    );
    let snap = rebuilt.snapshot(&team).expect("rebuilt team exists");
    assert_eq!(snap.members.len(), 3);
    assert_eq!(
        snap.board.get(&TaskId::new("t")).unwrap().owner,
        Some(e.w1.clone())
    );
    assert_eq!(snap.presence.get(&e.w1), Some(&Presence::Busy));
    assert!(!snap.dissolved);
}

#[test]
fn dissolved_team_rejects_all_writes() {
    let e = env();
    let team = e.team.clone();
    e.svc.dissolve_team(&team, &e.sup).unwrap();
    let err = e
        .svc
        .post_message(
            &team,
            &e.w1,
            Recipients::Direct {
                members: vec![e.w2.clone()],
            },
            "x".into(),
        )
        .unwrap_err();
    assert!(matches!(err, TeamError::TeamDissolved { .. }));
    let err = e
        .svc
        .claim_task(&team, &e.w1, TaskId::new("any"))
        .unwrap_err();
    assert!(matches!(err, TeamError::TeamDissolved { .. }));
}

/// 可注入失败的 store：前 `failures` 次 append 失败，之后放行（测试
/// persist-first：失败时状态不变、可安全重试）。
struct FailingStore {
    inner: pawork_orchestration::MemoryTeamStore,
    failures_left: AtomicUsize,
}

impl FailingStore {
    fn new(failures: usize) -> Self {
        Self {
            inner: pawork_orchestration::MemoryTeamStore::new(),
            failures_left: AtomicUsize::new(failures),
        }
    }
}

impl TeamEventStore for FailingStore {
    fn append(&self, envelope: &TeamEventEnvelope) -> Result<(), TeamStoreError> {
        if self
            .failures_left
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(TeamStoreError::Store("injected failure".into()));
        }
        self.inner.append(envelope)
    }

    fn replay(&self) -> Result<Vec<TeamEventEnvelope>, TeamStoreError> {
        self.inner.replay()
    }
}

/// 只让 `append_batch` 失败一次的 store（单条 append 恒成功）：定向验证
/// 批量命令的原子 persist-first（失败不留部分投递）。
struct BatchFailingStore {
    inner: pawork_orchestration::MemoryTeamStore,
    batch_failures_left: AtomicUsize,
}

impl BatchFailingStore {
    fn new(failures: usize) -> Self {
        Self {
            inner: pawork_orchestration::MemoryTeamStore::new(),
            batch_failures_left: AtomicUsize::new(failures),
        }
    }
}

impl TeamEventStore for BatchFailingStore {
    fn append(&self, envelope: &TeamEventEnvelope) -> Result<(), TeamStoreError> {
        self.inner.append(envelope)
    }

    fn append_batch(&self, envelopes: &[TeamEventEnvelope]) -> Result<(), TeamStoreError> {
        // 只拦截真正的多事件批量（单事件提交也走 append_batch）。
        if envelopes.len() > 1
            && self
                .batch_failures_left
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            return Err(TeamStoreError::Store("injected batch failure".into()));
        }
        self.inner.append_batch(envelopes)
    }

    fn replay(&self) -> Result<Vec<TeamEventEnvelope>, TeamStoreError> {
        self.inner.replay()
    }
}

#[test]
fn persist_failure_leaves_state_unchanged_and_retry_succeeds() {
    let sink = Arc::new(RecordingTeamSink::new());
    let store: Arc<dyn TeamEventStore> = Arc::new(FailingStore::new(1));
    let svc = TeamService::with_store_sink_and_policy(store, sink.clone(), PeerPolicy::default());
    let team = TeamId::from("team-f");
    let sup = AgentId::from("sup");

    // 首次 create：append 失败 → 命令报错，状态 / 序列 / sink 全部不变。
    let err = svc
        .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap_err();
    assert!(matches!(err, TeamError::Store(_)));
    assert!(svc.snapshot(&team).is_none());
    assert!(sink.events().is_empty());

    // 重试：同一序列 / event_id 复用，落盘成功后状态与 sink 一致。
    let event = svc
        .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    assert!(matches!(event, TeamEvent::TeamCreated { .. }));
    let snap = svc.snapshot(&team).expect("team exists after retry");
    assert_eq!(snap.members.len(), 1);
    let recorded = sink.events();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].sequence.value(), 1);
    assert_eq!(recorded[0].event_id.as_str(), "team-f-evt-1");
}

#[test]
fn from_store_restart_replay_restores_state_and_continues() {
    let store: Arc<dyn TeamEventStore> = Arc::new(pawork_orchestration::MemoryTeamStore::new());
    let team = TeamId::from("team-r");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    {
        let svc = TeamService::with_store_sink_and_policy(
            store.clone(),
            Arc::new(RecordingTeamSink::new()),
            PeerPolicy::default(),
        );
        svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
            .unwrap();
        svc.add_member(&team, &sup, &w1, MemberRole::Worker)
            .unwrap();
        svc.post_task(&team, &sup, TaskId::new("t"), "t".into(), vec![], 0)
            .unwrap();
        svc.claim_task(&team, &w1, TaskId::new("t")).unwrap();
        // 服务随「进程」丢弃；store 是唯一事实源。
    }

    // 重启：从同一 store 重放重建，状态完整、可继续追加且序列连续。
    let rebuilt = TeamService::from_store(
        store.clone(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    )
    .expect("restart replay");
    let snap = rebuilt.snapshot(&team).expect("rebuilt team");
    assert_eq!(snap.members.len(), 2);
    assert_eq!(
        snap.board.get(&TaskId::new("t")).unwrap().owner,
        Some(w1.clone())
    );

    rebuilt
        .post_message(
            &team,
            &w1,
            Recipients::Direct {
                members: vec![sup.clone()],
            },
            "after restart".into(),
        )
        .unwrap();
    let replayed = store.replay().unwrap();
    let team_events: Vec<_> = replayed.iter().filter(|e| e.team_id == team).collect();
    let sequences: Vec<u64> = team_events.iter().map(|e| e.sequence.value()).collect();
    assert_eq!(sequences, vec![1, 2, 3, 4, 5]);
    assert!(matches!(
        team_events.last().unwrap().payload,
        TeamEvent::MailboxPosted { .. }
    ));
}

#[test]
fn active_fan_out_decrements_only_after_all_targets_delivered() {
    let e = env();
    let team = e.team.clone();
    e.svc
        .route_peer_message(
            &team,
            &e.w1,
            Recipients::Direct {
                members: vec![e.sup.clone(), e.w2.clone()],
            },
            "fan".into(),
        )
        .unwrap();
    let snap = e.svc.snapshot(&team).unwrap();
    assert_eq!(
        snap.active_fan_out.get(&e.w1),
        Some(&1),
        "路由后发送者持有一个活跃 fan-out"
    );
    // 部分投递：计数保持。
    e.svc.pull_mailbox(&team, &e.sup).unwrap();
    assert_eq!(
        e.svc.snapshot(&team).unwrap().active_fan_out.get(&e.w1),
        Some(&1),
        "部分目标已投递时 fan-out 未完成"
    );
    // 全部目标投递：计数归零（从 map 移除）。
    e.svc.pull_mailbox(&team, &e.w2).unwrap();
    assert_eq!(
        e.svc.snapshot(&team).unwrap().active_fan_out.get(&e.w1),
        None,
        "全部目标 delivered 后 active_fan_out 递减"
    );
    // durable / replay deterministic：重放同一事件流得到同一投影。
    let rebuilt = TeamService::from_envelopes(
        e.sink.events(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    );
    assert_eq!(
        rebuilt.snapshot(&team).unwrap().active_fan_out.get(&e.w1),
        None,
        "重放必须得到相同的 fan-out 完成判定"
    );
}

#[test]
fn mark_read_completes_fan_out_without_pull() {
    let e = env();
    let team = e.team.clone();
    let routed = e
        .svc
        .route_peer_message(
            &team,
            &e.w1,
            Recipients::Direct {
                members: vec![e.w2.clone()],
            },
            "fan".into(),
        )
        .unwrap();
    let TeamEvent::PeerMessageRouted { message_id, .. } = routed else {
        panic!("expected peer_message_routed");
    };
    assert_eq!(
        e.svc.snapshot(&team).unwrap().active_fan_out.get(&e.w1),
        Some(&1)
    );
    // 已读即视为投递（MailboxRead 也写入 delivered_to）：最后一个目标读完
    // fan-out 完成。
    e.svc.mark_read(&team, &e.w2, message_id).unwrap();
    assert_eq!(
        e.svc.snapshot(&team).unwrap().active_fan_out.get(&e.w1),
        None,
        "read 路径同样递减 fan-out"
    );
}

#[test]
fn fan_out_decrements_exactly_once_despite_repeat_read() {
    let sink = Arc::new(RecordingTeamSink::new());
    let svc = TeamService::with_sink_and_policy(sink.clone(), PeerPolicy::permissive());
    let team = TeamId::from("team-exact");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    let w2 = AgentId::from("w2");
    svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    svc.add_member(&team, &sup, &w1, MemberRole::Worker)
        .unwrap();
    svc.add_member(&team, &sup, &w2, MemberRole::Worker)
        .unwrap();
    // 两个并发 fan-out：w1 → {w2} 与 w1 → {sup}。
    let m1 = match svc
        .route_peer_message(
            &team,
            &w1,
            Recipients::Direct {
                members: vec![w2.clone()],
            },
            "one".into(),
        )
        .unwrap()
    {
        TeamEvent::PeerMessageRouted { message_id, .. } => message_id,
        other => panic!("unexpected event: {other:?}"),
    };
    svc.route_peer_message(
        &team,
        &w1,
        Recipients::Direct {
            members: vec![sup.clone()],
        },
        "two".into(),
    )
    .unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        Some(&2)
    );
    // 第一个 fan-out 全部投递：恰好递减一次（2 → 1）。
    svc.pull_mailbox(&team, &w2).unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        Some(&1)
    );
    // 消息完成后的后续 read / 重复 mark_read 不得再次递减（1 保持 1，
    // 否则第二个仍在途的 fan-out 被误伤）。
    svc.mark_read(&team, &w2, m1.clone()).unwrap();
    svc.mark_read(&team, &w2, m1).unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        Some(&1),
        "已结算 fan-out 的后续 read 不重复递减"
    );
    // 第二个 fan-out 投递后归零移除。
    svc.pull_mailbox(&team, &sup).unwrap();
    assert_eq!(svc.snapshot(&team).unwrap().active_fan_out.get(&w1), None);
    // durable / replay deterministic：同一事件流重建同一投影。
    let rebuilt = TeamService::from_envelopes(
        sink.events(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::permissive(),
    );
    assert_eq!(
        rebuilt.snapshot(&team).unwrap().active_fan_out.get(&w1),
        None,
        "重放必须得到相同的 fan-out 结算"
    );
}

#[test]
fn fan_out_completes_when_target_member_removed() {
    let sink = Arc::new(RecordingTeamSink::new());
    let svc = TeamService::with_sink_and_policy(sink.clone(), PeerPolicy::permissive());
    let team = TeamId::from("team-rm");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    let w2 = AgentId::from("w2");
    let w3 = AgentId::from("w3");
    svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    for m in [&w1, &w2, &w3] {
        svc.add_member(&team, &sup, m, MemberRole::Worker).unwrap();
    }
    // 唯一目标是 w3：w3 移除后消息无人可收，fan-out 在移除点结算。
    svc.route_peer_message(
        &team,
        &w1,
        Recipients::Direct {
            members: vec![w3.clone()],
        },
        "only-w3".into(),
    )
    .unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        Some(&1)
    );
    svc.remove_member(&team, &sup, &w3).unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        None,
        "目标成员全部离开后 fan-out 在移除点完成，计数不泄漏"
    );
    // 部分目标仍在：移除仅收窄目标集，最后一个剩余目标投递后才结算。
    svc.route_peer_message(
        &team,
        &w1,
        Recipients::Direct {
            members: vec![w2.clone(), sup.clone()],
        },
        "mixed".into(),
    )
    .unwrap();
    svc.remove_member(&team, &sup, &w2).unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        Some(&1),
        "剩余目标 {sup} 尚未投递，计数保持"
    );
    svc.pull_mailbox(&team, &sup).unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        None,
        "剩余目标投递后结算"
    );
    // durable / replay deterministic：重放同一事件流得到同一结算点。
    let rebuilt = TeamService::from_envelopes(
        sink.events(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::permissive(),
    );
    assert_eq!(
        rebuilt.snapshot(&team).unwrap().active_fan_out.get(&w1),
        None
    );
}

#[test]
fn routed_broadcast_is_normalized_to_frozen_direct_targets() {
    let sink = Arc::new(RecordingTeamSink::new());
    let svc = TeamService::with_sink_and_policy(sink.clone(), PeerPolicy::permissive());
    let team = TeamId::from("team-bc");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    let w2 = AgentId::from("w2");
    svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    svc.add_member(&team, &sup, &w1, MemberRole::Worker)
        .unwrap();
    svc.add_member(&team, &sup, &w2, MemberRole::Worker)
        .unwrap();
    let routed = svc
        .route_peer_message(&team, &w1, Recipients::Broadcast, "all".into())
        .unwrap();
    let TeamEvent::PeerMessageRouted { recipients, .. } = routed else {
        panic!("expected peer_message_routed");
    };
    let Recipients::Direct { members } = recipients else {
        panic!("广播必须在路由时展开为 Direct 精确目标集");
    };
    assert_eq!(members.len(), 2, "除发送者 w1 外的全部成员");
    assert!(!members.contains(&w1));

    // 路由后加入的新成员不在冻结目标集内：不影响 fan-out 完成判定。
    svc.add_member(&team, &sup, &AgentId::from("w3"), MemberRole::Worker)
        .unwrap();
    svc.pull_mailbox(&team, &sup).unwrap();
    svc.pull_mailbox(&team, &w2).unwrap();
    assert_eq!(
        svc.snapshot(&team).unwrap().active_fan_out.get(&w1),
        None,
        "冻结目标集全部 delivered 即完成，不受后续成员变更影响"
    );
}

#[test]
fn advance_task_requires_strict_owner_and_explicit_supervisor_override() {
    let e = env();
    let team = e.team.clone();
    e.svc
        .post_task(&team, &e.sup, TaskId::new("t"), "t".into(), vec![], 1)
        .unwrap();
    e.svc.claim_task(&team, &e.w1, TaskId::new("t")).unwrap();
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Running)
        .unwrap();
    // 非 owner（w2 / supervisor）不能推进——终态也一样（严格 owner）。
    let err = e
        .svc
        .advance_task(&team, &e.w2, TaskId::new("t"), TaskState::Completed)
        .unwrap_err();
    assert!(
        matches!(err, TeamError::NotTaskOwner { .. }),
        "终态推进也必须由 owner 发起: {err}"
    );
    let err = e
        .svc
        .advance_task(&team, &e.sup, TaskId::new("t"), TaskState::Completed)
        .unwrap_err();
    assert!(matches!(err, TeamError::NotTaskOwner { .. }));
    // worker 不能使用 supervisor override。
    let err = e
        .svc
        .supervisor_advance_task(&team, &e.w2, TaskId::new("t"), TaskState::Completed)
        .unwrap_err();
    assert!(matches!(err, TeamError::NotSupervisor { .. }));
    // 显式 Supervisor override 放行。
    e.svc
        .supervisor_advance_task(&team, &e.sup, TaskId::new("t"), TaskState::Completed)
        .unwrap();
    assert_eq!(
        e.svc
            .snapshot(&team)
            .unwrap()
            .board
            .get(&TaskId::new("t"))
            .unwrap()
            .state,
        TaskState::Completed
    );
}

#[test]
fn failed_auto_retry_emits_consistent_event_pair() {
    let e = env();
    let team = e.team.clone();
    e.svc
        .post_task(&team, &e.sup, TaskId::new("t"), "t".into(), vec![], 2)
        .unwrap();
    e.svc.claim_task(&team, &e.w1, TaskId::new("t")).unwrap();
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Running)
        .unwrap();

    // 第 1 次失败（预算 2）：原子产出 [Failed, Ready] 两条事实。
    let events = e
        .svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Failed)
        .unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        TeamEvent::TaskAdvanced {
            state: TaskState::Failed,
            ..
        }
    ));
    assert!(matches!(
        events[1],
        TeamEvent::TaskAdvanced {
            state: TaskState::Ready,
            ..
        }
    ));
    let task = e
        .svc
        .snapshot(&team)
        .unwrap()
        .board
        .get(&TaskId::new("t"))
        .unwrap()
        .clone();
    assert_eq!(task.state, TaskState::Ready, "投影 = 第二条事件后的状态");
    assert_eq!(task.retry_count, 1);

    // 第 2 次失败：仍在预算内 → 又是事件对。
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Assigned)
        .unwrap();
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Running)
        .unwrap();
    let events = e
        .svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Failed)
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        e.svc.snapshot(&team).unwrap().board[&TaskId::new("t")].retry_count,
        2
    );

    // 第 3 次失败：预算耗尽 → 单条 Failed，投影停在 Failed。
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Assigned)
        .unwrap();
    e.svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Running)
        .unwrap();
    let events = e
        .svc
        .advance_task(&team, &e.w1, TaskId::new("t"), TaskState::Failed)
        .unwrap();
    assert_eq!(events.len(), 1, "预算耗尽后不再自动重排队");
    let task = e
        .svc
        .snapshot(&team)
        .unwrap()
        .board
        .get(&TaskId::new("t"))
        .unwrap()
        .clone();
    assert_eq!(task.state, TaskState::Failed);
    assert_eq!(task.retry_count, 2, "retry_count 不超 max_retries");

    // 事件流与投影一致：重放得到同一状态。
    let rebuilt = TeamService::from_envelopes(
        e.sink.events(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    );
    let replayed = rebuilt.snapshot(&team).unwrap().board[&TaskId::new("t")].clone();
    assert_eq!(replayed.state, TaskState::Failed);
    assert_eq!(replayed.retry_count, 2);
}

#[test]
fn pull_mailbox_batch_is_atomic_on_persist_failure() {
    let sink = Arc::new(RecordingTeamSink::new());
    let store: Arc<dyn TeamEventStore> = Arc::new(BatchFailingStore::new(1));
    let svc = TeamService::with_store_sink_and_policy(store, sink.clone(), PeerPolicy::default());
    let team = TeamId::from("team-batch");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    svc.add_member(&team, &sup, &w1, MemberRole::Worker)
        .unwrap();
    for i in 0..2 {
        svc.post_message(
            &team,
            &w1,
            Recipients::Direct {
                members: vec![sup.clone()],
            },
            format!("m{i}"),
        )
        .unwrap();
    }
    let before = sink.events().len();

    // 批量投递第 1 条 append 失败 → 整批不落盘：无任何 MailboxDelivered。
    let err = svc.pull_mailbox(&team, &sup).unwrap_err();
    assert!(matches!(err, TeamError::Store(_)));
    assert_eq!(sink.events().len(), before, "失败不得镜像任何投递事件");
    let snap = svc.snapshot(&team).unwrap();
    assert!(
        snap.mailbox
            .values()
            .all(|entry| entry.delivered_to.is_empty()),
        "失败时投递状态完全不变"
    );

    // 重试：两条一起原子落盘并镜像。
    let delivered = svc.pull_mailbox(&team, &sup).unwrap();
    assert_eq!(delivered.len(), 2);
    assert_eq!(sink.events().len(), before + 2);
    let snap = svc.snapshot(&team).unwrap();
    assert!(
        snap.mailbox
            .values()
            .all(|entry| entry.delivered_to.contains(&sup)),
        "重试成功后全部投递"
    );
}

#[test]
fn remove_last_supervisor_is_rejected_to_prevent_orphan() {
    let e = env();
    let team = e.team.clone();
    // 移除唯一 supervisor → 拒绝（防孤儿）。
    let err = e.svc.remove_member(&team, &e.sup, &e.sup).unwrap_err();
    assert!(matches!(err, TeamError::LastSupervisor(_)));
    assert_eq!(e.svc.snapshot(&team).unwrap().members.len(), 3);
    // 先添加第二个 supervisor，再移除原 supervisor → 放行。
    let sup2 = AgentId::from("sup2");
    e.svc
        .add_member(&team, &e.sup, &sup2, MemberRole::Supervisor)
        .unwrap();
    e.svc.remove_member(&team, &e.sup, &e.sup).unwrap();
    let snap = e.svc.snapshot(&team).unwrap();
    assert_eq!(
        snap.members.get(&sup2),
        Some(&MemberRole::Supervisor),
        "第二个 supervisor 保有管理权"
    );
    assert_eq!(snap.members.get(&e.sup), None);
    // 新 supervisor 仍可管理（审批等权限不孤儿化）。
    e.svc.dissolve_team(&team, &sup2).unwrap();
}

#[test]
fn presence_bridge_reuses_worker_events_and_ignores_non_members() {
    let e = env();
    let team = e.team.clone();
    let events = vec![
        OrchestrationEvent::WorkerCreated {
            agent_id: e.w1.clone(),
            tenant_id: TenantId::from("ten"),
            parent_id: Some(e.sup.clone()),
            role: WorkerRole::Worker,
            session_id: team.clone(),
            worktree_path: None,
            created_at_ms: 1,
        },
        OrchestrationEvent::WorkerAdmitted {
            agent_id: e.w1.clone(),
            at_ms: 1,
        },
        OrchestrationEvent::WorkerStarted {
            agent_id: e.w1.clone(),
            at_ms: 2,
        },
        OrchestrationEvent::WorkerRunning {
            agent_id: e.w1.clone(),
            at_ms: 3,
        },
        // 非成员：桥忽略，不失败。
        OrchestrationEvent::WorkerWaiting {
            agent_id: AgentId::from("ghost"),
            at_ms: 3,
        },
    ];
    let emitted = e.svc.observe_worker_events(&team, &events).unwrap();
    assert_eq!(emitted.len(), 1, "只有 w1 的 Running 翻译为 presence 变化");
    match &emitted[0] {
        TeamEvent::PresenceChanged {
            agent_id, presence, ..
        } => {
            assert_eq!(agent_id, &e.w1);
            assert_eq!(*presence, Presence::Busy);
        }
        other => panic!("unexpected emitted event: {other:?}"),
    }
    assert_eq!(
        e.svc.snapshot(&team).unwrap().presence.get(&e.w1),
        Some(&Presence::Busy)
    );
    // 同状态重放无新事件。
    assert!(e
        .svc
        .observe_worker_events(&team, &events)
        .unwrap()
        .is_empty());
    // 重放确定性：同一事件流重建同一 presence。
    let rebuilt = TeamService::from_envelopes(
        e.sink.events(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    );
    assert_eq!(
        rebuilt.snapshot(&team).unwrap().presence.get(&e.w1),
        Some(&Presence::Busy)
    );
}
