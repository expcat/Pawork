//! P16-5 Scheduled Automation 定向测试：四种触发器判定、cron 解析覆盖、派发、
//! result inbox 检索、事件可重放、失败退避、无幽灵 Running 断言。

mod common {
    //! 测试公共辅助：记录派发的 mock dispatcher。

    use std::sync::Mutex;

    use pawork_domain::{AutomationId, BackgroundTaskId};

    use pawork_workflow::automation::{AutomationAction, AutomationDispatcher, AutomationError};

    /// 记录每次派发的 mock dispatcher，返回稳定的 `rec_task_<n>` 任务 ID。
    pub struct RecordingDispatcher {
        calls: Mutex<Vec<(AutomationId, AutomationAction)>>,
    }

    impl Default for RecordingDispatcher {
        fn default() -> Self {
            Self::new()
        }
    }

    impl RecordingDispatcher {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        #[allow(dead_code)]
        pub fn calls(&self) -> Vec<(AutomationId, AutomationAction)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl AutomationDispatcher for RecordingDispatcher {
        fn dispatch(
            &self,
            automation_id: &AutomationId,
            action: &AutomationAction,
        ) -> Result<BackgroundTaskId, AutomationError> {
            let mut calls = self.calls.lock().unwrap();
            let n = calls.len();
            calls.push((automation_id.clone(), action.clone()));
            Ok(BackgroundTaskId::new(format!("rec_task_{n}")))
        }
    }
}

use pawork_domain::{
    ArtifactId, AutomationEvent, AutomationId, AutomationTriggerKind, BackgroundTaskId,
};
use pawork_domain::AgentEvent;
use pawork_workflow::automation::{
    replay, Automation, AutomationAction, AutomationEngine, AutomationError, AutomationTrigger,
    EngineConfig, InboxQuery, InboxStatus,
};

/// 四种触发器按声明时机判定 check_due。
#[test]
fn four_triggers_check_due_timing() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    let now: u64 = 1_000;

    engine
        .register(
            Automation {
                automation_id: AutomationId::from("cron_a"),
                trigger: AutomationTrigger::Cron {
                    expr: "* * * * *".into(),
                },
                action: AutomationAction::Prompt {
                    prompt: "hi".into(),
                },
            },
            now,
        )
        .unwrap();
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("int_a"),
                trigger: AutomationTrigger::Interval { secs: 120 },
                action: AutomationAction::Prompt {
                    prompt: "hi".into(),
                },
            },
            now,
        )
        .unwrap();
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("once_a"),
                trigger: AutomationTrigger::Once { delay_secs: 60 },
                action: AutomationAction::Prompt {
                    prompt: "hi".into(),
                },
            },
            now,
        )
        .unwrap();
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("evt_a"),
                trigger: AutomationTrigger::Event {
                    pattern: "deploy.*prod".into(),
                },
                action: AutomationAction::Prompt {
                    prompt: "hi".into(),
                },
            },
            now,
        )
        .unwrap();

    // now=1000：cron 下次落在 1020（对齐到下一分钟起点），无人到期。
    assert!(engine.check_due(now).is_empty());
    assert_eq!(engine.check_due(1020), vec![AutomationId::from("cron_a")]);
    assert_eq!(
        engine.check_due(1060),
        vec![AutomationId::from("cron_a"), AutomationId::from("once_a")]
    );
    let due_1120 = engine.check_due(1120);
    assert_eq!(due_1120.len(), 3);
    assert!(due_1120.contains(&AutomationId::from("cron_a")));
    assert!(due_1120.contains(&AutomationId::from("int_a")));
    assert!(due_1120.contains(&AutomationId::from("once_a")));
    assert!(!due_1120.contains(&AutomationId::from("evt_a")));
}

