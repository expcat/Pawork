// P17-6 durable 语义定向测试：
// - 重放后 `next_local_id` 从已用 msg/fanout 后缀继续，**不归零、不复用**；
// - 序列 / ID 计数器 checked 溢出显式报 `IdSpaceExhausted`（且不落盘）；
// - 重放流损坏（序列跳号）在恢复入口显式失败，不构造半初始化服务；
// - 并发命令保持序列连续、msg ID 唯一（单 Mutex 命令面 + persist-first）。

use std::collections::BTreeSet;
use std::sync::Arc;

use pawork_domain::{AgentId, EventId, TenantId, Timestamp};
use pawork_orchestration::{
    MemberRole, PeerPolicy, Recipients, RecordingTeamSink, TeamError, TeamEvent, TeamEventEnvelope,
    TeamEventSequence, TeamEventStore, TeamId, TeamService, TeamStoreError,
};

fn envelope(team_id: &str, sequence: u64, payload: TeamEvent) -> TeamEventEnvelope {
    TeamEventEnvelope::new(
        TeamId::from(team_id),
        TeamEventSequence::new(sequence),
        EventId::new(format!("{team_id}-evt-{sequence}")),
        Timestamp::from_unix_millis(1),
        payload,
    )
}

fn created(team_id: &TeamId, sup: &AgentId) -> TeamEvent {
    TeamEvent::TeamCreated {
        team_id: team_id.clone(),
        tenant_id: TenantId::from("ten"),
        supervisor: sup.clone(),
        name: "T".into(),
    }
}

fn direct(sup: &AgentId) -> Recipients {
    Recipients::Direct {
        members: vec![sup.clone()],
    }
}

#[test]
fn restart_continues_local_ids_without_reuse() {
    let store: Arc<dyn TeamEventStore> = Arc::new(pawork_orchestration::MemoryTeamStore::new());
    let team = TeamId::from("team-l");
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
        svc.post_message(&team, &w1, direct(&sup), "m1".into())
            .unwrap();
        svc.post_message(&team, &w1, direct(&sup), "m2".into())
            .unwrap();
    }

    // 本地计数源对每条事件 +1（生命周期事件也占用）：首会话两条消息为
    // msg-3 / msg-4。重启 #1：计数从已用后缀继续（msg-5），不归零复用。
    let rebuilt = TeamService::from_store(
        store.clone(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    )
    .unwrap();
    let ev = rebuilt
        .post_message(&team, &w1, direct(&sup), "m3".into())
        .unwrap();
    let TeamEvent::MailboxPosted { message_id, .. } = ev else {
        panic!("expected mailbox_posted");
    };
    assert_eq!(
        message_id.as_str(),
        "msg-5",
        "重放后 msg ID 必须继续而非归零"
    );

    // fan-out 与 msg 共享计数器：下一条为 msg-6 / fanout-6。
    let routed = rebuilt
        .route_peer_message(&team, &w1, direct(&sup), "peer".into())
        .unwrap();
    let TeamEvent::PeerMessageRouted {
        message_id,
        fan_out_id,
        ..
    } = routed
    else {
        panic!("expected peer_message_routed");
    };
    assert_eq!(message_id.as_str(), "msg-6");
    assert_eq!(fan_out_id.as_str(), "fanout-6");

    // 重启 #2：继续（msg-7），且全流 msg/fanout ID 无重复。
    let again = TeamService::from_store(
        store.clone(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    )
    .unwrap();
    let ev = again
        .post_message(&team, &w1, direct(&sup), "m5".into())
        .unwrap();
    let TeamEvent::MailboxPosted { message_id, .. } = ev else {
        panic!("expected mailbox_posted");
    };
    assert_eq!(message_id.as_str(), "msg-7");

    let replayed = store.replay().unwrap();
    let mut ids: Vec<String> = Vec::new();
    for e in &replayed {
        match &e.payload {
            TeamEvent::MailboxPosted { message_id, .. } => {
                ids.push(message_id.as_str().to_string());
            }
            TeamEvent::PeerMessageRouted {
                message_id,
                fan_out_id,
                ..
            } => {
                ids.push(message_id.as_str().to_string());
                ids.push(fan_out_id.as_str().to_string());
            }
            _ => {}
        }
    }
    let unique: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "重放前后 msg/fanout ID 全程不得复用: {ids:?}"
    );
}

