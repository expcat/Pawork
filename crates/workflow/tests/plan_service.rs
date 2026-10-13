//! Plan service 定向测试：Host 命令面、历史事件重放、审批 gate、只读断言。

use pawork_domain::AgentEvent;
use pawork_domain::{
    CheckpointId, PlanCommentAnchor, PlanEvent, PlanId, PlanReviewStatus, PlanStepId,
    PlanStepSnapshot, PlanStepStatus, PlanVersionId,
};
use pawork_workflow::plan::{apply, replay, PlanError, PlanService, PlanState};

fn assert_folds_agree(events: &[PlanEvent]) {
    let refs: Vec<&PlanEvent> = events.iter().collect();
    let mut manual = PlanState::default();
    for event in refs.iter().copied() {
        apply(&mut manual, event);
    }
    let via_replay = replay(refs.iter().copied());
    let via_service = PlanService::from_events(refs.iter().copied());
    assert_eq!(manual.snapshot(), via_replay.snapshot());
    assert_eq!(via_service.plan_snapshot(), via_replay.snapshot());
    assert_eq!(manual.history(), via_replay.history());
}

fn step(id: &str, text: &str) -> PlanStepSnapshot {
    PlanStepSnapshot {
        step_id: PlanStepId::new(id),
        text: text.into(),
        status: PlanStepStatus::Pending,
    }
}

/// 历史持久化序列：步骤推进、评审回合、评论、修订、批准。
/// 这些变体不再由命令面发出，重放仍必须折叠。
fn historical_review_events() -> Vec<PlanEvent> {
    let plan_id = PlanId::new("plan_1");
    let v1 = PlanVersionId::new("planver_1");
    let v2 = PlanVersionId::new("planver_99");
    let step_a = PlanStepId::new("step_1");
    let anchor = PlanCommentAnchor {
        step_id: step_a.clone(),
        line_offset: Some(12),
        file: Some("src/main.rs".into()),
        file_line: Some(42),
    };
    vec![
        PlanEvent::Created {
            plan_id: plan_id.clone(),
            version: v1.clone(),
            title: "迁移".into(),
            steps: vec![step("step_1", "分析"), step("step_2", "执行")],
        },
        PlanEvent::StepUpdated {
            plan_id: plan_id.clone(),
            step_id: step_a.clone(),
            status: PlanStepStatus::InProgress,
            note: None,
        },
        PlanEvent::StepUpdated {
            plan_id: plan_id.clone(),
            step_id: step_a,
            status: PlanStepStatus::Completed,
            note: Some("done".into()),
        },
        PlanEvent::ReviewRequested {
            plan_id: plan_id.clone(),
            version: v1.clone(),
        },
        PlanEvent::CommentAdded {
            plan_id: plan_id.clone(),
            version: v1.clone(),
            anchor,
            body: "这步需要补测试".into(),
        },
        PlanEvent::ReviewRequested {
            plan_id: plan_id.clone(),
            version: v1.clone(),
        },
        PlanEvent::Revised {
            plan_id: plan_id.clone(),
            version: v2.clone(),
            parent_version: v1,
            title: "revised title".into(),
            steps: vec![step("step_50", "改计划")],
        },
        PlanEvent::ReviewRequested {
            plan_id: plan_id.clone(),
            version: v2.clone(),
        },
        PlanEvent::Approved {
            plan_id,
            version: v2,
            checkpoint_id: Some(CheckpointId::new("ckpt_7")),
        },
    ]
}

#[test]
fn command_errors() {
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    assert!(matches!(
        svc.create_plan("p2", vec!["b".into()]),
        Err(PlanError::AlreadyExists(_))
    ));

    let svc2 = PlanService::new();
    assert!(matches!(
        svc2.replace_plan("x", vec!["y".into()]),
        Err(PlanError::NotCreated)
    ));

    let svc5 = PlanService::new();
    assert!(matches!(
        svc5.create_plan("p", vec![]),
        Err(PlanError::EmptyPlan)
    ));
    let svc6 = PlanService::new();
    assert!(matches!(
        svc6.create_plan("p", vec!["   ".into()]),
        Err(PlanError::EmptyStepText)
    ));
}

#[test]
fn plan_with_write_action_descriptions_is_inert() {
    let dangerous: Vec<String> = vec![
        "运行 `rm -rf /`".into(),
        "执行 shell: write /etc/passwd".into(),
        "spawn child process and exec payload".into(),
    ];
    let svc = PlanService::new();
    let created = svc
        .create_plan("危险（仅文本）", dangerous.clone())
        .unwrap();
    let PlanEvent::Created { plan_id, steps, .. } = &created else {
        panic!("expected Created");
    };

    let mut state = PlanState::default();
    apply(&mut state, &created);
    apply(
        &mut state,
        &PlanEvent::StepUpdated {
            plan_id: plan_id.clone(),
            step_id: steps[0].step_id.clone(),
            status: PlanStepStatus::Completed,
            note: None,
        },
    );

    let snapshot = state.snapshot().unwrap();
    let texts: Vec<String> = snapshot.steps.iter().map(|s| s.text.clone()).collect();
    assert_eq!(texts, dangerous);
    assert_eq!(snapshot.steps[0].status, PlanStepStatus::Completed);
    assert_eq!(snapshot.review_status, PlanReviewStatus::Draft);
}

