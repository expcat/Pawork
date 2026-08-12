//! P16-1/P16-2 Plan service 定向测试：步骤/评审状态机、重放一致性、版本链、
//! 修订、审批 gate、行锚点评审意见、只读断言、canonical 序列化。

use agent_domain::{
    CheckpointId, PlanCommentAnchor, PlanEvent, PlanId, PlanReviewStatus, PlanStepId,
    PlanStepStatus, PlanVersionId,
};
use agent_events::AgentEvent;
use plan_service::{apply, replay, PlanError, PlanService, PlanState};

fn step_id_at(event: &PlanEvent, idx: usize) -> PlanStepId {
    match event {
        PlanEvent::Created { steps, .. } | PlanEvent::Replaced { steps, .. } => {
            steps[idx].step_id.clone()
        }
        _ => panic!("expected Created/Replaced event"),
    }
}

#[test]
fn legal_transitions_succeed() {
    let svc = PlanService::new();
    let created = svc
        .create_plan("迁移", vec!["分析".into(), "导出".into(), "导入".into()])
        .unwrap();

    let s0 = step_id_at(&created, 0);
    svc.update_step(&s0, PlanStepStatus::InProgress, None).unwrap();
    svc.update_step(&s0, PlanStepStatus::Completed, None).unwrap();

    let s1 = step_id_at(&created, 1);
    svc.update_step(&s1, PlanStepStatus::InProgress, None).unwrap();
    svc.update_step(&s1, PlanStepStatus::Blocked, Some("卡住".into())).unwrap();
    svc.update_step(&s1, PlanStepStatus::InProgress, None).unwrap();
    svc.update_step(&s1, PlanStepStatus::Completed, None).unwrap();
}

#[test]
fn illegal_transitions_rejected() {
    let svc = PlanService::new();
    let created = svc.create_plan("p", vec!["a".into()]).unwrap();
    let s = step_id_at(&created, 0);

    // Pending 直达 Completed / Blocked / 自环均非法。
    assert!(matches!(
        svc.update_step(&s, PlanStepStatus::Completed, None),
        Err(PlanError::IllegalStepTransition { .. })
    ));
    assert!(matches!(
        svc.update_step(&s, PlanStepStatus::Blocked, None),
        Err(PlanError::IllegalStepTransition { .. })
    ));
    assert!(matches!(
        svc.update_step(&s, PlanStepStatus::Pending, None),
        Err(PlanError::IllegalStepTransition { .. })
    ));

    // Pending -> InProgress 合法；其后回退到 Pending 非法。
    svc.update_step(&s, PlanStepStatus::InProgress, None).unwrap();
    assert!(matches!(
        svc.update_step(&s, PlanStepStatus::Pending, None),
        Err(PlanError::IllegalStepTransition { .. })
    ));

    // 完成后为终态，任何转移非法。
    svc.update_step(&s, PlanStepStatus::Completed, None).unwrap();
    assert!(matches!(
        svc.update_step(&s, PlanStepStatus::InProgress, None),
        Err(PlanError::IllegalStepTransition { .. })
    ));
}

#[test]
fn replay_matches_live_service_and_manual_apply() {
    let svc = PlanService::new();
    let mut events: Vec<PlanEvent> = Vec::new();

    let created = svc
        .create_plan("v1", vec!["s1".into(), "s2".into()])
        .unwrap();
    events.push(created.clone());
    let s1 = step_id_at(&created, 0);
    events.push(svc.update_step(&s1, PlanStepStatus::InProgress, None).unwrap());
    events.push(svc.update_step(&s1, PlanStepStatus::Completed, Some("done".into())).unwrap());
    events.push(svc.replace_plan("v2", vec!["n1".into(), "n2".into(), "n3".into()]).unwrap());
    let n1 = svc.plan_snapshot().unwrap().steps[0].step_id.clone();
    events.push(svc.update_step(&n1, PlanStepStatus::InProgress, None).unwrap());

    let refs: Vec<&PlanEvent> = events.iter().collect();

    // 1) 逐步 apply 一致性。
    let mut manual = PlanState::default();
    for ev in refs.iter().copied() {
        apply(&mut manual, ev);
    }
    // 2) replay 一致性。
    let via_replay = replay(refs.iter().copied());
    // 3) PlanService::from_events 一致性。
    let replayed = PlanService::from_events(refs.iter().copied());

    assert_eq!(manual.snapshot(), svc.plan_snapshot());
    assert_eq!(via_replay.snapshot(), svc.plan_snapshot());
    assert_eq!(replayed.plan_snapshot(), svc.plan_snapshot());
    assert_eq!(replayed.version_history(), svc.version_history());
}