/// interval 触发并推进下次时刻；once 触发后不再触发。
#[test]
fn interval_advances_and_once_fires_once() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("int"),
                trigger: AutomationTrigger::Interval { secs: 100 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("once"),
                trigger: AutomationTrigger::Once { delay_secs: 50 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();

    let outcome = engine.fire(&AutomationId::from("int"), 100).unwrap();
    assert_eq!(outcome.fired_at, 100);
    let snap = engine
        .automation_snapshot(&AutomationId::from("int"))
        .unwrap();
    assert_eq!(snap.next_at, Some(200));
    assert_eq!(snap.fired_count, 1);

    engine.fire(&AutomationId::from("once"), 50).unwrap();
    let once_snap = engine
        .automation_snapshot(&AutomationId::from("once"))
        .unwrap();
    assert_eq!(once_snap.next_at, None);
    assert_eq!(once_snap.fired_count, 1);
    assert!(matches!(
        engine.fire(&AutomationId::from("once"), 999),
        Err(AutomationError::OnceAlreadyFired(_))
    ));
}

/// event 触发器：按正则匹配 canonical 载荷并派发；不匹配不触发。
#[test]
fn event_trigger_matches_and_dispatches() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("deploy"),
                trigger: AutomationTrigger::Event {
                    pattern: "deploy.*prod".into(),
                },
                action: AutomationAction::ToolCall {
                    name: "kubectl".into(),
                    input: "{}".into(),
                },
            },
            0,
        )
        .unwrap();

    assert!(engine.match_event("build done staging").is_empty());
    assert_eq!(
        engine.match_event("deploy service to prod"),
        vec![AutomationId::from("deploy")]
    );

    let outcomes = engine.dispatch_event("deploy api to prod", 10);
    assert_eq!(outcomes.len(), 1);
    let outcome = outcomes[0].as_ref().expect("dispatch ok");
    assert_eq!(outcome.automation_id, AutomationId::from("deploy"));
}

/// result inbox：按 automation / 状态 / 时间检索。
#[test]
fn result_inbox_searchable_by_automation_status_time() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("a"),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("b"),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();

    let o1 = engine.fire(&AutomationId::from("a"), 10).unwrap();
    let o2 = engine.fire(&AutomationId::from("a"), 20).unwrap();
    let o3 = engine.fire(&AutomationId::from("b"), 30).unwrap();

    engine
        .record_result(
            &AutomationId::from("a"),
            &o1.task_id,
            ArtifactId::from("art1"),
            None,
            InboxStatus::Succeeded,
            11,
        )
        .unwrap();
    engine
        .record_result(
            &AutomationId::from("a"),
            &o2.task_id,
            ArtifactId::from("art2"),
            None,
            InboxStatus::Failed,
            21,
        )
        .unwrap();
    engine
        .record_result(
            &AutomationId::from("b"),
            &o3.task_id,
            ArtifactId::from("art3"),
            None,
            InboxStatus::Succeeded,
            31,
        )
        .unwrap();

    let a_items = engine.search_inbox(InboxQuery {
        automation_id: Some(&AutomationId::from("a")),
        ..Default::default()
    });
    assert_eq!(a_items.len(), 2);

    let failed = engine.search_inbox(InboxQuery {
        status: Some(InboxStatus::Failed),
        ..Default::default()
    });
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].artifact_id, ArtifactId::from("art2"));

    let window = engine.search_inbox(InboxQuery {
        since: Some(15),
        until: Some(25),
        ..Default::default()
    });
    assert_eq!(window.len(), 1);
    assert_eq!(window[0].recorded_at, 21);

    let all = engine.inbox_items();
    assert_eq!(
        all.iter().map(|i| i.recorded_at).collect::<Vec<_>>(),
        vec![11, 21, 31]
    );
}

#[test]
fn record_result_rejects_task_not_triggered_by_automation() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    for id in ["a", "b"] {
        engine
            .register(
                Automation {
                    automation_id: AutomationId::from(id),
                    trigger: AutomationTrigger::Interval { secs: 10 },
                    action: AutomationAction::Prompt { prompt: "p".into() },
                },
                0,
            )
            .unwrap();
    }

    let a = AutomationId::from("a");
    let b = AutomationId::from("b");
    let a_task = engine.fire(&a, 10).unwrap().task_id;
    let b_task = engine.fire(&b, 10).unwrap().task_id;
    let event_count = engine.events().len();

    let wrong_owner = engine
        .record_result(
            &a,
            &b_task,
            ArtifactId::from("wrong-owner"),
            None,
            InboxStatus::Failed,
            11,
        )
        .unwrap_err();
    assert!(matches!(
        wrong_owner,
        AutomationError::TaskNotTriggeredByAutomation {
            automation_id,
            task_id,
        } if automation_id == a && task_id == b_task
    ));

    let unknown_task = BackgroundTaskId::from("not-triggered");
    let unknown = engine
        .record_result(
            &a,
            &unknown_task,
            ArtifactId::from("unknown-task"),
            None,
            InboxStatus::Succeeded,
            12,
        )
        .unwrap_err();
    assert!(matches!(
        unknown,
        AutomationError::TaskNotTriggeredByAutomation {
            automation_id,
            task_id,
        } if automation_id == a && task_id == unknown_task
    ));

    assert!(
        engine.inbox_items().is_empty(),
        "rejected results are not archived"
    );
    assert_eq!(
        engine.events().len(),
        event_count,
        "rejection emits no event"
    );

    engine
        .record_result(
            &a,
            &a_task,
            ArtifactId::from("valid"),
            None,
            InboxStatus::Succeeded,
            13,
        )
        .unwrap();
    assert_eq!(engine.inbox_items().len(), 1);
}

