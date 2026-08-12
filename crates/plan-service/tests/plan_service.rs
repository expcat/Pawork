//! P16-1 Plan service 定向测试：状态机、重放一致性、版本链、只读断言、canonical 序列化。

use agent_domain::{PlanEvent, PlanReviewStatus, PlanStepId, PlanStepStatus};
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
