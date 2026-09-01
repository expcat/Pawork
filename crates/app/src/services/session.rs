//! Session 领域服务：会话生命周期、workspace 绑定、事件序列与 resume 语义。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, ApprovalDecision, ContentPart, ErrorCategory, ErrorContext,
    Message, MessageId, MessageRole, SessionId, TextContent, ToolResultContent, WorkspaceId,
};
use pawork_engine::EngineError;
use pawork_storage::session::SessionRecord;

use crate::{AppCore, AppError};

pub(crate) struct SessionService {
    /// 进程内 session → workspace 绑定缓存。生产创建路径（ADR-043）与
    /// session/main 分支同事务落盘后更新本缓存；启动时从存储全量替换。
    /// 无绑定的历史 session 进 Unassigned。
    pub(crate) workspaces: Mutex<HashMap<String, WorkspaceId>>,
}

/// 启动清扫时对悬空 tool call 的诚实收语文案。
pub(crate) const INTERRUPTED_TOOL_RESULT_MESSAGE: &str = "run interrupted before completion";
/// 启动清扫时对无终态 run 的诚实收口文案。
pub(crate) const INTERRUPTED_RUN_FAILED_MESSAGE: &str =
    "host process ended before the run reached a terminal state";

impl SessionService {
    pub(crate) fn new() -> Self {
        Self {
            workspaces: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn insert_workspace_cache(&self, session_id: &SessionId, workspace_id: WorkspaceId) {
        self.workspaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(session_id.as_str().to_string(), workspace_id);
    }

    /// 启动预载（ADR-043）：以存储中全部非 NULL 绑定原子替换缓存。
    pub(crate) fn replace_workspace_cache(&self, bindings: Vec<(SessionId, WorkspaceId)>) {
        let mut workspaces = self
            .workspaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        workspaces.clear();
        workspaces.extend(
            bindings
                .into_iter()
                .map(|(session_id, workspace_id)| (session_id.into_inner(), workspace_id)),
        );
    }

    pub fn workspace(&self, session_id: &SessionId) -> Option<WorkspaceId> {
        self.workspaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id.as_str())
            .cloned()
    }

    pub fn workspace_for_record(&self, session_id: &str) -> Option<WorkspaceId> {
        self.workspace(&SessionId::from(session_id))
    }

    pub async fn create_session(
        &self,
        core: &AppCore,
        title: impl Into<String>,
    ) -> Result<SessionId, AppError> {
        let n = core.next_session.fetch_add(1, Ordering::Relaxed);
        let ts = pawork_engine::now_timestamp();
        let id = SessionId::from(format!("ses-{}-{n}", ts.as_unix_millis()));
        let workspace_id = core.workspace_id().clone();
        if workspace_id.as_str() != "ws-unbound" && core.workspace_by_id(&workspace_id).is_ok() {
            core.store()?
                .create_session_with_workspace(&id, title, ts, &workspace_id)
                .await?;
            self.insert_workspace_cache(&id, workspace_id);
        } else {
            core.store()?.create_session(&id, title, ts).await?;
        }
        Ok(id)
    }

    /// GUI SessionCreate：落盘会话并绑定 command 里的 workspace_id。
    pub async fn create_session_with_workspace(
        &self,
        core: &AppCore,
        title: impl Into<String>,
        workspace_id: WorkspaceId,
    ) -> Result<SessionId, AppError> {
        let n = core.next_session.fetch_add(1, Ordering::Relaxed);
        let ts = pawork_engine::now_timestamp();
        let id = SessionId::from(format!("ses-{}-{n}", ts.as_unix_millis()));
        core.store()?
            .create_session_with_workspace(&id, title, ts, &workspace_id)
            .await?;
        self.insert_workspace_cache(&id, workspace_id);
        Ok(id)
    }

    pub async fn list_sessions(&self, core: &AppCore) -> Result<Vec<SessionRecord>, AppError> {
        Ok(core.store()?.list_sessions().await?)
    }

    pub async fn get_session(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<SessionRecord, AppError> {
        Ok(core.store()?.get_session(session_id).await?)
    }

    pub async fn resume_messages(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<Vec<Message>, AppError> {
        let record = self.get_session(core, session_id).await?;
        self.seal_orphaned_approvals(core, session_id).await?;
        Ok(core
            .store()?
            .projection_snapshot_on_branch(session_id, &record.active_branch)
            .await?
            .messages)
    }

    /// GUI resume：保留 waiting_for_approval，不把孤儿审批收成 Denied。
    pub async fn resume_messages_keep_pending(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<Vec<Message>, AppError> {
        let record = self.get_session(core, session_id).await?;
        Ok(core
            .store()?
            .projection_snapshot_on_branch(session_id, &record.active_branch)
            .await?
            .messages)
    }

    /// 把中途被杀、仍停在 `waiting_for_approval` 的调用以 Denied 收口，避免 resume 后重跑。
    async fn seal_orphaned_approvals(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<(), AppError> {
        let pending: Vec<_> = core
            .store()?
            .projection_snapshot(session_id)
            .await?
            .tool_calls
            .into_iter()
            .filter(|call| call.state == "waiting_for_approval")
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let mut sequence = self.next_sequence(core, session_id).await?;
        for call in pending {
            self.resolve_waiting_tool_call(
                core,
                session_id,
                &call,
                ApprovalDecision::Denied,
                "pending approval closed on resume",
                &mut sequence,
            )
            .await?;
        }
        Ok(())
    }

    /// 启动清扫（悬空 run 诚实收口）：宿主进程在终态前结束（崩溃 / sink
    /// 失败）遗留的 `running` run 在重放侧永远悬空。装配时对所有 session
    /// 扫 projection，把仍在 `running` 的 run 收口：非 waiting 的悬空 tool
    /// call 先落 `ToolExecutionCompleted(is_error)`，再落 `RunFailed`。
    ///
    /// - `waiting_for_approval` 的 tool call 不追加 ToolExecutionCompleted
    ///   （保持 pending 可决议；pending 重建只看 tool_calls 状态，与 runs
    ///   表无关），其所属 run 仍照常落 RunFailed——run 已是崩溃孤儿，
    ///   审批决议只是后续清理；
    /// - 幂等：收口后 run 进入 `failed`、tool call 进入 `completed`，
    ///   重复清扫自然早退；中途失败下次启动收敛；
    /// - 单 session 失败只 warn 后继续，不阻断启动。
    pub(crate) async fn seal_interrupted_runs(&self, core: &AppCore) {
        let sessions = match core.store() {
            Ok(store) => store.list_sessions().await,
            Err(error) => {
                tracing::warn!(error = %error, "interrupted-run sweep skipped: store not open");
                return;
            }
        };
        let sessions = match sessions {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(error = %error, "interrupted-run sweep skipped: list sessions failed");
                return;
            }
        };
        for record in sessions {
            let session_id = SessionId::from(record.session_id.as_str());
            if let Err(error) = self
                .seal_interrupted_runs_for_session(core, &session_id)
                .await
            {
                tracing::warn!(
                    session_id = session_id.as_str(),
                    error = %error,
                    "interrupted-run sweep failed for session; continuing startup"
                );
            }
        }
    }

    async fn seal_interrupted_runs_for_session(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<(), AppError> {
        let snapshot = core.store()?.projection_snapshot(session_id).await?;
        let running: Vec<_> = snapshot
            .runs
            .iter()
            .filter(|run| run.state == "running")
            .map(|run| run.run_id.clone())
            .collect();
        if running.is_empty() {
            return Ok(());
        }
        let tool_calls: Vec<_> = snapshot.tool_calls.clone();
        let mut sequence = self.next_sequence(core, session_id).await?;
        for run_id in running {
            for call in tool_calls.iter().filter(|call| {
                call.run_id == run_id
                    && call.state != "completed"
                    && call.state != "waiting_for_approval"
            }) {
                core.append_payload(
                    session_id,
                    &run_id,
                    &mut sequence,
                    AgentEvent::ToolExecutionCompleted {
                        tool_call_id: call.tool_call_id.clone(),
                        result: ToolResultContent {
                            tool_call_id: call.tool_call_id.clone(),
                            tool_name: Some(call.name.clone()),
                            content: vec![ContentPart::Text(TextContent {
                                text: INTERRUPTED_TOOL_RESULT_MESSAGE.into(),
                            })],
                            is_error: true,
                            metadata: serde_json::Value::Null,
                            artifacts: Vec::new(),
                        },
                    },
                )
                .await?;
            }
            core.append_payload(
                session_id,
                &run_id,
                &mut sequence,
                AgentEvent::RunFailed {
                    error: ErrorContext {
                        category: ErrorCategory::Internal,
                        message: INTERRUPTED_RUN_FAILED_MESSAGE.into(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    },
                    usage: None,
                },
            )
            .await?;
        }
        Ok(())
    }

    /// 参数化决策与 comment：Denied/Approved 都落 Responded + ToolExecutionCompleted(is_error) + MessageCommitted，
    /// 工具一律不重跑。
    /// 返回本次落库的 envelope 序列（result 已存在时早退只含 Responded），
    /// 供宿主在 persist-first 之后补实时广播。
    pub(crate) async fn resolve_waiting_tool_call(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        call: &pawork_storage::session::ProjectedToolCall,
        decision: ApprovalDecision,
        comment: &str,
        sequence: &mut u64,
    ) -> Result<Vec<AgentEventEnvelope>, AppError> {
        let mut envelopes = Vec::new();
        envelopes.push(
            core.append_payload(
                session_id,
                &call.run_id,
                sequence,
                AgentEvent::ToolApprovalResponded {
                    tool_call_id: call.tool_call_id.clone(),
                    decision,
                    comment: Some(comment.into()),
                },
            )
            .await?,
        );
        if call.result.is_some() {
            return Ok(envelopes);
        }
        let result = ToolResultContent {
            tool_call_id: call.tool_call_id.clone(),
            tool_name: Some(call.name.clone()),
            content: vec![ContentPart::Text(TextContent {
                text: comment.into(),
            })],
            is_error: true,
            metadata: serde_json::Value::Null,
            artifacts: Vec::new(),
        };
        envelopes.push(
            core.append_payload(
                session_id,
                &call.run_id,
                sequence,
                AgentEvent::ToolExecutionCompleted {
                    tool_call_id: call.tool_call_id.clone(),
                    result: result.clone(),
                },
            )
            .await?,
        );
        let n = core.next_message.fetch_add(1, Ordering::Relaxed);
        let message = Message {
            id: MessageId::from(format!(
                "msg-{}-{n}",
                pawork_engine::now_timestamp().as_unix_millis()
            )),
            role: MessageRole::Tool,
            content: vec![ContentPart::ToolResult(result)],
            metadata: Default::default(),
        };
        envelopes.push(
            core.append_payload(
                session_id,
                &call.run_id,
                sequence,
                AgentEvent::MessageCommitted { message },
            )
            .await?,
        );
        Ok(envelopes)
    }

    pub(crate) async fn session_active_branch(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<String, AppError> {
        Ok(core.store()?.get_session(session_id).await?.active_branch)
    }

    pub async fn next_sequence(
        &self,
        core: &AppCore,
        session_id: &SessionId,
    ) -> Result<u64, AppError> {
        let tail = core.store()?.tail_events(session_id, 1).await?;
        Ok(match tail.last() {
            Some(event) => event
                .sequence
                .value()
                .checked_add(1)
                .ok_or_else(|| AppError::Engine(EngineError::sink("sequence overflow")))?,
            None => 1,
        })
    }

    /// `latest`、完整 id，或唯一前缀。多命中 fail-closed。
    pub async fn resolve_session(&self, core: &AppCore, spec: &str) -> Result<SessionId, AppError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(AppError::SessionNotFound(spec.into()));
        }
        if spec == "latest" {
            return self
                .list_sessions(core)
                .await?
                .into_iter()
                .next()
                .map(|record| SessionId::from(record.session_id))
                .ok_or_else(|| AppError::SessionNotFound("latest".into()));
        }
        let exact = SessionId::from(spec);
        if core.store()?.get_session(&exact).await.is_ok() {
            return Ok(exact);
        }
        let matches: Vec<String> = self
            .list_sessions(core)
            .await?
            .into_iter()
            .map(|record| record.session_id)
            .filter(|id| id.starts_with(spec))
            .collect();
        match matches.as_slice() {
            [only] => Ok(SessionId::from(only.as_str())),
            [] => Err(AppError::SessionNotFound(spec.into())),
            many => Err(AppError::AmbiguousSession {
                prefix: spec.into(),
                matches: many.join(", "),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, ContentPart, EventId, EventSequence, MessageId,
        MessageRole, RunId, SessionId, TextContent, WorkspaceId,
    };
    use pawork_storage::session::DEFAULT_BRANCH_ID;

    use crate::testsupport::mock_core;

    #[tokio::test]
    async fn workspace_binding_survives_restart_via_startup_preload() {
        use pawork_auth::{MemoryBackend, SecretBackend};
        use pawork_workspace::config::PaworkConfig;

        // 同一临时 data_dir 顺序开两个 AppCore：第一个实例写穿绑定并关停，
        // 第二个实例经 open_store 启动预载恢复绑定（ADR-043）。
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let backend: std::sync::Arc<dyn SecretBackend> = std::sync::Arc::new(MemoryBackend::new());

        let mut core = crate::AppCore::from_config_inner(
            PaworkConfig::default(),
            None,
            None,
            backend.clone(),
            true,
        )
        .await
        .expect("first core");
        core.attach_workspace(dir.path())
            .expect("attach workspace 1");
        core.open_store(&path).await.expect("open store 1");
        let session = core
            .create_session_with_workspace("restart-demo", WorkspaceId::from("ws-default"))
            .await
            .expect("create with workspace");
        assert_eq!(
            core.session_workspace(&session),
            Some(WorkspaceId::from("ws-default")),
            "写穿后首实例进程内缓存同步更新"
        );
        let unknown_session = SessionId::from("unknown-workspace");
        core.store()
            .expect("store 1")
            .create_session_with_workspace(
                &unknown_session,
                "unknown",
                pawork_engine::now_timestamp(),
                &WorkspaceId::from("ws-missing"),
            )
            .await
            .expect("create unknown workspace session");
        core.store()
            .expect("store 1")
            .database()
            .call({
                let session_id = session.as_str().to_string();
                move |connection| {
                    connection.execute(
                        "UPDATE sessions SET archived=1 WHERE session_id=?1",
                        [session_id],
                    )
                }
            })
            .await
            .expect("actor")
            .expect("archive");
        core.shutdown().await.expect("shutdown 1");

        let mut restarted =
            crate::AppCore::from_config_inner(PaworkConfig::default(), None, None, backend, true)
                .await
                .expect("second core");
        restarted
            .attach_workspace(dir.path())
            .expect("attach workspace 2");
        restarted.open_store(&path).await.expect("open store 2");
        assert_eq!(
            restarted.session_workspace_for_record(session.as_str()),
            Some(WorkspaceId::from("ws-default")),
            "重启后第二个实例必须从存储预载恢复归档会话的归属"
        );
        assert_eq!(
            restarted.session_workspace_for_record(unknown_session.as_str()),
            Some(WorkspaceId::from("ws-missing")),
            "尚未登记的 canonical workspace id 必须原样保留"
        );

        let empty_path = dir.path().join("empty-session.db");
        restarted
            .open_store(&empty_path)
            .await
            .expect("open empty store");
        assert_eq!(
            restarted.session_workspace_for_record(session.as_str()),
            None,
            "重复 open_store 必须清掉旧库的陈旧绑定"
        );
        restarted.shutdown().await.expect("shutdown 2");
    }

    fn committed(session: &SessionId, sequence: u64, id: &str) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{sequence}")),
            session.clone(),
            RunId::from("run-fork-resume"),
            EventSequence::new(sequence),
            pawork_engine::now_timestamp(),
            AgentEvent::MessageCommitted {
                message: pawork_domain::Message {
                    id: MessageId::from(id),
                    role: MessageRole::User,
                    content: vec![ContentPart::Text(TextContent { text: id.into() })],
                    metadata: Default::default(),
                },
            },
        )
    }

    fn envelope(session: &SessionId, sequence: u64, payload: AgentEvent) -> AgentEventEnvelope {
        AgentEventEnvelope::new(
            EventId::from(format!("event-{sequence}")),
            session.clone(),
            RunId::from("run-fork-resume"),
            EventSequence::new(sequence),
            pawork_engine::now_timestamp(),
            payload,
        )
    }

    async fn append_sweep_event(
        store: &pawork_storage::session::SessionStore,
        session: &SessionId,
        run: &RunId,
        sequence: u64,
        payload: AgentEvent,
    ) {
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from(format!("evt-sweep-{sequence}")),
                    session.clone(),
                    run.clone(),
                    EventSequence::new(sequence),
                    pawork_engine::now_timestamp(),
                    payload,
                ),
            )
            .await
            .expect("append sweep seed event");
    }

    #[tokio::test]
    async fn startup_sweep_seals_interrupted_runs_idempotently() {
        use pawork_auth::{MemoryBackend, SecretBackend};
        use pawork_workspace::config::PaworkConfig;

        // 直接经 storage 落一个「RunStarted + tool call 参数收集中、无终态」
        // 的 run，模拟宿主在终态前崩溃；装配（open_store）触发启动清扫。
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let session = SessionId::from("ses-sweep-interrupted");
        let run = RunId::from("run-sweep-interrupted");
        let tool_call_id = pawork_domain::ToolCallId::from("call-sweep-1");
        {
            let (store, _) = pawork_storage::session::SessionStore::open(&path)
                .await
                .expect("store");
            store
                .create_session(&session, "sweep", pawork_engine::now_timestamp())
                .await
                .expect("session");
            append_sweep_event(
                &store,
                &session,
                &run,
                1,
                AgentEvent::RunStarted {
                    trigger_message_id: MessageId::from("msg-sweep-1"),
                },
            )
            .await;
            append_sweep_event(
                &store,
                &session,
                &run,
                2,
                AgentEvent::ToolCallStarted {
                    tool_call_id: tool_call_id.clone(),
                    name: "read_file".into(),
                },
            )
            .await;
            append_sweep_event(
                &store,
                &session,
                &run,
                3,
                AgentEvent::ToolCallArgumentsDelta {
                    tool_call_id,
                    json_delta: "{\"path\":\"a.txt\"}".into(),
                },
            )
            .await;
        }

        let backend: std::sync::Arc<dyn SecretBackend> = std::sync::Arc::new(MemoryBackend::new());
        let mut core = crate::AppCore::from_config_inner(
            PaworkConfig::default(),
            None,
            None,
            backend.clone(),
            true,
        )
        .await
        .expect("core");
        core.attach_workspace(dir.path()).expect("attach workspace");
        core.open_store(&path).await.expect("open store sweeps");

        let store = core.store().expect("store");
        let snapshot = store.projection_snapshot(&session).await.expect("snapshot");
        assert_eq!(snapshot.runs[0].state, "failed", "{snapshot:?}");
        assert_eq!(snapshot.tool_calls[0].state, "completed", "{snapshot:?}");
        let result = snapshot.tool_calls[0]
            .result
            .as_ref()
            .expect("interrupted tool result")
            .to_string();
        assert!(
            result.contains(super::INTERRUPTED_TOOL_RESULT_MESSAGE),
            "{result}"
        );
        let events = store.replay_events(&session, 1, 100).await.expect("replay");
        let event_count = events.len();
        match &events
            .iter()
            .rev()
            .find(|event| matches!(event.payload, AgentEvent::RunFailed { .. }))
            .expect("sweep must persist RunFailed")
            .payload
        {
            AgentEvent::RunFailed { error, .. } => {
                assert_eq!(error.category, pawork_domain::ErrorCategory::Internal);
                assert_eq!(error.message, super::INTERRUPTED_RUN_FAILED_MESSAGE);
            }
            _ => unreachable!("matched RunFailed"),
        }
        core.shutdown().await.expect("shutdown");

        // 幂等：再次装配不重复收口（事件数不变，run 仍 failed）。
        let mut restarted =
            crate::AppCore::from_config_inner(PaworkConfig::default(), None, None, backend, true)
                .await
                .expect("second core");
        restarted
            .attach_workspace(dir.path())
            .expect("attach workspace 2");
        restarted.open_store(&path).await.expect("open store 2");
        let snapshot2 = restarted
            .store()
            .expect("store 2")
            .projection_snapshot(&session)
            .await
            .expect("snapshot 2");
        assert_eq!(snapshot2.runs[0].state, "failed");
        let events2 = restarted
            .store()
            .expect("store 2")
            .replay_events(&session, 1, 100)
            .await
            .expect("replay 2");
        assert_eq!(
            events2.len(),
            event_count,
            "second startup sweep must append nothing"
        );
        restarted.shutdown().await.expect("shutdown 2");
    }

    /// 落一个「RunStarted + tool call 停在 waiting_for_approval、无终态」的
    /// run，模拟宿主在审批等待中崩溃。
    async fn seed_waiting_run(path: &std::path::Path) -> SessionId {
        let session = SessionId::from("ses-sweep-waiting");
        let run = RunId::from("run-sweep-waiting");
        let tool_call_id = pawork_domain::ToolCallId::from("call-sweep-wait");
        let (store, _) = pawork_storage::session::SessionStore::open(path)
            .await
            .expect("store");
        store
            .create_session(&session, "waiting", pawork_engine::now_timestamp())
            .await
            .expect("session");
        append_sweep_event(
            &store,
            &session,
            &run,
            1,
            AgentEvent::RunStarted {
                trigger_message_id: MessageId::from("msg-sweep-wait"),
            },
        )
        .await;
        append_sweep_event(
            &store,
            &session,
            &run,
            2,
            AgentEvent::ToolCallStarted {
                tool_call_id: tool_call_id.clone(),
                name: "write_file".into(),
            },
        )
        .await;
        append_sweep_event(
            &store,
            &session,
            &run,
            3,
            AgentEvent::ToolApprovalRequested {
                tool_call_id,
                reason: "needs approval".into(),
            },
        )
        .await;
        session
    }

    async fn open_core_with_sweep(
        path: &std::path::Path,
        workspace: &std::path::Path,
    ) -> crate::AppCore {
        use pawork_auth::{MemoryBackend, SecretBackend};
        use pawork_workspace::config::PaworkConfig;

        let backend: std::sync::Arc<dyn SecretBackend> = std::sync::Arc::new(MemoryBackend::new());
        let mut core =
            crate::AppCore::from_config_inner(PaworkConfig::default(), None, None, backend, true)
                .await
                .expect("core");
        core.attach_workspace(workspace).expect("attach workspace");
        core.open_store(path).await.expect("open store");
        core
    }

    #[tokio::test]
    async fn startup_sweep_fails_waiting_run_but_keeps_call_pending() {
        // waiting_for_approval 的 tool call 保持 pending 可决议，但其所属
        // run 同样是崩溃孤儿：启动清扫必须把 run 收口为 failed。
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let session = seed_waiting_run(&path).await;
        let core = open_core_with_sweep(&path, dir.path()).await;

        let store = core.store().expect("store");
        let snapshot = store.projection_snapshot(&session).await.expect("snapshot");
        assert_eq!(
            snapshot.runs[0].state, "failed",
            "waiting run is still a crash orphan and must be sealed"
        );
        assert_eq!(
            snapshot.tool_calls[0].state, "waiting_for_approval",
            "pending approval must stay resolvable after the sweep"
        );
        let events = store.replay_events(&session, 1, 100).await.expect("replay");
        assert_eq!(
            events.len(),
            4,
            "sweep must append exactly one RunFailed for the waiting run"
        );
        match &events.last().expect("sealed").payload {
            AgentEvent::RunFailed { error, .. } => {
                assert_eq!(error.category, pawork_domain::ErrorCategory::Internal);
                assert_eq!(error.message, super::INTERRUPTED_RUN_FAILED_MESSAGE);
            }
            other => panic!("sweep must end with RunFailed: {other:?}"),
        }
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn waiting_approval_resolves_after_startup_sweep() {
        // 清扫后经 durable seal 决议 pending 审批：Responded /
        // ToolExecutionCompleted 落在 RunFailed 之后，重放与投影不炸，
        // tool 行正确闭合。
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.db");
        let session = seed_waiting_run(&path).await;
        let core = open_core_with_sweep(&path, dir.path()).await;

        let waiting = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("snapshot")
            .tool_calls
            .into_iter()
            .find(|call| call.state == "waiting_for_approval")
            .expect("waiting call survives sweep");
        let mut sequence = core.next_sequence(&session).await.expect("next sequence");
        core.resolve_waiting_tool_call(
            &session,
            &waiting,
            pawork_domain::ApprovalDecision::Denied,
            "resolved after sweep",
            &mut sequence,
        )
        .await
        .expect("resolve waiting tool call");

        let store = core.store().expect("store");
        let events = store.replay_events(&session, 1, 100).await.expect("replay");
        let run_failed_at = events
            .iter()
            .position(|event| matches!(event.payload, AgentEvent::RunFailed { .. }))
            .expect("sweep RunFailed");
        let responded_at = events
            .iter()
            .position(|event| matches!(event.payload, AgentEvent::ToolApprovalResponded { .. }))
            .expect("ToolApprovalResponded");
        let completed_at = events
            .iter()
            .position(|event| matches!(event.payload, AgentEvent::ToolExecutionCompleted { .. }))
            .expect("ToolExecutionCompleted");
        assert!(
            run_failed_at < responded_at && responded_at < completed_at,
            "approval resolution must replay after the sweep RunFailed"
        );
        let snapshot = store
            .projection_snapshot(&session)
            .await
            .expect("replayable");
        assert_eq!(snapshot.tool_calls[0].state, "completed");
        let result = snapshot.tool_calls[0]
            .result
            .as_ref()
            .expect("closed tool result")
            .to_string();
        assert!(result.contains("resolved after sweep"), "{result}");
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn resume_messages_on_fork_contains_only_ancestor_prefix() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let session = core.create_session("fork-resume").await.expect("create");
        let store = core.store().expect("store");
        store
            .append_event(DEFAULT_BRANCH_ID, committed(&session, 1, "m-1"))
            .await
            .expect("append 1");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                envelope(
                    &session,
                    2,
                    AgentEvent::CompactionCompleted {
                        summary_message_id: MessageId::from("fork-boundary"),
                        compacted_through: EventSequence::new(0),
                    },
                ),
            )
            .await
            .expect("append boundary");
        store
            .fork_from_event(&session, "experiment", &EventId::from("event-2"))
            .await
            .expect("fork");
        for sequence in 3..=4u64 {
            store
                .append_event(
                    DEFAULT_BRANCH_ID,
                    committed(&session, sequence, &format!("m-{sequence}")),
                )
                .await
                .expect("append");
        }
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch");

        let messages = core.resume_messages(&session).await.expect("resume");
        let ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["m-1"],
            "fork 后 resume 只含祖先前缀，不含 main 的 3–4"
        );
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn compact_on_fork_does_not_delete_main_messages() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let session = core.create_session("fork-compact").await.expect("create");
        let store = core.store().expect("store");
        store
            .append_event(DEFAULT_BRANCH_ID, committed(&session, 1, "m-1"))
            .await
            .expect("append 1");
        store
            .append_event(
                DEFAULT_BRANCH_ID,
                envelope(
                    &session,
                    2,
                    AgentEvent::CompactionCompleted {
                        summary_message_id: MessageId::from("fork-boundary"),
                        compacted_through: EventSequence::new(0),
                    },
                ),
            )
            .await
            .expect("append boundary");
        store
            .fork_from_event(&session, "experiment", &EventId::from("event-2"))
            .await
            .expect("fork");
        for sequence in 3..=4u64 {
            store
                .append_event(
                    DEFAULT_BRANCH_ID,
                    committed(&session, sequence, &format!("m-{sequence}")),
                )
                .await
                .expect("append main tail");
        }
        store
            .switch_branch(&session, "experiment")
            .await
            .expect("switch");
        store
            .append_event("experiment", committed(&session, 5, "m-fork"))
            .await
            .expect("fork message");
        store
            .append_event(
                "experiment",
                AgentEventEnvelope::new(
                    EventId::from("event-6"),
                    session.clone(),
                    RunId::from("run-fork-resume"),
                    EventSequence::new(6),
                    pawork_engine::now_timestamp(),
                    AgentEvent::CompactionCompleted {
                        summary_message_id: MessageId::from("m-fork"),
                        compacted_through: EventSequence::new(1),
                    },
                ),
            )
            .await
            .expect("fork compact");

        let fork_messages = core.resume_messages(&session).await.expect("resume fork");
        let fork_ids: Vec<&str> = fork_messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        assert_eq!(
            fork_ids,
            vec!["m-fork"],
            "fork 支按自身 lineage 水位折叠祖先前缀，保留摘要"
        );

        store
            .switch_branch(&session, DEFAULT_BRANCH_ID)
            .await
            .expect("switch main");
        let messages = core.resume_messages(&session).await.expect("resume main");
        let ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["m-1", "m-3", "m-4"],
            "fork 压缩后 main 中低于全局水位的消息仍在"
        );
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn unknown_resume_is_fail_closed() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let err = core
            .resolve_session("missing-session")
            .await
            .expect_err("missing");
        assert!(matches!(err, crate::AppError::SessionNotFound(_)));
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn resume_seals_orphaned_approval_as_denied() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let session = core.create_session("orphan").await.expect("create");
        let tool_call_id = pawork_domain::ToolCallId::from("call-orphan");
        let run_id = RunId::from("run-orphan");
        let ts = pawork_engine::now_timestamp();
        core.store()
            .expect("store")
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from("evt-1"),
                    session.clone(),
                    run_id.clone(),
                    EventSequence::new(1),
                    ts,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: tool_call_id.clone(),
                        name: "write_file".into(),
                    },
                ),
            )
            .await
            .expect("started");
        core.store()
            .expect("store")
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from("evt-2"),
                    session.clone(),
                    run_id,
                    EventSequence::new(2),
                    ts,
                    AgentEvent::ToolApprovalRequested {
                        tool_call_id: tool_call_id.clone(),
                        reason: "needs approval".into(),
                    },
                ),
            )
            .await
            .expect("requested");

        let waiting = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("snap");
        assert_eq!(waiting.tool_calls[0].state, "waiting_for_approval");

        let messages = core.resume_messages(&session).await.expect("resume");
        assert!(messages
            .iter()
            .any(|message| message.role == MessageRole::Tool));

        let sealed = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("sealed");
        assert_eq!(sealed.tool_calls[0].state, "completed");
        assert!(sealed.tool_calls[0].result.is_some());

        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 64)
            .await
            .expect("replay");
        let responded = replayed
            .iter()
            .find_map(|envelope| match &envelope.payload {
                AgentEvent::ToolApprovalResponded {
                    decision, comment, ..
                } => Some((decision.clone(), comment.clone())),
                _ => None,
            });
        assert_eq!(
            responded,
            Some((
                pawork_domain::ApprovalDecision::Denied,
                Some("pending approval closed on resume".into())
            ))
        );

        let again = core.resume_messages(&session).await.expect("idempotent");
        assert_eq!(again.len(), messages.len());
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn keep_pending_resume_does_not_seal_orphaned_approval() {
        let (core, _dir) = mock_core(Vec::new()).await;
        let session = core.create_session("keep").await.expect("create");
        let tool_call_id = pawork_domain::ToolCallId::from("call-keep");
        let run_id = RunId::from("run-keep");
        let ts = pawork_engine::now_timestamp();
        core.store()
            .expect("store")
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from("evt-1"),
                    session.clone(),
                    run_id.clone(),
                    EventSequence::new(1),
                    ts,
                    AgentEvent::ToolCallStarted {
                        tool_call_id: tool_call_id.clone(),
                        name: "write_file".into(),
                    },
                ),
            )
            .await
            .expect("started");
        core.store()
            .expect("store")
            .append_event(
                DEFAULT_BRANCH_ID,
                AgentEventEnvelope::new(
                    EventId::from("evt-2"),
                    session.clone(),
                    run_id,
                    EventSequence::new(2),
                    ts,
                    AgentEvent::ToolApprovalRequested {
                        tool_call_id: tool_call_id.clone(),
                        reason: "needs approval".into(),
                    },
                ),
            )
            .await
            .expect("requested");

        let messages = core
            .resume_messages_keep_pending(&session)
            .await
            .expect("keep pending");
        assert!(messages
            .iter()
            .all(|message| message.role != MessageRole::Tool));
        let waiting = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("snap");
        assert_eq!(waiting.tool_calls[0].state, "waiting_for_approval");
        core.shutdown().await.expect("shutdown");
    }
}
