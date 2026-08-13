//! P17-6 durable Team 事件流集成测试：
//! - 命令先落盘 SQLite，drain 队列收到 typed `AppEvent::TeamEvent` 镜像
//!   （经唯一 EventHub 的前置 limiter，由 EventPump 发布）；
//! - 重启（同一 DB 路径重建 AppService）后状态完整重放，序列从末尾继续；
//! - 持久化失败不改变状态（persist-first）。

use std::sync::Arc;

use agent_domain::{AgentId, TenantId};
use app_service::AppService;
use core_api::AppEvent;
use orchestration::{TaskId, WorkerState};
use teams::{
    MemberRole, PeerPolicy, Recipients, TeamEventEnvelope, TeamEventStore, TeamId, TeamStoreError,
};

#[test]
fn team_events_persist_replay_across_restart_and_mirror_typed_events() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("teams.sqlite");
    let team = TeamId::from("team-1");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");

    // 第一次「进程」：命令全部落盘，drain 输出 typed 镜像。
    {
        let service = AppService::with_team_db("inst-1", &db).expect("open team DB");
        let teams = service.teams();
        teams
            .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
            .unwrap();
        teams
            .add_member(&team, &sup, &w1, MemberRole::Worker)
            .unwrap();
        teams
            .post_task(&team, &sup, TaskId::new("t"), "t".into(), vec![], 0)
            .unwrap();
        teams.claim_task(&team, &w1, TaskId::new("t")).unwrap();
        teams
            .observe_worker_state(&team, &w1, WorkerState::Running)
            .unwrap();

        let drained = service.drain_events();
        let team_mirrors: Vec<_> = drained
            .iter()
            .filter(|e| matches!(e.payload, AppEvent::TeamEvent { .. }))
            .collect();
        assert_eq!(team_mirrors.len(), 5);
        assert!(team_mirrors.iter().all(|e| e.stream_sequence >= 1));
        // 镜像按投递顺序 Global 流连续。
        for window in team_mirrors.windows(2) {
            assert_eq!(window[1].stream_sequence, window[0].stream_sequence + 1);
        }
        assert_eq!(
            team_mirrors.first().unwrap().event_id.as_str(),
            "team-1-evt-1"
        );
        match &team_mirrors.last().unwrap().payload {
            AppEvent::TeamEvent { event } => assert_eq!(event.kind(), "presence_changed"),
            other => panic!("expected team mirror, got {other:?}"),
        }
    }

    // 重启：同一路径重放，状态完整、可继续追加。
    {
        let service = AppService::with_team_db("inst-1", &db).expect("reopen team DB");
        let snap = service.teams().snapshot(&team).expect("replayed team");
        assert_eq!(snap.members.len(), 2);
        assert_eq!(
            snap.board.get(&TaskId::new("t")).unwrap().owner,
            Some(w1.clone())
        );
        assert_eq!(
            snap.presence.get(&w1),
            Some(&teams::Presence::Busy),
            "presence 重放恢复"
        );

        service
            .teams()
            .post_message(
                &team,
                &w1,
                Recipients::Direct {
                    members: vec![sup.clone()],
                },
                "after restart".into(),
            )
            .unwrap();
        let drained = service.drain_events();
        let mirrors: Vec<_> = drained
            .iter()
            .filter(|e| matches!(e.payload, AppEvent::TeamEvent { .. }))
            .collect();
        assert_eq!(mirrors.len(), 1, "重放不重复镜像旧事件");
        match &mirrors[0].payload {
            AppEvent::TeamEvent { event } => assert_eq!(event.kind(), "mailbox_posted"),
            other => panic!("expected mailbox_posted mirror, got {other:?}"),
        }
        // store 中的序列从 6 继续（5 条旧事件 + 1 条新事件）。
        let replayed = service.team_host().store().replay().unwrap();
        assert_eq!(replayed.len(), 6);
        assert_eq!(
            replayed
                .iter()
                .map(|e| e.sequence.value())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
    }
}

/// 注入失败的 store：验证持久化失败时状态不变、事件不进 drain 队列。
struct FailingStore(Arc<dyn TeamEventStore>);

impl TeamEventStore for FailingStore {
    fn append(&self, _envelope: &TeamEventEnvelope) -> Result<(), TeamStoreError> {
        Err(TeamStoreError::Store("injected failure".into()))
    }

    fn replay(&self) -> Result<Vec<TeamEventEnvelope>, TeamStoreError> {
        self.0.replay()
    }
}

#[test]
fn persist_failure_leaves_service_state_unchanged() {
    use app_service::TeamHost;
    use teams::RecordingTeamSink;

    let backing: Arc<dyn TeamEventStore> = Arc::new(teams::MemoryTeamStore::new());
    let host = TeamHost::open(
        Arc::new(FailingStore(backing)),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    )
    .unwrap();
    let team = TeamId::from("team-f");
    let sup = AgentId::from("sup");

    let error = host
        .service()
        .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap_err();
    assert!(matches!(error, teams::TeamError::Store(_)));
    assert!(host.service().snapshot(&team).is_none());
    assert!(
        host.store().replay().unwrap().is_empty(),
        "失败不得产生任何持久化事实"
    );
}