#[test]
fn version_history_forms_chain() {
    let svc = PlanService::new();
    svc.create_plan("v1", vec!["a".into()]).unwrap();
    svc.replace_plan("v2", vec!["b".into()]).unwrap();
    svc.replace_plan("v3", vec!["c".into(), "d".into()]).unwrap();

    let history = svc.version_history();
    assert_eq!(history.len(), 3);
    assert!(history[0].parent_version.is_none());
    assert_eq!(history[1].parent_version.as_ref(), Some(&history[0].version));
    assert_eq!(history[2].parent_version.as_ref(), Some(&history[1].version));

    let snapshot = svc.plan_snapshot().unwrap();
    assert_eq!(snapshot.version, history[2].version);
    assert_eq!(snapshot.review_status, PlanReviewStatus::Draft);
    assert_eq!(snapshot.steps.len(), 2);
}

#[test]
fn command_errors() {
    // create twice.
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    assert!(matches!(
        svc.create_plan("p2", vec!["b".into()]),
        Err(PlanError::AlreadyExists(_))
    ));

    // replace before create.
    let svc2 = PlanService::new();
    assert!(matches!(
        svc2.replace_plan("x", vec!["y".into()]),
        Err(PlanError::NotCreated)
    ));

    // update_step on missing step.
    let svc3 = PlanService::new();
    svc3.create_plan("p", vec!["a".into()]).unwrap();
    let missing = PlanStepId::new("step_999");
    assert!(matches!(
        svc3.update_step(&missing, PlanStepStatus::InProgress, None),
        Err(PlanError::StepNotFound(_))
    ));

    // update_step before any plan.
    let svc4 = PlanService::new();
    assert!(matches!(
        svc4.update_step(&missing, PlanStepStatus::InProgress, None),
        Err(PlanError::NotCreated)
    ));

    // empty plan / empty step text.
    let svc5 = PlanService::new();
    assert!(matches!(svc5.create_plan("p", vec![]), Err(PlanError::EmptyPlan)));
    let svc6 = PlanService::new();
    assert!(matches!(
        svc6.create_plan("p", vec!["   ".into()]),
        Err(PlanError::EmptyStepText)
    ));
}

#[test]
fn plan_with_write_action_descriptions_is_inert() {
    // 构造一个「带写动作描述」的 Plan——文本仅作为惰性数据，绝不应被执行。
    let dangerous: Vec<String> = vec![
        "运行 `rm -rf /`".into(),
        "执行 shell: write /etc/passwd".into(),
        "spawn child process and exec payload".into(),
    ];
    let svc = PlanService::new();
    let created = svc.create_plan("危险（仅文本）", dangerous.clone()).unwrap();
    let steps = match &created {
        PlanEvent::Created { steps, .. } => steps.clone(),
        _ => unreachable!(),
    };
    for s in &steps {
        svc.update_step(&s.step_id, PlanStepStatus::InProgress, None).unwrap();
        svc.update_step(&s.step_id, PlanStepStatus::Completed, None).unwrap();
    }

    let snapshot = svc.plan_snapshot().unwrap();
    // 步骤文本原样保留（未被解释/执行/改写）。
    let texts: Vec<String> = snapshot.steps.iter().map(|s| s.text.clone()).collect();
    assert_eq!(texts, dangerous);
    // 状态机走到 completed，评审仍为 Draft。
    assert!(snapshot.steps.iter().all(|s| s.status == PlanStepStatus::Completed));
    assert_eq!(snapshot.review_status, PlanReviewStatus::Draft);
    // PlanService 无 spawn/exec/write API：仅命令面 + 查询面被调用，
    // 故「带写动作描述的 Plan」不可能产生任何副作用。
}

