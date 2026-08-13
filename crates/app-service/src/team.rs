//! Team（P17-6）生产装配：durable SQLite 事件存储 + 重启重放 + typed
//! EventHub 桥。
//!
//! # 职责
//! - [`SqliteTeamStore`]：`teams::TeamEventStore` 的 SQLite 实现（append-only，
//!   按 `(team_id, sequence)` 主键去重，全量 replay 按序返回）。teams 只定义
//!   契约，durable 后端在本模块落地，保持 teams crate 纯领域。
//! - [`TeamHost`]：装配 `TeamService` + store + 桥；构造时从 store 全量重放
//!   重建状态（重启恢复入口），恢复后继续向同一 store 追加。
//! - [`to_app_event`]：把 `teams::TeamEvent` 1:1 映射为 `core_api::TeamEvent`
//!   typed 镜像；镜像经 [`super::supervisor::AppTeamEventSink`] 推入共享
//!   limiter，由 EventPump 发布到唯一 EventHub（ADR-024），CLI / GUI watch
//!   消费同一份全局连续事件流。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use core_api::{TeamRecipients, TeamTaskState};
use orchestration::TaskState;
use rusqlite::Connection;
use teams::{
    PeerPolicy, TeamEventEnvelope, TeamEventSink, TeamEventStore, TeamService, TeamStoreError,
};

/// SQLite durable Team 事件存储（append-only，可失败）。
///
/// 表结构：`team_events(team_id, sequence, event_id, timestamp_ms,
/// parent_event_id, payload)`，主键 `(team_id, sequence)` + `event_id` 唯一
/// 索引。`append` 为单条原子 INSERT，重复追加返回
/// [`TeamStoreError::Duplicate`]；`replay` 按 `(team_id, sequence)` 升序返回。
pub struct SqliteTeamStore {
    conn: Mutex<Connection>,
}

