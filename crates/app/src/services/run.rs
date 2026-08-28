//! Run 领域服务：事件化单轮执行（persist-first 双写）与追补事件入口。

use std::sync::atomic::Ordering;

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, CancellationToken, ContentPart, DegradeEvent, EventId,
    EventSequence, Message, MessageId, MessageRole, ModelResponseSummary, RequestId, RunId,
    SessionId, TextContent,
};
use pawork_engine::{
    assemble_request, assemble_request_with_tools, run_manual_compaction, run_session,
    AgentEventSink, EngineError, SessionTurn, DEFAULT_MAX_TOOL_ROUNDS,
};
use pawork_policy::PolicyEngine;

use crate::loop_ctx::SessionLoopCtx;
use crate::persist::PersistThenRender;
use crate::{AppCore, AppError};

pub(crate) struct RunService;

impl RunService {
    /// resume / 计划 / 审批收口共用的追补事件入口（persist-first）。
    /// 返回构造并落库的 envelope，供调用方在持久化成功后补广播。
    pub(crate) async fn append_payload(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        run_id: &RunId,
        sequence: &mut u64,
        payload: AgentEvent,
    ) -> Result<AgentEventEnvelope, AppError> {
        let value = *sequence;
        *sequence = sequence
            .checked_add(1)
            .ok_or_else(|| AppError::Engine(EngineError::sink("sequence overflow")))?;
        let envelope = AgentEventEnvelope::new(
            EventId::from(format!("evt-resume-{}-{value}", run_id.as_str())),
            session_id.clone(),
            run_id.clone(),
            EventSequence::new(value),
            pawork_engine::now_timestamp(),
            payload,
        );
        core.store()?
            .append_event(core.session_active_branch(session_id).await?, envelope.clone())
            .await?;
        Ok(envelope)
    }