#[test]
fn from_store_rejects_non_contiguous_corruption() {
    let store: Arc<dyn TeamEventStore> = Arc::new(pawork_orchestration::MemoryTeamStore::new());
    let team = TeamId::from("team-c");
    let sup = AgentId::from("sup");
    store
        .append(&envelope("team-c", 1, created(&team, &sup)))
        .unwrap();
    // 跳号（1 → 3）：append-only 不变量被破坏，恢复入口必须显式失败。
    store
        .append(&envelope(
            "team-c",
            3,
            TeamEvent::TeamDissolved {
                team_id: team.clone(),
            },
        ))
        .unwrap();
    let err = match TeamService::from_store(
        store.clone(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    ) {
        Ok(_) => panic!("corrupt stream must not reconstruct a service"),
        Err(error) => error,
    };
    assert!(matches!(
        err,
        TeamError::Store(TeamStoreError::NonContiguous {
            expected: 2,
            found: 3,
            ..
        })
    ));
}

#[test]
fn concurrent_commands_keep_sequences_contiguous_and_ids_unique() {
    let store: Arc<dyn TeamEventStore> = Arc::new(pawork_orchestration::MemoryTeamStore::new());
    let svc = Arc::new(TeamService::with_store_sink_and_policy(
        store.clone(),
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    ));
    let team = TeamId::from("team-x");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    svc.create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
        .unwrap();
    svc.add_member(&team, &sup, &w1, MemberRole::Worker)
        .unwrap();

    let threads: Vec<_> = (0..8)
        .map(|t| {
            let svc = Arc::clone(&svc);
            let team = team.clone();
            let sup = sup.clone();
            let w1 = w1.clone();
            std::thread::spawn(move || {
                for i in 0..20 {
                    svc.post_message(&team, &w1, direct(&sup), format!("{t}-{i}"))
                        .unwrap();
                }
            })
        })
        .collect();
    for handle in threads {
        handle.join().unwrap();
    }

    let replayed = store.replay().unwrap();
    let team_events: Vec<_> = replayed.iter().filter(|e| e.team_id == team).collect();
    // 2 条初始化 + 160 条并发消息 = 162 条，序列必须 1..162 连续无洞。
    let sequences: Vec<u64> = team_events.iter().map(|e| e.sequence.value()).collect();
    let expected: Vec<u64> = (1..=(team_events.len() as u64)).collect();
    assert_eq!(sequences, expected, "并发命令序列必须严格连续");
    let ids: Vec<&str> = team_events
        .iter()
        .filter_map(|e| match &e.payload {
            TeamEvent::MailboxPosted { message_id, .. } => Some(message_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 160);
    let unique: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "并发下 msg ID 不得复用: {ids:?}");
}

#[test]
fn local_id_exhaustion_is_reported_before_append() {
    let team = TeamId::from("team-e");
    let sup = AgentId::from("sup");
    let w1 = AgentId::from("w1");
    // 计数源已用到 u64::MAX → 下一条 checked 溢出必须显式报错。
    let envelopes = vec![
        envelope("team-e", 1, created(&team, &sup)),
        envelope(
            "team-e",
            2,
            TeamEvent::MemberAdded {
                team_id: team.clone(),
                agent_id: w1.clone(),
                role: MemberRole::Worker,
            },
        ),
        envelope(
            "team-e",
            3,
            TeamEvent::MailboxPosted {
                team_id: team.clone(),
                message_id: pawork_orchestration::MailboxMessageId::from("msg-18446744073709551615"),
                sender: w1.clone(),
                recipients: direct(&sup),
                body: "x".into(),
            },
        ),
    ];
    let svc = TeamService::from_envelopes(
        envelopes,
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    );
    let err = svc
        .post_message(&team, &w1, direct(&sup), "boom".into())
        .unwrap_err();
    assert!(matches!(err, TeamError::IdSpaceExhausted(_)));
}

#[test]
fn sequence_exhaustion_is_reported_before_append() {
    let team = TeamId::from("team-s");
    let sup = AgentId::from("sup");
    // 最后一条序列 u64::MAX-1 → next_sequence 饱和到 u64::MAX → 提交时
    // checked 预检溢出，事件不得落盘。
    let envelopes = vec![envelope("team-s", u64::MAX - 1, created(&team, &sup))];
    let svc = TeamService::from_envelopes(
        envelopes,
        Arc::new(RecordingTeamSink::new()),
        PeerPolicy::default(),
    );
    let err = svc
        .post_message(&team, &sup, direct(&sup), "boom".into())
        .unwrap_err();
    assert!(matches!(err, TeamError::IdSpaceExhausted(_)));
}