#[test]
fn host_commands_replay_matches_manual_apply() {
    let svc = PlanService::new();
    let mut events = Vec::new();
    events.push(
        svc.create_plan("v1", vec!["s1".into(), "s2".into()])
            .unwrap(),
    );
    events.push(
        svc.replace_plan("v2", vec!["n1".into(), "n2".into(), "n3".into()])
            .unwrap(),
    );
    let snap = svc.plan_snapshot().unwrap();
    events.push(svc.request_review(&snap.version).unwrap());
    let checkpoint = CheckpointId::new("ckpt_1");
    events.push(
        svc.approve(&snap.plan_id, &snap.version, Some(checkpoint.clone()))
            .unwrap(),
    );

    assert_folds_agree(&events);
    let via_replay = replay(events.iter());
    assert_eq!(via_replay.snapshot(), svc.plan_snapshot());
    assert_eq!(via_replay.history().len(), 2);
    assert_eq!(
        via_replay.history()[1].parent_version.as_ref(),
        Some(&via_replay.history()[0].version)
    );
    let replayed = PlanService::from_events(&events);
    assert!(replayed.is_approved_for_execution(&snap.plan_id, &snap.version));
    assert_eq!(
        replayed
            .plan_snapshot()
            .unwrap()
            .approved_checkpoint_id
            .as_ref(),
        Some(&checkpoint)
    );
}

#[test]
fn replace_plan_forms_version_chain() {
    let svc = PlanService::new();
    let created = svc.create_plan("v1", vec!["a".into()]).unwrap();
    let replaced = svc.replace_plan("v2", vec!["b".into()]).unwrap();
    let replaced_again = svc
        .replace_plan("v3", vec!["c".into(), "d".into()])
        .unwrap();
    let state = replay([&created, &replaced, &replaced_again]);
    let history = state.history();
    assert_eq!(history.len(), 3);
    assert!(history[0].parent_version.is_none());
    assert_eq!(
        history[1].parent_version.as_ref(),
        Some(&history[0].version)
    );
    assert_eq!(
        history[2].parent_version.as_ref(),
        Some(&history[1].version)
    );
    let snapshot = state.snapshot().unwrap();
    assert_eq!(snapshot.version, history[2].version);
    assert_eq!(snapshot.review_status, PlanReviewStatus::Draft);
    assert_eq!(snapshot.steps.len(), 2);
    assert!(snapshot.comments.is_empty());
}

#[test]
fn historical_plan_events_replay_identically() {
    let events = historical_review_events();
    let stepped = replay(&events[..2]);
    assert_eq!(stepped.steps()[0].status, PlanStepStatus::InProgress);
    assert_eq!(stepped.steps()[0].text, "分析");
    assert_eq!(stepped.steps()[1].status, PlanStepStatus::Pending);

    let mut ignored = stepped.clone();
    apply(
        &mut ignored,
        &PlanEvent::StepUpdated {
            plan_id: PlanId::new("plan_1"),
            step_id: PlanStepId::new("step_missing"),
            status: PlanStepStatus::Completed,
            note: None,
        },
    );
    assert_eq!(ignored.steps()[0].status, PlanStepStatus::InProgress);
    assert_eq!(ignored.steps().len(), 2);

    assert_folds_agree(&events);
    let state = replay(&events);
    let snapshot = state.snapshot().unwrap();
    assert_eq!(snapshot.title, "revised title");
    assert_eq!(snapshot.version, PlanVersionId::new("planver_99"));
    assert_eq!(snapshot.steps, vec![step("step_50", "改计划")]);
    assert_eq!(snapshot.review_status, PlanReviewStatus::Approved);
    assert_eq!(
        snapshot
            .approved_checkpoint_id
            .as_ref()
            .map(|id| id.as_str()),
        Some("ckpt_7")
    );
    assert_eq!(snapshot.comments.len(), 1);
    assert_eq!(snapshot.comments[0].body, "这步需要补测试");
    assert_eq!(snapshot.comments[0].anchor.line_offset, Some(12));

    let history = state.history();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[1].parent_version.as_ref(),
        Some(&PlanVersionId::new("planver_1"))
    );
    assert_eq!(history[1].title, "revised title");
    assert_eq!(history[1].version, PlanVersionId::new("planver_99"));

    let plan_id = PlanId::new("plan_1");
    let v2 = PlanVersionId::new("planver_99");
    let replayed = PlanService::from_events(&events);
    assert!(replayed.is_approved_for_execution(&plan_id, &v2));
    assert!(!replayed.is_approved_for_execution(&plan_id, &PlanVersionId::new("planver_1")));
}