    /// 事件化单轮：persist-first 双写。`messages` 最后一条必须是本轮 user。
    ///
    /// 调用方传入的 user `message_id` 会在落库前换成全局唯一 id：V1 schema 里
    /// `messages.message_id` 是跨 session 主键，CLI 进程内从 `msg-1` 起号会撞号。
    pub async fn chat_turn(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        messages: Vec<Message>,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, AppError> {
        let run_n = core.next_run.fetch_add(1, Ordering::Relaxed);
        let run_id = RunId::from(format!(
            "run-{}-{run_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        self.chat_turn_with_run_id(core, run_id, session_id, messages, render, cancel)
            .await
    }

    /// 以调用方提供的 run_id 执行一轮（GUI 需要在启动前登记取消令牌并
    /// 向客户端回报 run_id，因此 run id 的分配权上移到宿主）。
    pub async fn chat_turn_with_run_id(
        &self,
        core: &AppCore,
        run_id: RunId,
        session_id: &SessionId,
        mut messages: Vec<Message>,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, AppError> {
        let n = core.next_request.fetch_add(1, Ordering::Relaxed);
        let trigger = messages.last_mut().ok_or(AppError::EmptyTurn)?;
        if trigger.role != MessageRole::User {
            return Err(AppError::EmptyTurn);
        }
        // trigger 与 assistant/tool 消息共用 next_message 命名空间；
        // 若误用 next_request，两个计数器同从 1 起且同毫秒时会产生相同
        // message_id（messages.message_id 全局主键 → UNIQUE 冲突）。
        let message_n = core.next_message.fetch_add(1, Ordering::Relaxed);
        trigger.id = MessageId::from(format!(
            "msg-{}-{message_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        let trigger = trigger.clone();
        core.ensure_plan_allows_execution(session_id).await?;
        let mut request_messages = messages;
        if let Some(note) = crate::diff::git_status_note(&core.extensions.workspace_roots).await {
            request_messages.insert(
                0,
                Message {
                    id: MessageId::from(format!(
                        "msg-git-{}",
                        pawork_engine::now_timestamp().as_unix_millis()
                    )),
                    role: MessageRole::System,
                    content: vec![ContentPart::Text(TextContent { text: note })],
                    metadata: Default::default(),
                },
            );
        }
        let request_id = RequestId::from(format!("req-{n}"));
        let request = assemble_request_with_tools(
            request_id.clone(),
            core.model.clone(),
            request_messages,
            core.tool_defs.clone(),
        );
        let start_sequence = core.next_sequence(session_id).await?;
        let turn = SessionTurn::new(
            session_id.clone(),
            run_id.clone(),
            core.provider_id.clone(),
            core.model.clone(),
            start_sequence,
            trigger,
        );
        let sink = PersistThenRender {
            store: core.store()?,
            render,
            branch_id: core.session_active_branch(session_id).await?,
        };
        let mut turn_context = core.turn_context();
        turn_context.injected_layers = core.load_injected_layers();
        let loop_ctx = SessionLoopCtx {
            scheduler: core.scheduler.clone(),
            workspace_id: core.extensions.workspace_id.clone(),
            run_id: run_id.clone(),
            next_message: &core.next_message,
            next_request: &core.next_request,
            policy: PolicyEngine::new(core.approval.mode()),
            approval_mode: core.approval.mode(),
            workspace_trusted: core.approval.workspace_trusted(),
            descriptors: core.descriptors.clone(),
            approval_host: core.approval.host(),
            store: Some(core.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(core.session_estimator.clone()),
            checkpoints: core.checkpoints.clone(),
            workspace_roots: core.extensions.workspace_roots.clone(),
        };
        let task_id = match core.tasks_start_agent(Some(session_id)) {
            Ok(task_id) => Some(task_id),
            Err(error) => {
                tracing::warn!(error=%error, "tasks_start_agent failed; run proceeds without task ledger entry");
                None
            }
        };
        let result = run_session(
            core.provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context,
        )
        .await;
        let usage = match &result {
            Ok(summary) => Some(summary.usage.clone()),
            Err(_) => core.projected_run_usage(session_id, &run_id).await,
        };
        if let Some(usage) = usage.filter(|item| !item.is_zero()) {
            if let Err(error) = core
                .record_completed_usage(session_id, &run_id, &request_id, &usage)
                .await
            {
                tracing::warn!(error = %error, "usage ledger record failed");
            }
        }
        let finish_status = if result.is_ok() {
            pawork_domain::TaskStatus::Completed
        } else {
            pawork_domain::TaskStatus::Failed
        };
        if let Some(task_id) = &task_id {
            match core.tasks_finish(task_id, finish_status, None) {
                Ok(()) => {
                    if let Some(degrade) = core.tasks.take_last_degrade() {
                        emit_tasks_finish_degrade(core, session_id, &run_id, &sink, degrade).await;
                    }
                }
                Err(error) => {
                    let degrade = DegradeEvent::new(
                        pawork_domain::DegradeKind::TasksFinishFailed,
                        pawork_domain::DegradeSeverity::Error,
                        "tasks_finish failed",
                        serde_json::json!({
                            "task_id": task_id.as_str(),
                            "error": error.to_string(),
                        }),
                    );
                    emit_tasks_finish_degrade(core, session_id, &run_id, &sink, degrade).await;
                }
            }
        }
        Ok(result?)
    }

    /// 手动压缩（REPL /compact）：与自动链同一 engine 函数与事件序，
    /// persist-first 落 CompactionStarted / MessageCommitted(summary) /
    /// CompactionCompleted；返回重建后的消息列表。
    pub async fn compact_session(
        &self,
        core: &AppCore,
        session_id: &SessionId,
        render: &dyn AgentEventSink,
        cancel: CancellationToken,
    ) -> Result<Vec<Message>, AppError> {
        let messages = core.resume_messages(session_id).await?;
        let trigger = messages
            .last()
            .cloned()
            .ok_or(AppError::EmptyTurn)?;
        let n = core.next_request.fetch_add(1, Ordering::Relaxed);
        let request = assemble_request(
            RequestId::from(format!("req-compact-{n}")),
            core.model.clone(),
            messages,
        );
        let run_n = core.next_run.fetch_add(1, Ordering::Relaxed);
        let run_id = RunId::from(format!(
            "compact-{}-{run_n}",
            pawork_engine::now_timestamp().as_unix_millis()
        ));
        let turn = SessionTurn::new(
            session_id.clone(),
            run_id.clone(),
            core.provider_id.clone(),
            core.model.clone(),
            core.next_sequence(session_id).await?,
            trigger,
        );
        let sink = PersistThenRender {
            store: core.store()?,
            render,
            branch_id: core.session_active_branch(session_id).await?,
        };
        let loop_ctx = SessionLoopCtx {
            scheduler: core.scheduler.clone(),
            workspace_id: core.extensions.workspace_id.clone(),
            run_id,
            next_message: &core.next_message,
            next_request: &core.next_request,
            policy: PolicyEngine::new(core.approval.mode()),
            approval_mode: core.approval.mode(),
            workspace_trusted: core.approval.workspace_trusted(),
            descriptors: core.descriptors.clone(),
            approval_host: core.approval.host(),
            store: Some(core.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(core.session_estimator.clone()),
            checkpoints: core.checkpoints.clone(),
            workspace_roots: core.extensions.workspace_roots.clone(),
        };
        Ok(run_manual_compaction(
            core.provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            core.turn_context(),
        )
        .await?)
    }
}

async fn emit_tasks_finish_degrade(
    core: &AppCore,
    session_id: &SessionId,
    run_id: &RunId,
    sink: &dyn AgentEventSink,
    degrade: DegradeEvent,
) {
    let Ok(sequence) = core.next_sequence(session_id).await else {
        tracing::error!(
            code = %degrade.code(),
            "tasks_finish degrade dropped: sequence unavailable"
        );
        return;
    };
    let envelope = AgentEventEnvelope::new(
        EventId::from(format!("evt-degrade-{}-{sequence}", run_id.as_str())),
        session_id.clone(),
        run_id.clone(),
        EventSequence::new(sequence),
        pawork_engine::now_timestamp(),
        degrade.to_agent_event(),
    );
    if let Err(error) = sink.emit(envelope).await {
        tracing::error!(error = %error, code = %degrade.code(), "tasks_finish degrade emit failed");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pawork_domain::{
        AgentEvent, CancellationToken, ContentPart, MessageRole, ProviderStreamEvent, StopReason,
    };
    use pawork_storage::session::{SessionStore, DEFAULT_BRANCH_ID};
    use pawork_testkit::{MockProvider, MockScript};

    use crate::testsupport::{mock_core, RecordingEvents, user_hello};
    use crate::AppCore;

    #[tokio::test]
    async fn chat_turn_persists_and_projects_for_resume() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("hi".into()),
            ProviderStreamEvent::ThinkingDelta("think".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("hello").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");
        assert!(sink.types().contains(&"user"));
        assert!(sink.types().contains(&"assistant"));
        assert!(sink.types().contains(&"RunCompleted"));
        assert!(!sink.types().contains(&"RunFailed"));

        let messages = core.resume_messages(&session).await.expect("resume");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, MessageRole::User);
        assert_eq!(messages[1].role, MessageRole::Assistant);

        let listed = core.list_sessions().await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, session.as_str());
        assert_eq!(
            core.resolve_session("latest").await.expect("latest").as_str(),
            session.as_str()
        );

        let models = core.list_models().await.expect("models");
        assert_eq!(models[0].id.as_str(), "glm-5.2");
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn chat_turn_on_forked_branch_appends_to_active_branch() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("hi".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("fork").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("main turn");

        let parent = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 100)
            .await
            .expect("replay")
            .into_iter()
            .find(|event| {
                matches!(
                    &event.payload,
                    AgentEvent::RunCompleted { .. }
                )
            })
            .expect("run completed boundary");
        core.store()
            .expect("store")
            .fork_from_event(&session, "experiment", &parent.event_id)
            .await
            .expect("fork");
        core.store()
            .expect("store")
            .switch_branch(&session, "experiment")
            .await
            .expect("switch");

        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("fork turn");

        let store = core.store().expect("store");
        let main_starts = store
            .events_by_branch(&session, DEFAULT_BRANCH_ID, 1, 100)
            .await
            .expect("main events")
            .iter()
            .filter(|event| matches!(event.payload, AgentEvent::RunStarted { .. }))
            .count();
        let fork_starts = store
            .events_by_branch(&session, "experiment", 1, 100)
            .await
            .expect("fork events")
            .iter()
            .filter(|event| matches!(event.payload, AgentEvent::RunStarted { .. }))
            .count();
        assert_eq!(main_starts, 1, "main should keep only the first run");
        assert_eq!(fork_starts, 1, "forked branch should persist its own run");
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn two_sessions_do_not_collide_on_caller_message_ids() {
        let (core, _dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("ok".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let first = core.create_session("one").await.expect("first");
        let second = core.create_session("two").await.expect("second");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &first,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("first turn");
        core.chat_turn(
            &second,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("second turn");

        let first_messages = core.resume_messages(&first).await.expect("resume first");
        let second_messages = core.resume_messages(&second).await.expect("resume second");
        assert_eq!(first_messages.len(), 2);
        assert_eq!(second_messages.len(), 2);
        assert_ne!(
            first_messages[0].id, second_messages[0].id,
            "user message_id is a global primary key"
        );
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn secret_in_message_metadata_is_redacted_from_db() {
        let secret = "fake-api-key-that-must-not-reach-sqlite";
        let (core, dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("ok".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        let session = core.create_session("secret-test").await.expect("create");
        let mut user = user_hello();
        user.metadata
            .provider_metadata
            .insert("api_key".into(), serde_json::json!(secret));
        core.chat_turn(&session, vec![user], &RecordingEvents::default(), CancellationToken::new())
            .await
            .expect("turn");

        let path = core.store().expect("store").path().to_path_buf();
        let bytes = std::fs::read(&path).expect("read db");
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(
            !haystack.contains(secret),
            "secret leaked into session.db"
        );
        let replayed = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 64)
            .await
            .expect("replay");
        let json = serde_json::to_string(&replayed).expect("json");
        assert!(!json.contains(secret), "secret leaked into replay json");
        assert!(json.contains("[REDACTED]"));
        core.shutdown().await.expect("shutdown");
        drop(dir);
    }

    #[tokio::test]
    async fn chat_turn_executes_read_file_via_scheduler() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::write(workspace.path().join("hello.txt"), "hello-from-workspace")
            .expect("write fixture");
        let dir = tempfile::tempdir().expect("store");
        let path = dir.path().join("session.db");
        let (store, _) = SessionStore::open(&path).await.expect("store");
        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("read_file", serde_json::json!({"path": "hello.txt"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new()
                .text("the file says hello-from-workspace")
                .complete(),
        ]);
        let mut core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        core.attach_workspace(workspace.path()).expect("attach");
        let session = core.create_session("tools").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("tool loop");

        let types = sink.types();
        assert!(types.contains(&"ToolCallStarted"));
        assert!(types.contains(&"ToolExecutionStarted"));
        assert!(types.contains(&"ToolExecutionCompleted"));
        assert!(types.contains(&"RunCompleted"));
        let messages = core.resume_messages(&session).await.expect("resume");
        assert!(messages.iter().any(|message| message.role == MessageRole::Tool));
        let joined: String = messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("hello-from-workspace"),
            "expected tool output or assistant recap, got {joined}"
        );
        core.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn tasks_finish_persist_failure_emits_diagnostic_through_sink() {
        let (mut core, dir) = mock_core(vec![
            ProviderStreamEvent::TextDelta("hi".into()),
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed),
        ])
        .await;
        core.open_control_plane(dir.path()).expect("control");
        let tasks_path = dir.path().join("tasks.json");
        std::fs::remove_file(&tasks_path).ok();
        std::fs::create_dir_all(&tasks_path).expect("block persist path");
        let session = core.create_session("degrade-tasks").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn still succeeds");
        let found = sink
            .0
            .lock()
            .expect("mutex")
            .iter()
            .any(|envelope| match &envelope.payload {
                AgentEvent::Diagnostic { code, details } => {
                    code == "degrade.tasks_finish_failed"
                        && details.get("severity").and_then(|v| v.as_str()) == Some("error")
                }
                _ => false,
            });
        assert!(found, "run sink must receive tasks_finish_failed Diagnostic");
        core.shutdown().await.expect("shutdown");
    }
}