impl SqliteTeamStore {
    /// 打开（必要时创建）指定路径的 SQLite 数据库并建表。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TeamStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                TeamStoreError::Store(format!("create dir {}: {e}", parent.display()))
            })?;
        }
        let conn = Connection::open(path)
            .map_err(|e| TeamStoreError::Store(format!("open {}: {e}", path.display())))?;
        Self::with_connection(conn)
    }

    /// 内存数据库（默认装配 / 测试：无持久路径时提供完整 SQL 语义）。
    pub fn in_memory() -> Result<Self, TeamStoreError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| TeamStoreError::Store(format!("open in-memory: {e}")))?;
        Self::with_connection(conn)
    }

    fn with_connection(conn: Connection) -> Result<Self, TeamStoreError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| TeamStoreError::Store(format!("busy timeout: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS team_events (
                team_id TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                event_id TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                parent_event_id TEXT,
                payload TEXT NOT NULL,
                PRIMARY KEY (team_id, sequence)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS team_events_event_id ON team_events(event_id);",
        )
        .map_err(|e| TeamStoreError::Store(format!("create table: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}

impl TeamEventStore for SqliteTeamStore {
    fn append(&self, envelope: &TeamEventEnvelope) -> Result<(), TeamStoreError> {
        let payload = serde_json::to_string(envelope)?;
        let conn = self.conn.lock().expect("team store poisoned");
        let result = conn.execute(
            "INSERT INTO team_events
                (team_id, sequence, event_id, timestamp_ms, parent_event_id, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                envelope.team_id.as_str(),
                envelope.sequence.value(),
                envelope.event_id.as_str(),
                envelope.timestamp.as_unix_millis(),
                envelope.parent_event_id.as_ref().map(|id| id.as_str()),
                payload,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(e)
                if e.to_string().contains("UNIQUE constraint failed")
                    || e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
            {
                Err(TeamStoreError::Duplicate(envelope.event_id.clone()))
            }
            Err(e) => Err(TeamStoreError::Store(format!("insert: {e}"))),
        }
    }

    fn append_batch(&self, envelopes: &[TeamEventEnvelope]) -> Result<(), TeamStoreError> {
        if envelopes.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().expect("team store poisoned");
        // 单事务原子追加：任一条失败（重复 / IO）即整体回滚，不留部分事件。
        // 多事件命令（mailbox 批量投递 / auto-retry 事件对 / presence 批量
        // 派生）依赖此原子性维持 persist-first（失败状态不变）。
        let tx = conn
            .transaction()
            .map_err(|e| TeamStoreError::Store(format!("begin batch tx: {e}")))?;
        for envelope in envelopes {
            let payload = serde_json::to_string(envelope)?;
            let result = tx.execute(
                "INSERT INTO team_events
                    (team_id, sequence, event_id, timestamp_ms, parent_event_id, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    envelope.team_id.as_str(),
                    envelope.sequence.value(),
                    envelope.event_id.as_str(),
                    envelope.timestamp.as_unix_millis(),
                    envelope.parent_event_id.as_ref().map(|id| id.as_str()),
                    payload,
                ],
            );
            match result {
                Ok(_) => {}
                Err(e)
                    if e.to_string().contains("UNIQUE constraint failed")
                        || e.sqlite_error_code()
                            == Some(rusqlite::ErrorCode::ConstraintViolation) =>
                {
                    return Err(TeamStoreError::Duplicate(envelope.event_id.clone()));
                }
                Err(e) => return Err(TeamStoreError::Store(format!("batch insert: {e}"))),
            }
        }
        tx.commit()
            .map_err(|e| TeamStoreError::Store(format!("commit batch tx: {e}")))?;
        Ok(())
    }

    fn replay(&self) -> Result<Vec<TeamEventEnvelope>, TeamStoreError> {
        let conn = self.conn.lock().expect("team store poisoned");
        let mut statement = conn
            .prepare(
                "SELECT team_id, sequence, event_id, timestamp_ms, parent_event_id, payload
                 FROM team_events ORDER BY team_id, sequence",
            )
            .map_err(|e| TeamStoreError::Store(format!("prepare replay: {e}")))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| TeamStoreError::Store(format!("query replay: {e}")))?;
        let mut envelopes = Vec::new();
        for row in rows {
            let (team_id, sequence, event_id, timestamp_ms, parent_event_id, payload) =
                row.map_err(|e| TeamStoreError::Store(format!("read row: {e}")))?;
            let envelope: TeamEventEnvelope = serde_json::from_str(&payload)?;
            // 与写入列对账（损坏检测）：列与信封不一致视为 store 损坏。
            if envelope.team_id.as_str() != team_id
                || envelope.sequence.value() != sequence
                || envelope.event_id.as_str() != event_id
                || envelope.timestamp.as_unix_millis() != timestamp_ms
                || envelope.parent_event_id.as_ref().map(|id| id.as_str())
                    != parent_event_id.as_deref()
            {
                return Err(TeamStoreError::Store(format!(
                    "replay row mismatch for event {event_id}"
                )));
            }
            envelopes.push(envelope);
        }
        Ok(envelopes)
    }
}

/// Team 生产装配：durable store + TeamService + EventHub 桥（重启重放）。
pub struct TeamHost {
    store: Arc<dyn TeamEventStore>,
    service: TeamService,
}

impl TeamHost {
    /// 装配并重放：从 `store` 全量重放重建 `TeamService`（重启恢复入口）。
    pub fn open(
        store: Arc<dyn TeamEventStore>,
        sink: Arc<dyn TeamEventSink>,
        peer_policy: PeerPolicy,
    ) -> Result<Self, teams::TeamError> {
        let service = TeamService::from_store(store.clone(), sink, peer_policy)?;
        Ok(Self { store, service })
    }

    /// 打开指定路径的 durable store 并重放。
    pub fn open_sqlite(path: impl Into<PathBuf>) -> Result<Self, teams::TeamError> {
        let store: Arc<dyn TeamEventStore> = Arc::new(SqliteTeamStore::open(path.into())?);
        Self::open(store, Arc::new(teams::NullTeamSink), PeerPolicy::default())
    }

    /// 命令面 / 查询面。
    pub fn service(&self) -> &TeamService {
        &self.service
    }

    /// presence 生产桥：把既有 P12 worker 生命周期事件（[`OrchestrationEvent`]
    /// 流）翻译为 team presence。
    ///
    /// 复用 `orchestration::replay_workers`（`AgentSupervisor` 同一折叠）作为
    /// worker 状态源，teams 不复制 run loop / worker 状态机，只做协作层
    /// 翻译；变化事件经 durable store 单批原子落盘并镜像到唯一 EventHub。
    pub fn observe_worker_events(
        &self,
        team_id: &teams::TeamId,
        events: &[orchestration::OrchestrationEvent],
    ) -> Result<Vec<teams::TeamEvent>, teams::TeamError> {
        self.service.observe_worker_events(team_id, events)
    }

    /// durable 事实源（测试 / 自省）。
    pub fn store(&self) -> &Arc<dyn TeamEventStore> {
        &self.store
    }
}

/// `teams::TeamEvent` → `core_api::TeamEvent` typed 镜像（1:1，边界转换）。
pub fn to_app_event(event: &teams::TeamEvent) -> core_api::TeamEvent {
    use core_api::{
        TeamBoardTask, TeamEvent, TeamMemberRole, TeamPlanCommentAnchor, TeamPlanStepSnapshot,
        TeamPlanStepStatus, TeamPresence,
    };
    match event {
        teams::TeamEvent::TeamCreated {
            team_id,
            tenant_id,
            supervisor,
            name,
        } => TeamEvent::TeamCreated {
            team_id: team_id.clone(),
            tenant_id: tenant_id.clone(),
            supervisor: supervisor.clone(),
            name: name.clone(),
        },
        teams::TeamEvent::MemberAdded {
            team_id,
            agent_id,
            role,
        } => TeamEvent::MemberAdded {
            team_id: team_id.clone(),
            agent_id: agent_id.clone(),
            role: match role {
                teams::MemberRole::Supervisor => TeamMemberRole::Supervisor,
                teams::MemberRole::Worker => TeamMemberRole::Worker,
            },
        },
        teams::TeamEvent::MemberRemoved { team_id, agent_id } => TeamEvent::MemberRemoved {
            team_id: team_id.clone(),
            agent_id: agent_id.clone(),
        },
        teams::TeamEvent::TeamDissolved { team_id } => TeamEvent::TeamDissolved {
            team_id: team_id.clone(),
        },
        teams::TeamEvent::TaskPosted { team_id, task } => TeamEvent::TaskPosted {
            team_id: team_id.clone(),
            task: TeamBoardTask {
                task_id: task.task_id.as_str().to_string(),
                poster: task.poster.clone(),
                owner: task.owner.clone(),
                description: task.description.clone(),
                depends_on: task
                    .depends_on
                    .iter()
                    .map(|id| id.as_str().to_string())
                    .collect(),
                state: task_state(&task.state),
                retry_count: task.retry_count,
                max_retries: task.max_retries,
            },
        },
        teams::TeamEvent::TaskClaimed {
            team_id,
            task_id,
            claimer,
        } => TeamEvent::TaskClaimed {
            team_id: team_id.clone(),
            task_id: task_id.as_str().to_string(),
            claimer: claimer.clone(),
        },
        teams::TeamEvent::TaskReleased {
            team_id,
            task_id,
            by,
        } => TeamEvent::TaskReleased {
            team_id: team_id.clone(),
            task_id: task_id.as_str().to_string(),
            by: by.clone(),
        },
        teams::TeamEvent::TaskAdvanced {
            team_id,
            task_id,
            state,
        } => TeamEvent::TaskAdvanced {
            team_id: team_id.clone(),
            task_id: task_id.as_str().to_string(),
            state: task_state(state),
        },
        teams::TeamEvent::MailboxPosted {
            team_id,
            message_id,
            sender,
            recipients,
            body,
        } => TeamEvent::MailboxPosted {
            team_id: team_id.clone(),
            message_id: message_id.clone(),
            sender: sender.clone(),
            recipients: recipients_mirror(recipients),
            body: body.clone(),
        },
        teams::TeamEvent::MailboxDelivered {
            team_id,
            message_id,
            recipient,
        } => TeamEvent::MailboxDelivered {
            team_id: team_id.clone(),
            message_id: message_id.clone(),
            recipient: recipient.clone(),
        },
        teams::TeamEvent::MailboxRead {
            team_id,
            message_id,
            by,
        } => TeamEvent::MailboxRead {
            team_id: team_id.clone(),
            message_id: message_id.clone(),
            by: by.clone(),
        },
        teams::TeamEvent::PresenceChanged {
            team_id,
            agent_id,
            presence,
        } => TeamEvent::PresenceChanged {
            team_id: team_id.clone(),
            agent_id: agent_id.clone(),
            presence: match presence {
                teams::Presence::Online => TeamPresence::Online,
                teams::Presence::Busy => TeamPresence::Busy,
                teams::Presence::Idle => TeamPresence::Idle,
                teams::Presence::Offline => TeamPresence::Offline,
            },
        },
        teams::TeamEvent::PeerMessageRouted {
            team_id,
            message_id,
            fan_out_id,
            sender,
            recipients,
            body,
        } => TeamEvent::PeerMessageRouted {
            team_id: team_id.clone(),
            message_id: message_id.clone(),
            fan_out_id: fan_out_id.clone(),
            sender: sender.clone(),
            recipients: recipients_mirror(recipients),
            body: body.clone(),
        },
        teams::TeamEvent::FanOutDenied {
            team_id,
            sender,
            recipients,
            reason,
        } => TeamEvent::FanOutDenied {
            team_id: team_id.clone(),
            sender: sender.clone(),
            recipients: recipients_mirror(recipients),
            reason: reason.clone(),
        },
        teams::TeamEvent::PlanSubmitted {
            team_id,
            plan_id,
            version,
            title,
            steps,
        } => TeamEvent::PlanSubmitted {
            team_id: team_id.clone(),
            plan_id: plan_id.clone(),
            version: version.clone(),
            title: title.clone(),
            steps: steps
                .iter()
                .map(|step| TeamPlanStepSnapshot {
                    step_id: step.step_id.as_str().to_string(),
                    text: step.text.clone(),
                    status: match step.status {
                        agent_domain::PlanStepStatus::Pending => TeamPlanStepStatus::Pending,
                        agent_domain::PlanStepStatus::InProgress => TeamPlanStepStatus::InProgress,
                        agent_domain::PlanStepStatus::Completed => TeamPlanStepStatus::Completed,
                        agent_domain::PlanStepStatus::Blocked => TeamPlanStepStatus::Blocked,
                    },
                })
                .collect(),
        },
        teams::TeamEvent::PlanApproved {
            team_id,
            plan_id,
            version,
            checkpoint_id,
        } => TeamEvent::PlanApproved {
            team_id: team_id.clone(),
            plan_id: plan_id.clone(),
            version: version.clone(),
            checkpoint_id: checkpoint_id.clone(),
        },
        teams::TeamEvent::PlanRejected {
            team_id,
            plan_id,
            version,
            reason,
        } => TeamEvent::PlanRejected {
            team_id: team_id.clone(),
            plan_id: plan_id.clone(),
            version: version.clone(),
            reason: reason.clone(),
        },
        teams::TeamEvent::PlanCommented {
            team_id,
            plan_id,
            version,
            anchor,
            body,
        } => TeamEvent::PlanCommented {
            team_id: team_id.clone(),
            plan_id: plan_id.clone(),
            version: version.clone(),
            anchor: TeamPlanCommentAnchor {
                step_id: anchor.step_id.as_str().to_string(),
                line_offset: anchor.line_offset,
                file: anchor.file.clone(),
                file_line: anchor.file_line,
            },
            body: body.clone(),
        },
    }
}

fn task_state(state: &TaskState) -> TeamTaskState {
    match state {
        TaskState::Created => TeamTaskState::Created,
        TaskState::Ready => TeamTaskState::Ready,
        TaskState::Assigned => TeamTaskState::Assigned,
        TaskState::Running => TeamTaskState::Running,
        TaskState::Blocked => TeamTaskState::Blocked,
        TaskState::Completed => TeamTaskState::Completed,
        TaskState::Failed => TeamTaskState::Failed,
        TaskState::Cancelled => TeamTaskState::Cancelled,
    }
}

fn recipients_mirror(recipients: &teams::Recipients) -> TeamRecipients {
    match recipients {
        teams::Recipients::Direct { members } => TeamRecipients::Direct {
            members: members.clone(),
        },
        teams::Recipients::Broadcast => TeamRecipients::Broadcast,
    }
}

/// 装配 durable TeamHost：指定路径则打开 SQLite（**失败即致命**：绝不静默
/// 降级到内存空状态），否则使用内存 SQLite（完整 SQL 语义，无跨进程持久性）。
///
/// 持久路径打开失败、或从该路径重放失败（损坏 / 序列不连续）时 fail-fast
/// panic——降级到空内存状态会**丢失已持久化事实**（重启后 team / mailbox /
/// 任务板凭空消失），因此视为启动期致命错误，由调用方决定终止或上报；
/// 不做静默降级。
pub(crate) fn open_durable(
    sink: Arc<dyn TeamEventSink>,
    db_path: Option<PathBuf>,
) -> Result<TeamHost, teams::TeamError> {
    let store: Arc<dyn TeamEventStore> = match db_path {
        Some(path) => Arc::new(SqliteTeamStore::open(path)?),
        None => Arc::new(SqliteTeamStore::in_memory()?),
    };
    TeamHost::open(store, sink, PeerPolicy::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::{
        AgentId, EventId, PlanId, PlanStepId, PlanStepSnapshot, PlanStepStatus, PlanVersionId,
        TenantId, Timestamp,
    };
    use core_api::TeamEvent;
    use orchestration::{OrchestrationEvent, TaskId, TaskState, WorkerRole, WorkerState};
    use teams::{MailboxMessageId, MemberRole, Presence, Recipients, TeamEventEnvelope};

    fn envelope(team_id: &str, sequence: u64, payload: teams::TeamEvent) -> TeamEventEnvelope {
        TeamEventEnvelope::new(
            teams::TeamId::from(team_id),
            teams::TeamEventSequence::new(sequence),
            EventId::new(format!("{team_id}-evt-{sequence}")),
            Timestamp::from_unix_millis(1),
            payload,
        )
    }

    #[test]
    fn sqlite_store_roundtrips_rejects_duplicates_and_persists_reopen() {
        let store = SqliteTeamStore::in_memory().unwrap();
        let team = teams::TeamId::from("t1");
        let created = teams::TeamEvent::TeamCreated {
            team_id: team.clone(),
            tenant_id: TenantId::from("ten"),
            supervisor: AgentId::from("sup"),
            name: "T".into(),
        };
        store.append(&envelope("t1", 1, created)).unwrap();
        store
            .append(&envelope(
                "t1",
                2,
                teams::TeamEvent::TeamDissolved {
                    team_id: team.clone(),
                },
            ))
            .unwrap();
        assert!(matches!(
            store.append(&envelope(
                "t1",
                1,
                teams::TeamEvent::TeamDissolved {
                    team_id: team.clone(),
                }
            )),
            Err(TeamStoreError::Duplicate(_))
        ));
        let replayed = store.replay().unwrap();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].sequence.value(), 1);
        assert_eq!(replayed[1].sequence.value(), 2);
        assert_eq!(replayed[1].team_id, team);
    }

    #[test]
    fn sqlite_store_reopen_restores_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams.sqlite");
        {
            let store = SqliteTeamStore::open(&path).unwrap();
            store
                .append(&envelope(
                    "t1",
                    1,
                    teams::TeamEvent::TeamCreated {
                        team_id: teams::TeamId::from("t1"),
                        tenant_id: TenantId::from("ten"),
                        supervisor: AgentId::from("sup"),
                        name: "T".into(),
                    },
                ))
                .unwrap();
        }
        // 重开同一路径：事件仍在（重启重放的事实源）。
        let reopened = SqliteTeamStore::open(&path).unwrap();
        let replayed = reopened.replay().unwrap();
        assert_eq!(replayed.len(), 1);
        assert!(matches!(
            replayed[0].payload,
            teams::TeamEvent::TeamCreated { .. }
        ));
    }

    #[test]
    fn sqlite_append_batch_is_atomic_on_conflict() {
        let store = SqliteTeamStore::in_memory().unwrap();
        let team = teams::TeamId::from("t1");
        let created = teams::TeamEvent::TeamCreated {
            team_id: team.clone(),
            tenant_id: TenantId::from("ten"),
            supervisor: AgentId::from("sup"),
            name: "T".into(),
        };
        store.append(&envelope("t1", 1, created)).unwrap();
        // 批中第 2 条与已存在事件冲突：整批回滚，不留下第 1 条。
        let batch = vec![
            envelope(
                "t1",
                2,
                teams::TeamEvent::MemberAdded {
                    team_id: team.clone(),
                    agent_id: AgentId::from("w1"),
                    role: teams::MemberRole::Worker,
                },
            ),
            envelope(
                "t1",
                1,
                teams::TeamEvent::TeamDissolved {
                    team_id: team.clone(),
                },
            ),
        ];
        assert!(matches!(
            store.append_batch(&batch),
            Err(TeamStoreError::Duplicate(_))
        ));
        let replayed = store.replay().unwrap();
        assert_eq!(replayed.len(), 1, "冲突批必须整体回滚，不得留下部分事件");
        assert_eq!(replayed[0].sequence.value(), 1);
        assert!(matches!(
            replayed[0].payload,
            teams::TeamEvent::TeamCreated { .. }
        ));

        // 无冲突批：全部落盘且顺序保持。
        store
            .append_batch(&[
                envelope(
                    "t1",
                    2,
                    teams::TeamEvent::MemberAdded {
                        team_id: team.clone(),
                        agent_id: AgentId::from("w1"),
                        role: teams::MemberRole::Worker,
                    },
                ),
                envelope(
                    "t1",
                    3,
                    teams::TeamEvent::MemberAdded {
                        team_id: team.clone(),
                        agent_id: AgentId::from("w2"),
                        role: teams::MemberRole::Worker,
                    },
                ),
            ])
            .unwrap();
        let replayed = store.replay().unwrap();
        assert_eq!(replayed.len(), 3);
        assert_eq!(
            replayed
                .iter()
                .map(|e| e.sequence.value())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn team_host_observe_worker_events_bridges_presence_from_worker_lifecycle() {
        let team = teams::TeamId::from("t1");
        let sup = AgentId::from("sup");
        let w1 = AgentId::from("w1");
        let sink = Arc::new(teams::RecordingTeamSink::new());
        let host = TeamHost::open(
            Arc::new(SqliteTeamStore::in_memory().unwrap()),
            sink.clone(),
            PeerPolicy::default(),
        )
        .unwrap();
        host.service()
            .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
            .unwrap();
        host.service()
            .add_member(&team, &sup, &w1, teams::MemberRole::Worker)
            .unwrap();
        let events = vec![
            OrchestrationEvent::WorkerCreated {
                agent_id: w1.clone(),
                tenant_id: TenantId::from("ten"),
                parent_id: Some(sup.clone()),
                role: WorkerRole::Worker,
                session_id: team.clone(),
                worktree_path: None,
                created_at_ms: 1,
            },
            OrchestrationEvent::WorkerAdmitted {
                agent_id: w1.clone(),
                at_ms: 1,
            },
            OrchestrationEvent::WorkerStarted {
                agent_id: w1.clone(),
                at_ms: 2,
            },
            OrchestrationEvent::WorkerRunning {
                agent_id: w1.clone(),
                at_ms: 3,
            },
        ];
        let emitted = host.observe_worker_events(&team, &events).unwrap();
        assert_eq!(emitted.len(), 1);
        assert!(matches!(
            emitted[0],
            teams::TeamEvent::PresenceChanged {
                ref agent_id,
                ..
            } if agent_id == &w1
        ));
        // durable：create + add_member + presence 共 3 条；镜像：sink 从
        // 装配起即收到全部 3 条，最后一条是 presence_changed。
        assert_eq!(host.store().replay().unwrap().len(), 3);
        let mirrored = sink.events();
        assert_eq!(mirrored.len(), 3);
        assert!(matches!(
            mirrored[2].payload,
            teams::TeamEvent::PresenceChanged {
                presence: teams::Presence::Busy,
                ..
            }
        ));
        assert_eq!(
            host.service().snapshot(&team).unwrap().presence.get(&w1),
            Some(&teams::Presence::Busy)
        );
    }

    #[test]
    fn to_app_event_maps_every_variant_and_kind() {
        let team = teams::TeamId::from("t1");
        let sup = AgentId::from("sup");
        let cases: Vec<teams::TeamEvent> = vec![
            teams::TeamEvent::TeamCreated {
                team_id: team.clone(),
                tenant_id: TenantId::from("ten"),
                supervisor: sup.clone(),
                name: "T".into(),
            },
            teams::TeamEvent::MemberAdded {
                team_id: team.clone(),
                agent_id: sup.clone(),
                role: MemberRole::Supervisor,
            },
            teams::TeamEvent::MemberRemoved {
                team_id: team.clone(),
                agent_id: sup.clone(),
            },
            teams::TeamEvent::TeamDissolved {
                team_id: team.clone(),
            },
            teams::TeamEvent::TaskPosted {
                team_id: team.clone(),
                task: teams::BoardTask {
                    task_id: TaskId::new("t"),
                    poster: sup.clone(),
                    owner: None,
                    description: "d".into(),
                    depends_on: vec![TaskId::new("d1")],
                    state: TaskState::Ready,
                    retry_count: 0,
                    max_retries: 2,
                },
            },
            teams::TeamEvent::TaskClaimed {
                team_id: team.clone(),
                task_id: TaskId::new("t"),
                claimer: sup.clone(),
            },
            teams::TeamEvent::TaskReleased {
                team_id: team.clone(),
                task_id: TaskId::new("t"),
                by: sup.clone(),
            },
            teams::TeamEvent::TaskAdvanced {
                team_id: team.clone(),
                task_id: TaskId::new("t"),
                state: TaskState::Running,
            },
            teams::TeamEvent::MailboxPosted {
                team_id: team.clone(),
                message_id: MailboxMessageId::from("m1"),
                sender: sup.clone(),
                recipients: Recipients::Broadcast,
                body: "hi".into(),
            },
            teams::TeamEvent::MailboxDelivered {
                team_id: team.clone(),
                message_id: MailboxMessageId::from("m1"),
                recipient: sup.clone(),
            },
            teams::TeamEvent::MailboxRead {
                team_id: team.clone(),
                message_id: MailboxMessageId::from("m1"),
                by: sup.clone(),
            },
            teams::TeamEvent::PresenceChanged {
                team_id: team.clone(),
                agent_id: sup.clone(),
                presence: Presence::Busy,
            },
            teams::TeamEvent::PeerMessageRouted {
                team_id: team.clone(),
                message_id: MailboxMessageId::from("m2"),
                fan_out_id: teams::FanOutId::from("f1"),
                sender: sup.clone(),
                recipients: Recipients::Direct {
                    members: vec![sup.clone()],
                },
                body: "peer".into(),
            },
            teams::TeamEvent::FanOutDenied {
                team_id: team.clone(),
                sender: sup.clone(),
                recipients: Recipients::Broadcast,
                reason: "policy".into(),
            },
            teams::TeamEvent::PlanSubmitted {
                team_id: team.clone(),
                plan_id: PlanId::from("p1"),
                version: PlanVersionId::from("v1"),
                title: "plan".into(),
                steps: vec![PlanStepSnapshot {
                    step_id: PlanStepId::from("s1"),
                    text: "step".into(),
                    status: PlanStepStatus::Pending,
                }],
            },
            teams::TeamEvent::PlanApproved {
                team_id: team.clone(),
                plan_id: PlanId::from("p1"),
                version: PlanVersionId::from("v1"),
                checkpoint_id: None,
            },
            teams::TeamEvent::PlanRejected {
                team_id: team.clone(),
                plan_id: PlanId::from("p1"),
                version: PlanVersionId::from("v1"),
                reason: "no".into(),
            },
            teams::TeamEvent::PlanCommented {
                team_id: team.clone(),
                plan_id: PlanId::from("p1"),
                version: PlanVersionId::from("v1"),
                anchor: agent_domain::PlanCommentAnchor {
                    step_id: PlanStepId::from("s1"),
                    line_offset: None,
                    file: None,
                    file_line: None,
                },
                body: "comment".into(),
            },
        ];
        assert_eq!(cases.len(), 18, "mirror must cover all team variants");
        for (index, source) in cases.iter().enumerate() {
            let mirror = to_app_event(source);
            assert_eq!(mirror.team_id(), &team);
            assert!(!mirror.kind().is_empty());
            // typed 镜像可 JSON 往返（协议 wire 兼容）。
            let json = serde_json::to_string(&mirror).unwrap();
            let back: TeamEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(back, mirror, "variant #{index} roundtrip");
        }
        // 抽查具体映射字段。
        let posted = cases
            .iter()
            .find_map(|e| match e {
                teams::TeamEvent::TaskPosted { .. } => Some(to_app_event(e)),
                _ => None,
            })
            .unwrap();
        let TeamEvent::TaskPosted { task, .. } = posted else {
            panic!("expected task_posted mirror");
        };
        assert_eq!(task.task_id, "t");
        assert_eq!(task.depends_on, vec!["d1".to_string()]);
        assert_eq!(task.state, core_api::TeamTaskState::Ready);
    }

    #[test]
    fn team_host_replays_from_durable_store_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams.sqlite");
        let team = teams::TeamId::from("t1");
        let sup = AgentId::from("sup");
        let sink = Arc::new(teams::RecordingTeamSink::new());
        {
            let host = TeamHost::open_sqlite(&path).unwrap();
            host.service()
                .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
                .unwrap();
            host.service()
                .add_member(&team, &sup, &AgentId::from("w1"), MemberRole::Worker)
                .unwrap();
        }
        // 重启：同一路径重放重建（注入 sink 观察镜像仅对新建事件生效）。
        let host = TeamHost::open(
            Arc::new(SqliteTeamStore::open(&path).unwrap()),
            sink.clone(),
            PeerPolicy::default(),
        )
        .unwrap();
        let snap = host.service().snapshot(&team).unwrap();
        assert_eq!(snap.members.len(), 2);
        assert!(sink.events().is_empty(), "replay 不重复镜像旧事件");
        host.service()
            .observe_worker_state(&team, &AgentId::from("w1"), WorkerState::Running)
            .unwrap();
        assert_eq!(sink.events().len(), 1);
        assert_eq!(
            sink.events()[0].sequence.value(),
            3,
            "恢复后序列从重放末尾继续"
        );
    }

    #[test]
    fn team_host_restart_continues_local_ids_without_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams.sqlite");
        let team = teams::TeamId::from("t1");
        let sup = AgentId::from("sup");
        {
            let host = TeamHost::open_sqlite(&path).unwrap();
            host.service()
                .create_team(team.clone(), TenantId::from("ten"), &sup, "T".into())
                .unwrap();
            host.service()
                .post_message(
                    &team,
                    &sup,
                    Recipients::Direct {
                        members: vec![sup.clone()],
                    },
                    "m1".into(),
                )
                .unwrap();
            host.service()
                .post_message(
                    &team,
                    &sup,
                    Recipients::Direct {
                        members: vec![sup.clone()],
                    },
                    "m2".into(),
                )
                .unwrap();
        }
        // 计数源对每条事件 +1（TeamCreated 占用 1）：两条消息为 msg-2/msg-3。
        // 重启：本地计数从已用后缀继续（msg-4），不得归零复用。
        let host = TeamHost::open_sqlite(&path).unwrap();
        let ev = host
            .service()
            .post_message(
                &team,
                &sup,
                Recipients::Direct {
                    members: vec![sup.clone()],
                },
                "m3".into(),
            )
            .unwrap();
        let teams::TeamEvent::MailboxPosted { message_id, .. } = ev else {
            panic!("expected mailbox_posted");
        };
        assert_eq!(
            message_id.as_str(),
            "msg-4",
            "重放后 msg ID 必须继续而非归零"
        );
        let replayed = host.store().replay().unwrap();
        let ids: Vec<&str> = replayed
            .iter()
            .filter_map(|e| match &e.payload {
                teams::TeamEvent::MailboxPosted { message_id, .. } => Some(message_id.as_str()),
                _ => None,
            })
            .collect();
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "msg ID 全程不得复用: {ids:?}");
    }

    #[test]
    fn sqlite_replay_rejects_corrupt_payload_row() {
        let store = SqliteTeamStore::in_memory().unwrap();
        store
            .append(&envelope(
                "t1",
                1,
                teams::TeamEvent::TeamCreated {
                    team_id: teams::TeamId::from("t1"),
                    tenant_id: TenantId::from("ten"),
                    supervisor: AgentId::from("sup"),
                    name: "T".into(),
                },
            ))
            .unwrap();
        // 直接写坏 payload 列（绕过 store API，模拟磁盘 / 外部写入损坏）。
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE team_events SET payload = 'not-json'", [])
            .unwrap();
        let err = store.replay().unwrap_err();
        assert!(
            matches!(err, TeamStoreError::Json(_)),
            "损坏 payload 必须显式报错: {err}"
        );
    }

    #[test]
    fn sqlite_replay_rejects_row_column_mismatch() {
        let store = SqliteTeamStore::in_memory().unwrap();
        store
            .append(&envelope(
                "t1",
                1,
                teams::TeamEvent::TeamCreated {
                    team_id: teams::TeamId::from("t1"),
                    tenant_id: TenantId::from("ten"),
                    supervisor: AgentId::from("sup"),
                    name: "T".into(),
                },
            ))
            .unwrap();
        // 列与信封不一致（改 sequence 列）→ 对账失败即损坏。
        store
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE team_events SET sequence = 99", [])
            .unwrap();
        let err = store.replay().unwrap_err();
        assert!(
            matches!(err, TeamStoreError::Store(_)),
            "行列不一致必须显式报错: {err}"
        );
    }

    #[test]
    fn open_durable_persistent_path_failure_fails_loudly() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "not a dir").unwrap();
        // parent 是普通文件 → 建目录失败 → 必须 fail-fast，不得降级内存。
        let path = blocker.join("teams.sqlite");
        let result = open_durable(Arc::new(teams::RecordingTeamSink::new()), Some(path));
        assert!(
            result.is_err(),
            "持久路径打开失败必须 fail-fast，不得静默降级丢事实"
        );
    }

    #[test]
    fn open_durable_replay_corruption_fails_loudly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams.sqlite");
        {
            let store = SqliteTeamStore::open(&path).unwrap();
            store
                .append(&envelope(
                    "t1",
                    1,
                    teams::TeamEvent::TeamCreated {
                        team_id: teams::TeamId::from("t1"),
                        tenant_id: TenantId::from("ten"),
                        supervisor: AgentId::from("sup"),
                        name: "T".into(),
                    },
                ))
                .unwrap();
            // 跳号 → 重放损坏 → 重启必须 fail-fast，不得以空状态启动。
            store
                .append(&envelope(
                    "t1",
                    3,
                    teams::TeamEvent::TeamDissolved {
                        team_id: teams::TeamId::from("t1"),
                    },
                ))
                .unwrap();
        }
        let result = open_durable(Arc::new(teams::RecordingTeamSink::new()), Some(path));
        assert!(
            result.is_err(),
            "重放损坏必须 fail-fast，不得静默降级丢事实"
        );
    }
}