#[test]
fn from_events_seeds_counters_from_historical_revision() {
    let events = historical_review_events();
    let svc = PlanService::from_events(&events);
    let replaced = svc.replace_plan("next", vec!["z".into()]).unwrap();
    match replaced {
        PlanEvent::Replaced {
            version,
            parent_version,
            steps,
            ..
        } => {
            assert_eq!(version, PlanVersionId::new("planver_100"));
            assert_eq!(parent_version, PlanVersionId::new("planver_99"));
            assert_eq!(steps[0].step_id, PlanStepId::new("step_51"));
        }
        other => panic!("expected Replaced, got {other:?}"),
    }
}

#[test]
fn replayed_changes_requested_stays_closed_until_host_decides() {
    let events = historical_review_events();
    // Created + 两次 ReviewRequested（中间夹着步骤与评论）到达 ChangesRequested。
    let changes_requested = &events[..6];
    let svc = PlanService::from_events(changes_requested);
    let snap = svc.plan_snapshot().unwrap();
    assert_eq!(snap.review_status, PlanReviewStatus::ChangesRequested);
    assert!(!svc.is_approved_for_execution(&snap.plan_id, &snap.version));

    let approved = PlanService::from_events(changes_requested);
    approved
        .approve(
            &snap.plan_id,
            &snap.version,
            Some(CheckpointId::new("ckpt_9")),
        )
        .unwrap();
    assert!(approved.is_approved_for_execution(&snap.plan_id, &snap.version));

    let rejected = PlanService::from_events(changes_requested);
    rejected
        .reject(&snap.plan_id, &snap.version, "优先级不足")
        .unwrap();
    assert_eq!(
        rejected.plan_snapshot().unwrap().review_status,
        PlanReviewStatus::Rejected
    );
    assert!(!rejected.is_approved_for_execution(&snap.plan_id, &snap.version));
    assert!(matches!(
        rejected.request_review(&snap.version),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
}

#[test]
fn approval_gate_closed_until_approved() {
    let svc = PlanService::new();
    assert!(
        !svc.is_approved_for_execution(&PlanId::new("plan_1"), &PlanVersionId::new("planver_1"))
    );

    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));
    svc.request_review(&v1).unwrap();
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));
    svc.reject(&plan_id, &v1, "方向不对").unwrap();
    assert!(!svc.is_approved_for_execution(&plan_id, &v1));
}

#[test]
fn illegal_review_transitions_rejected() {
    let svc = PlanService::new();
    svc.create_plan("p", vec!["a".into()]).unwrap();
    let plan_id = svc.plan_snapshot().unwrap().plan_id.clone();
    let v1 = svc.plan_snapshot().unwrap().version.clone();

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

    svc.request_review(&v1).unwrap();
    assert!(matches!(
        svc.request_review(&v1),
        Err(PlanError::IllegalReviewTransition { .. })
    ));

    svc.approve(&plan_id, &v1, None).unwrap();
    assert!(matches!(
        svc.approve(&plan_id, &v1, None),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
    assert!(matches!(
        svc.request_review(&v1),
        Err(PlanError::IllegalReviewTransition { .. })
    ));
}

#[test]
fn approve_directly_from_review() {
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
    assert!(!svc.is_approved_for_execution(&plan_id, &PlanVersionId::new("planver_x")));
    assert!(!svc.is_approved_for_execution(&PlanId::new("plan_x"), &v1));
}

#[test]
fn review_command_errors() {
    let svc = PlanService::new();
    let pid = PlanId::new("plan_1");
    let vid = PlanVersionId::new("planver_1");
    assert!(matches!(
        svc.request_review(&vid),
        Err(PlanError::NotCreated)
    ));
    assert!(matches!(
        svc.approve(&pid, &vid, None),
        Err(PlanError::NotCreated)
    ));
    assert!(matches!(
        svc.reject(&pid, &vid, "x"),
        Err(PlanError::NotCreated)
    ));

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
    assert!(matches!(
        svc.approve(&plan_id, &PlanVersionId::new("planver_9"), None),
        Err(PlanError::VersionMismatch { .. })
    ));
}

#[test]
fn plan_events_round_trip_through_agent_event() {
    let svc = PlanService::new();
    let created = svc.create_plan("canonical", vec!["a".into()]).unwrap();
    let mut events = historical_review_events();
    events.push(created);
    events.push(PlanEvent::Rejected {
        plan_id: PlanId::new("plan_1"),
        version: PlanVersionId::new("planver_1"),
        reason: "方向不对".into(),
    });
    events.push(PlanEvent::Replaced {
        plan_id: PlanId::new("plan_1"),
        version: PlanVersionId::new("planver_2"),
        parent_version: PlanVersionId::new("planver_1"),
        title: "replaced".into(),
        steps: vec![step("step_9", "b")],
    });

    for event in events {
        let wrapped = AgentEvent::Plan(event.clone());
        let json = serde_json::to_string(&wrapped).expect("serialize AgentEvent::Plan");
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize AgentEvent::Plan");
        assert_eq!(back, wrapped);

        let pe_json = serde_json::to_string(&event).unwrap();
        let pe_back: PlanEvent = serde_json::from_str(&pe_json).unwrap();
        assert_eq!(pe_back, event);
    }
}