#[test]
fn source_has_no_io_or_spawn_api() {
    // 静态只读保证：扫描本 crate 实现源码，确认不存在任何进程/文件/网络 IO 入口。
    let src = concat!(
        include_str!("../src/lib.rs"),
        include_str!("../src/state.rs"),
        include_str!("../src/snapshot.rs"),
        include_str!("../src/error.rs"),
        include_str!("../src/service.rs"),
    );
    let forbidden = [
        "std::process",
        "process::Command",
        "Command::new",
        "std::fs",
        "fs::write",
        "fs::remove",
        "File::create",
        "OpenOptions",
        "std::net",
        "tokio::spawn",
        "std::thread",
        ".spawn(",
        "reqwest",
    ];
    for token in forbidden {
        assert!(
            !src.contains(token),
            "read-only violation: source references `{token}`"
        );
    }
}

#[test]
fn plan_event_round_trips_through_agent_event() {
    let svc = PlanService::new();
    let event = svc.create_plan("canonical", vec!["a".into(), "b".into()]).unwrap();

    // PlanEvent 经 AgentEvent::Plan 包装后 JSON 往返（canonical 持久化兼容）。
    let wrapped = AgentEvent::Plan(event.clone());
    let json = serde_json::to_string(&wrapped).expect("serialize AgentEvent::Plan");
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize AgentEvent::Plan");
    assert_eq!(back, wrapped);

    // PlanEvent 自身也可独立 JSON 往返。
    let pe_json = serde_json::to_string(&event).unwrap();
    let pe_back: PlanEvent = serde_json::from_str(&pe_json).unwrap();
    assert_eq!(pe_back, event);
}

#[test]
fn review_revise_approve_flow_with_checkpoint() {
    let svc = PlanService::new();
    svc.create_plan("迁移", vec!["分析".into(), "执行".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();

    // Draft：gate 关闭。
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));

    // draft → in_review。
    svc.request_review(&v1).unwrap();
    assert_eq!(
        svc.plan_snapshot().unwrap().review_status,
        PlanReviewStatus::InReview
    );
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));

    // in_review → changes_requested。
    svc.request_changes(&v1).unwrap();
    assert_eq!(
        svc.plan_snapshot().unwrap().review_status,
        PlanReviewStatus::ChangesRequested
    );

    // 修订：新版本带 parent_version，旧版本保留在历史中。
    let v2 = PlanVersionId::new("planver_99");
    svc.revise(&v2, &v1).unwrap();
    let snap = svc.plan_snapshot().unwrap();
    assert_eq!(snap.version, v2);
    assert_eq!(snap.review_status, PlanReviewStatus::Draft);
    let history = svc.version_history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].parent_version.as_ref(), Some(&v1));

    // 重新提交评审 → 审批（带 checkpoint）。
    svc.request_review(&v2).unwrap();
    let cp = CheckpointId::new("ckpt_7");
    svc.approve(&plan_id, &v2, Some(cp.clone())).unwrap();
    let snap = svc.plan_snapshot().unwrap();
    assert_eq!(snap.review_status, PlanReviewStatus::Approved);
    assert_eq!(snap.approved_checkpoint_id.as_ref(), Some(&cp));

    // gate 仅放行已批准版本的精确 plan_id + version。
    assert!(svc.is_approved_for_execution(&plan_id, &v2));
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));
    assert!(!svc.is_approved_for_execution(&PlanId::new("plan_x"), &v2));
    assert!(!svc.is_approved_for_execution(&plan_id, &PlanVersionId::new("planver_x")));
}