/// 连续失败达阈值发 Suspended 暂停并告警（不静默吞错）。
#[test]
fn consecutive_failures_suspend_and_alert() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    let id = AutomationId::from("flaky");
    engine
        .register(
            Automation {
                automation_id: id.clone(),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();

    let fail = |engine: &AutomationEngine, n: u64| {
        let outcome = engine.fire(&id, n).unwrap();
        engine
            .record_result(
                &id,
                &outcome.task_id,
                ArtifactId::from(format!("art{n}")),
                None,
                InboxStatus::Failed,
                n + 1,
            )
            .unwrap()
    };

    let e1 = fail(&engine, 10);
    let e2 = fail(&engine, 20);
    let e3 = fail(&engine, 30);

    assert!(e1
        .iter()
        .all(|e| matches!(e, AutomationEvent::ResultArchived { .. })));
    assert!(e2
        .iter()
        .all(|e| matches!(e, AutomationEvent::ResultArchived { .. })));
    assert!(e3.iter().any(
        |e| matches!(e, AutomationEvent::Suspended { automation_id, .. } if automation_id == &id)
    ));

    // 挂起后 fire 报 Suspended；check_due 也不会返回它。
    assert!(matches!(
        engine.fire(&id, 41),
        Err(AutomationError::Suspended(_, _))
    ));
}

#[test]
fn record_result_is_idempotent_for_same_task() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig {
            failure_threshold: 2,
        },
    );
    let id = AutomationId::from("once");
    engine
        .register(
            Automation {
                automation_id: id.clone(),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();
    let task_id = engine.fire(&id, 10).unwrap().task_id;
    let first = engine
        .record_result(
            &id,
            &task_id,
            ArtifactId::from("art-a"),
            None,
            InboxStatus::Failed,
            11,
        )
        .unwrap();
    assert_eq!(first.len(), 1);
    assert!(matches!(
        first[0],
        AutomationEvent::ResultArchived { .. }
    ));
    let archived_len = engine.events().iter().filter(|e| {
        matches!(e, AutomationEvent::ResultArchived { .. })
    }).count();
    let second = engine
        .record_result(
            &id,
            &task_id,
            ArtifactId::from("art-b"),
            None,
            InboxStatus::Failed,
            12,
        )
        .unwrap();
    assert!(second.is_empty());
    let archived_len_after = engine.events().iter().filter(|e| {
        matches!(e, AutomationEvent::ResultArchived { .. })
    }).count();
    assert_eq!(archived_len, archived_len_after);
    assert!(!engine.state().is_suspended(&id));
}

/// 触发与结果为 AutomationEvent，经 AgentEvent::Automation 包装可持久化，且可重放。
#[test]
fn events_round_trip_via_agent_event_and_replay() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    let id = AutomationId::from("rep");
    engine
        .register(
            Automation {
                automation_id: id.clone(),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();

    let outcome = engine.fire(&id, 10).unwrap();
    let events = vec![
        AutomationEvent::Registered {
            automation_id: id.clone(),
            trigger: AutomationTriggerKind::Interval,
        },
        AutomationEvent::Triggered {
            automation_id: id.clone(),
            task_id: outcome.task_id.clone(),
        },
        AutomationEvent::ResultArchived {
            automation_id: id.clone(),
            artifact_id: ArtifactId::from("art"),
            run_id: None,
            task_id: Some(outcome.task_id.clone()),
        },
    ];
    engine
        .record_result(
            &id,
            &outcome.task_id,
            ArtifactId::from("art"),
            None,
            InboxStatus::Succeeded,
            11,
        )
        .unwrap();

    // 每个事件都能经 AgentEvent::Automation JSON 往返（可持久化）。
    for event in &events {
        let wrapped = AgentEvent::Automation(event.clone());
        let json = serde_json::to_string(&wrapped).expect("serialize");
        let decoded: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, wrapped);
    }

    // engine 内部事件流与手工重放一致。
    let live = engine.state();
    let replayed = replay(engine.events().iter());
    assert_eq!(live.fired_count(&id), replayed.fired_count(&id));
    assert_eq!(live.archived().len(), replayed.archived().len());
    assert_eq!(live.event_log().len(), replayed.event_log().len());
}

/// event 触发器只消费 canonical 载荷字符串：外部来源（adapter 认证后）与本地
/// 构造的载荷匹配行为一致，engine core 无平台分支。
#[test]
fn event_trigger_matches_canonical_payload_only() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    engine
        .register(
            Automation {
                automation_id: AutomationId::from("ext"),
                trigger: AutomationTrigger::Event {
                    pattern: "push.*main".into(),
                },
                action: AutomationAction::Prompt {
                    prompt: "ci".into(),
                },
            },
            0,
        )
        .unwrap();

    // 相同 canonical 载荷（无论来源）匹配一致；不同载荷不匹配。
    let payload = "push to main branch".to_string();
    assert_eq!(
        engine.match_event(&payload),
        vec![AutomationId::from("ext")]
    );
    assert!(engine.match_event("push to staging").is_empty());
}