#[test]
fn approval_gate_closed_until_approved() {
    // 未创建 Plan：gate 关闭。
    let svc = PlanService::new();
    assert!(!svc.is_approved_for_execution(
        &PlanId::new("plan_1"),
        &PlanVersionId::new("planver_1")
    ));

    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    let v2 = PlanVersionId::new("planver_80");

    assert!(!svc.is_approved_for_execution(&plan_id, &v1)); // Draft
    svc.request_review(&v1).unwrap();
    assert!(!svc.is_approved_for_execution(&plan_id, &v1)); // InReview
    svc.request_changes(&v1).unwrap();
    assert!(!svc.is_approved_for_execution(&plan_id, &v1)); // ChangesRequested
    svc.revise(&v2, &v1).unwrap();
    svc.request_review(&v2).unwrap();
    svc.reject(&plan_id, &v2, "方向不对").unwrap();
    assert!(!svc.is_approved_for_execution(&plan_id, &v2)); // Rejected（终态）
}

#[test]
fn illegal_review_transitions_rejected() {
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();

    // Draft 不能直接 Approved / Rejected。
    assert!(matches!(
        svc.approve(&plan_id, &v1, None),
        Err(PlanError::IllegalReviewTransition {
            to: PlanReviewStatus::Approved,
            ..
        })
    ));
    assert!(matches!(
        svc.reject(&plan_id, &v1, "no"),
        Err(PlanError::IllegalReviewTransition {
            to: PlanReviewStatus::Rejected,
            ..
        })
    ));

    // Draft → InReview 后重复提交评审非法。
    svc.request_review(&v1).unwrap();
    assert!(matches!(
        svc.request_review(&v1),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
    // InReview → ChangesRequested 合法；重复请求修改非法。
    svc.request_changes(&v1).unwrap();
    assert!(matches!(
        svc.request_changes(&v1),
        Err(PlanError::IllegalReviewTransition { .. })
    ));

    // ChangesRequested → 修订（Draft）合法；但 InReview 状态下不能直接修订。
    let v2 = PlanVersionId::new("planver_98");
    svc.revise(&v2, &v1).unwrap();
    svc.request_review(&v2).unwrap();
    assert!(matches!(
        svc.revise(&PlanVersionId::new("planver_97"), &v2),
        Err(PlanError::NotChangesRequested { .. })
    ));

    // 审批后为终态：不能再次审批 / 提交评审。
    svc.approve(&plan_id, &v2, None).unwrap();
    assert!(matches!(
        svc.approve(&plan_id, &v2, None),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
    assert!(matches!(
        svc.request_review(&v2),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
}

#[test]
fn approve_or_reject_directly_from_review() {
    // InReview 可直接审批（无需先请求修改）。
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    svc.request_review(&v1).unwrap();
    svc.approve(&plan_id, &v1, None).unwrap();
    assert_eq!(
        svc.plan_snapshot().unwrap().review_status,
        PlanReviewStatus::Approved
    );
    assert!(svc.is_approved_for_execution(&plan_id, &v1));

    // ChangesRequested 可直接拒绝（终态，gate 保持关闭）。
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    svc.request_review(&v1).unwrap();
    svc.request_changes(&v1).unwrap();
    svc.reject(&plan_id, &v1, "优先级不足").unwrap();
    assert_eq!(
        svc.plan_snapshot().unwrap().review_status,
        PlanReviewStatus::Rejected
    );
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));
    assert!(matches!(
        svc.request_review(&v1),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
    assert!(matches!(
        svc.revise(&PlanVersionId::new("planver_70"), &v1),
        Err(PlanError::NotChangesRequested { .. })
    ));
}

#[test]
fn comments_carry_line_anchors() {
    let svc = PlanService::new();
    let created = svc.create_plan("p", vec!["a".into(), "b".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    let s0 = step_id_at(&created, 0);

    let anchor = PlanCommentAnchor {
        step_id: s0.clone(),
        line_offset: Some(12),
        file: Some("src/main.rs".into()),
        file_line: Some(42),
    };
    svc.add_comment(&plan_id, &v1, anchor.clone(), "这步需要补测试").unwrap();
    let snap = svc.plan_snapshot().unwrap();
    assert_eq!(snap.comments.len(), 1);
    assert_eq!(snap.comments[0].anchor, anchor);
    assert_eq!(snap.comments[0].body, "这步需要补测试");

    // 错误：未知步骤 / 版本不匹配 / plan_id 不匹配 / 空正文。
    let unknown = PlanCommentAnchor {
        step_id: PlanStepId::new("step_999"),
        line_offset: None,
        file: None,
        file_line: None,
    };
    assert!(matches!(
        svc.add_comment(&plan_id, &v1, unknown, "x"),
        Err(PlanError::StepNotFound(_))
    ));
    assert!(matches!(
        svc.add_comment(&plan_id, &PlanVersionId::new("planver_9"), anchor.clone(), "x"),
        Err(PlanError::VersionMismatch { .. })
    ));
    assert!(matches!(
        svc.add_comment(&PlanId::new("plan_x"), &v1, anchor.clone(), "x"),
        Err(PlanError::PlanIdMismatch { .. })
    ));
    assert!(matches!(
        svc.add_comment(&plan_id, &v1, anchor, "   "),
        Err(PlanError::EmptyComment)
    ));
}

#[test]
fn revise_validates_version_chain() {
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    svc.request_review(&v1).unwrap();
    svc.request_changes(&v1).unwrap();

    // parent_version 必须等于当前版本。
    assert!(matches!(
        svc.revise(
            &PlanVersionId::new("planver_60"),
            &PlanVersionId::new("planver_59")
        ),
        Err(PlanError::VersionMismatch { .. })
    ));
    // 新版本必须不同于 parent。
    assert!(matches!(
        svc.revise(&v1, &v1),
        Err(PlanError::SameVersion(_))
    ));

    // 未到 changes_requested 不能修订。
    let svc2 = PlanService::new();
    svc2.create_plan("p", vec!["a".into()]).unwrap();
    let v1 = svc2.plan_snapshot().unwrap().version.clone();
    assert!(matches!(
        svc2.revise(&PlanVersionId::new("planver_60"), &v1),
        Err(PlanError::NotChangesRequested { .. })
    ));

    // 未创建 Plan 不能修订。
    let svc3 = PlanService::new();
    assert!(matches!(
        svc3.revise(
            &PlanVersionId::new("planver_60"),
            &PlanVersionId::new("planver_1")
        ),
        Err(PlanError::NotCreated)
    ));
}

#[test]
fn review_command_errors() {
    // 未创建 Plan 时，评审 / 审批 / 评论命令全部报 NotCreated。
    let svc = PlanService::new();
    let pid = PlanId::new("plan_1");
    let vid = PlanVersionId::new("planver_1");
    assert!(matches!(svc.request_review(&vid), Err(PlanError::NotCreated)));
    assert!(matches!(
        svc.request_changes(&vid),
        Err(PlanError::NotCreated)
    ));
    assert!(matches!(svc.revise(&vid, &vid), Err(PlanError::NotCreated)));
    assert!(matches!(
        svc.approve(&pid, &vid, None),
        Err(PlanError::NotCreated)
    ));
    assert!(matches!(
        svc.reject(&pid, &vid, "x"),
        Err(PlanError::NotCreated)
    ));
    assert!(matches!(
        svc.add_comment(
            &pid,
            &vid,
            PlanCommentAnchor {
                step_id: PlanStepId::new("step_1"),
                line_offset: None,
                file: None,
                file_line: None,
            },
            "x"
        ),
        Err(PlanError::NotCreated)
    ));

    // 拒绝必须给出理由；plan_id 必须匹配。
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    assert!(matches!(
        svc.reject(&plan_id, &v1, "  "),
        Err(PlanError::EmptyReason)
    ));
    svc.request_review(&v1).unwrap();
    assert!(matches!(
        svc.reject(&plan_id, &v1, ""),
        Err(PlanError::EmptyReason)
    ));
    assert!(matches!(
        svc.approve(&PlanId::new("plan_99"), &v1, None),
        Err(PlanError::PlanIdMismatch { .. })
    ));
}

#[test]
fn review_flow_replays_identically() {
    let svc = PlanService::new();
    let mut events: Vec<PlanEvent> = Vec::new();
    events.push(svc.create_plan("v1", vec!["s1".into(), "s2".into()]).unwrap());
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    events.push(svc.request_review(&v1).unwrap());
    events.push(svc.request_changes(&v1).unwrap());
    let v2 = PlanVersionId::new("planver_99");
    events.push(svc.revise(&v2, &v1).unwrap());
    events.push(svc.request_review(&v2).unwrap());
    let anchor = PlanCommentAnchor {
        step_id: svc.plan_snapshot().unwrap().steps[0].step_id.clone(),
        line_offset: Some(3),
        file: None,
        file_line: None,
    };
    events.push(svc.add_comment(&plan_id, &v2, anchor, "补充边界用例").unwrap());
    let cp = CheckpointId::new("ckpt_42");
    events.push(svc.approve(&plan_id, &v2, Some(cp.clone())).unwrap());

    let refs: Vec<&PlanEvent> = events.iter().collect();
    let mut manual = PlanState::default();
    for ev in refs.iter().copied() {
        apply(&mut manual, ev);
    }
    let via_replay = replay(refs.iter().copied());
    let replayed = PlanService::from_events(refs.iter().copied());

    // 三种重建路径与在线 service 完全一致（评审状态 / 评论 / checkpoint 全含）。
    assert_eq!(manual.snapshot(), svc.plan_snapshot());
    assert_eq!(via_replay.snapshot(), svc.plan_snapshot());
    assert_eq!(replayed.plan_snapshot(), svc.plan_snapshot());
    assert_eq!(replayed.version_history(), svc.version_history());
    assert!(replayed.is_approved_for_execution(&plan_id, &v2));
    assert_eq!(
        replayed.plan_snapshot().unwrap().approved_checkpoint_id,
        Some(cp)
    );

    // 修订链可追溯：v2.parent == v1，历史含两版。
    let history = svc.version_history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].parent_version.as_ref(), Some(&v1));
    assert_eq!(history[1].version, v2);
}

#[test]
fn review_events_round_trip_through_agent_event() {
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    svc.request_review(&v1).unwrap();
    svc.request_changes(&v1).unwrap();
    let revised = svc.revise(&PlanVersionId::new("planver_99"), &v1).unwrap();
    svc.request_review(&PlanVersionId::new("planver_99")).unwrap();
    let approved = svc
        .approve(
            &plan_id,
            &PlanVersionId::new("planver_99"),
            Some(CheckpointId::new("ckpt_1")),
        )
        .unwrap();

    // Approved 载荷带 checkpoint_id。
    match &approved {
        PlanEvent::Approved { checkpoint_id, .. } => {
            assert_eq!(checkpoint_id.as_ref().map(|c| c.as_str()), Some("ckpt_1"));
        }
        _ => panic!("expected Approved event"),
    }

    // Revised / Approved 经 AgentEvent::Plan 包装后 JSON 往返。
    for event in [revised, approved] {
        let wrapped = AgentEvent::Plan(event.clone());
        let json = serde_json::to_string(&wrapped).expect("serialize AgentEvent::Plan");
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize AgentEvent::Plan");
        assert_eq!(back, wrapped);
    }
}

#[test]
fn review_surface_adds_no_write_or_exec_api() {
    // P16-2 只新增只读评审 / 审批命令：service 方法面不允许出现任何
    // 写文件 / 执行 / 派生进程 / 应用补丁类入口（审批不扩权）。
    let src = include_str!("../src/service.rs");
    let forbidden_prefixes = [
        "pub fn write",
        "pub fn exec",
        "pub fn spawn",
        "pub fn run",
        "pub fn apply",
        "pub fn shell",
        "pub fn remove",
        "pub fn delete",
        "pub fn launch",
    ];
    for line in src.lines().filter(|l| l.trim_start().starts_with("pub fn")) {
        for prefix in forbidden_prefixes {
            assert!(
                !line.contains(prefix),
                "write-like API leaked in service surface: {line}"
            );
        }
    }
}