/// 触发计数唯一来自 canonical 状态：snapshot 与事件折叠保持一致，重放不漂移。
#[test]
fn fired_count_is_sourced_from_canonical_state() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    let id = AutomationId::from("cnt");
    engine
        .register(
            Automation {
                automation_id: id.clone(),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();

    engine.fire(&id, 10).unwrap();
    engine.fire(&id, 20).unwrap();

    let snap = engine.automation_snapshot(&id).unwrap();
    assert_eq!(snap.fired_count, 2);
    assert_eq!(engine.state().fired_count(&id), 2);

    // 重放重建的 canonical 计数与实时一致（不存在第二份计数源）。
    let replayed = replay(engine.events().iter());
    assert_eq!(replayed.fired_count(&id), snap.fired_count);

    // resume（重新 Registered）只清挂起，不清触发计数。
    engine.resume(&id, 30).unwrap();
    assert_eq!(engine.state().fired_count(&id), 2);
    assert_eq!(engine.automation_snapshot(&id).unwrap().fired_count, 2);
}

/// 非法 cron 表达式与非法 event 正则在注册时被拒绝。
#[test]
fn invalid_trigger_config_rejected_at_register() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    let bad_cron = engine.register(
        Automation {
            automation_id: AutomationId::from("bad_cron"),
            trigger: AutomationTrigger::Cron {
                expr: "99 * * * *".into(),
            },
            action: AutomationAction::Prompt { prompt: "p".into() },
        },
        0,
    );
    assert!(matches!(bad_cron, Err(AutomationError::InvalidCron { .. })));

    let bad_regex = engine.register(
        Automation {
            automation_id: AutomationId::from("bad_re"),
            trigger: AutomationTrigger::Event {
                pattern: "[unclosed".into(),
            },
            action: AutomationAction::Prompt { prompt: "p".into() },
        },
        0,
    );
    assert!(matches!(
        bad_regex,
        Err(AutomationError::InvalidEventPattern(_))
    ));
}

/// 手动挂起停止触发；resume 后恢复（重算下次时刻）。
#[test]
fn suspend_stops_and_resume_re_enables() {
    let engine = AutomationEngine::new(
        Box::new(common::RecordingDispatcher::new()),
        EngineConfig::default(),
    );
    let id = AutomationId::from("s");
    engine
        .register(
            Automation {
                automation_id: id.clone(),
                trigger: AutomationTrigger::Interval { secs: 10 },
                action: AutomationAction::Prompt { prompt: "p".into() },
            },
            0,
        )
        .unwrap();

    engine.suspend(&id, "manual pause".into()).unwrap();
    assert!(engine.check_due(10_000).is_empty());

    engine.resume(&id, 100).unwrap();
    let snap = engine.automation_snapshot(&id).unwrap();
    assert_eq!(snap.next_at, Some(110));
    assert!(!snap.suspended);
}
