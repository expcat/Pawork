//! Run 领域服务：事件化单轮执行（persist-first 双写）与追补事件入口。

use std::sync::atomic::{AtomicU64, Ordering};

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

/// 账本幂等键含 request_id（control-plane 按 (tenant, account, request_id,
/// upstream_attempt) 去重）：裸 `req-{n}` 在 Host 重启、计数器归零后撞键，
/// 新 run 的用量会被账本当成同一请求的重放拒收（实证见 docs/ROADMAP.md
/// §2.2，2026-09-14）。与 client `new_request_namespace` 同形态：pid +
/// 纳秒 + 进程级计数器——毫秒粒度不够，纳秒也不够（同进程双 AppCore
/// 各自计数器同从 1 起，同一时钟嘀嗒取到相同纳秒仍撞，定向测试实证），
/// 故计数器为进程级 static，同进程内不再依赖时钟分辨率为唯一性兜底。
fn run_request_id(_core: &AppCore) -> RequestId {
    // 计数器须为进程级：AppCore 实例各自的 next_request 同从 1 起，
    // 同进程双实例在同一时钟嘀嗒内取到相同纳秒仍会撞键（定向测试实证）。
    static PROCESS_NEXT: AtomicU64 = AtomicU64::new(1);
    let n = PROCESS_NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    RequestId::from(format!("req-{:x}-{nanos:x}-{n}", std::process::id()))
}

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
            .append_event(
                core.session_active_branch(session_id).await?,
                envelope.clone(),
            )
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
        self.chat_turn_with_run_id(core, run_id, session_id, messages, render, cancel, None)
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
        web_search: Option<bool>,
    ) -> Result<ModelResponseSummary, AppError> {
        let request_id = run_request_id(core);
        let trigger = messages.last_mut().ok_or(AppError::EmptyTurn)?;
        if trigger.role != MessageRole::User {
            return Err(AppError::EmptyTurn);
        }
        // trigger 与 assistant/tool 消息共用 next_message 命名空间；
        // 若误用 next_request，两个计数器同从 1 起且同毫秒时会产生相同
        // message_id（messages.message_id 全局主键 → UNIQUE 冲突）。
        let message_n = core.next_message.fetch_add(1, Ordering::Relaxed);
        trigger.id = MessageId::from(format!("msg-{}-{message_n}", run_id.as_str()));
        let trigger = trigger.clone();
        core.ensure_plan_allows_execution(session_id).await?;
        let run_workspace = core.workspace_for_session_or_unbound(session_id)?;
        let account_change = core.select_account_for_run(&cancel).await?;
        let provider = core.request_provider_snapshot().await?;
        if let Some(change) = account_change {
            let mut sequence = core.next_sequence(session_id).await?;
            let event = self.append_payload(core, session_id, &run_id, &mut sequence,
                AgentEvent::Diagnostic { code: "provider.account_selected".into(), details: serde_json::json!({
                    "provider_id": core.provider_id.as_str(), "previous_credential_id": change.previous_id,
                    "credential_id": change.account.credential_id, "reason": "quota_exhausted",
                    "method": change.account.kind.as_str(), "masked_credential": change.account.stored.masked.as_str(),
                }) }).await?;
            render.emit(event).await?;
        }
        let mut request_messages = messages;
        if let Err(error) = core
            .extensions
            .file_index
            .scan_workspace(&run_workspace)
            .await
        {
            tracing::warn!(error = %error, "file-index scan failed");
        }
        if let Some(note) = crate::diff::git_status_note(&run_workspace.roots).await {
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
        let subagents =
            crate::subagents::SubagentRun::new(core, session_id, &run_id, cancel.clone());
        let config = core.config.subagents.clone().unwrap_or_default();
        let model_rule =
            crate::subagents::rule(&config, core.provider_id.as_str(), core.model.as_str());
        let mut descriptors: Vec<_> = core
            .descriptors
            .iter()
            .filter(|d| crate::subagents::allows_tool(&model_rule, d))
            .cloned()
            .collect();
        if subagents.enabled() {
            descriptors.extend(crate::subagents::definitions());
        }
        let tool_defs = descriptors
            .iter()
            .map(|d| pawork_domain::ToolDefinition {
                name: d.name.clone(),
                description: d.description.clone(),
                input_schema: d.input_schema.clone(),
            })
            .collect();
        let request = assemble_request_with_tools(
            request_id.clone(),
            core.model.clone(),
            request_messages,
            tool_defs,
        );
        // SEARCH-1：Global `web_search = true` 时为本轮追加 Provider 服务端搜索。
        let mut request = request;
        // ADR-063：reasoning effort = RunStart 显式值 > Global `[reasoning]`
        // 模型默认；都无则不写 reasoning 字段（Provider 默认，行为同旧版）。
        let effort = core.effort().or_else(|| {
            core.config.reasoning.as_ref().and_then(|reasoning| {
                reasoning
                    .model(core.model.as_str())
                    .and_then(|entry| entry.default_effort.as_deref())
                    .and_then(pawork_domain::ReasoningEffort::from_wire_name)
            })
        });
        if let Some(effort) = effort {
            request.reasoning = Some(pawork_domain::ReasoningConfig::new(effort));
        }
        // GUI 1.22：RunStart.web_search 仅覆盖本轮；缺省沿用 Global。
        let enable_web_search = web_search.unwrap_or(core.config.web_search == Some(true));
        if enable_web_search {
            request.hosted_tools.push(pawork_domain::HostedToolRequest {
                name: "web_search".into(),
                kind: pawork_domain::ToolCapabilityTag::WebSearch,
                description: "Provider-hosted web search".into(),
                capabilities: Vec::new(),
                config: None,
            });
        }
        // VISION-1 / SEARCH-1 前置闸门：图片输入与 hosted 工具按当前模型证据
        // fail-closed（发 HTTP 前拒绝，不静默丢弃、不伪造支持）。
        // 无任何证据（未知模型）时按空证据处理：纯文本请求照常放行，
        // 带图片 / hosted 工具的请求 fail-closed。
        let evidence = core
            .registry
            .capability_evidence(core.model.as_str())
            .filter(|evidence| evidence.provider.as_ref() == Some(&core.provider_id))
            .unwrap_or_else(|| pawork_providers::registry::CapabilityEvidence {
                model: core.model.clone(),
                provider: None,
                static_declared: None,
                probe_declared: None,
                override_declared: None,
            });
        pawork_providers::negotiate::capability_gate(&evidence, &request)
            .map_err(AppError::Provider)?;
        let start_sequence = core.next_sequence(session_id).await?;
        let turn = SessionTurn::new(
            session_id.clone(),
            run_id.clone(),
            core.provider_id.clone(),
            core.model.clone(),
            start_sequence,
            trigger,
        );
        let persisted = PersistThenRender {
            store: core.store()?,
            render,
            branch_id: core.session_active_branch(session_id).await?,
        };
        let sink = crate::subagents::ParentSink {
            inner: &persisted,
            subagents: &subagents,
        };
        let mut turn_context = core.turn_context();
        turn_context.injected_layers = core.load_injected_layers_for_session(session_id).await;
        let workspace_trusted = core.workspace_trusted_for_roots(&run_workspace.roots);
        let loop_ctx = SessionLoopCtx {
            subagents: Some(&subagents),
            parent_tool_run: core.parent_tool_run.clone(),
            scheduler: std::sync::Arc::new(
                core.scheduler
                    .with_approval_snapshot(core.approval.mode(), workspace_trusted),
            ),
            workspace_id: run_workspace.id.clone(),
            run_id: run_id.clone(),
            next_message: &core.next_message,
            next_request: &core.next_request,
            policy: PolicyEngine::new(core.approval.mode()),
            approval_mode: core.approval.mode(),
            workspace_trusted,
            descriptors,
            approval_host: core.approval.host(),
            store: Some(core.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(core.session_estimator.clone()),
            checkpoints: core.checkpoints.clone(),
            workspace_roots: run_workspace.roots.clone(),
        };
        let task_id = match core.tasks_start_agent(Some(session_id)) {
            Ok(task_id) => Some(task_id),
            Err(error) => {
                tracing::warn!(error=%error, "tasks_start_agent failed; run proceeds without task ledger entry");
                None
            }
        };
        let result = run_session(
            provider.as_ref(),
            request,
            turn,
            &sink,
            cancel,
            &loop_ctx,
            DEFAULT_MAX_TOOL_ROUNDS,
            turn_context,
        )
        .await;
        if result.is_err() {
            subagents.cancel_children();
        }
        subagents.finish().await;
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
        let provider = core.request_provider_snapshot().await?;
        let run_workspace = core.workspace_for_session_or_unbound(session_id)?;
        let messages = core.resume_messages(session_id).await?;
        let trigger = messages.last().cloned().ok_or(AppError::EmptyTurn)?;
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
        let workspace_trusted = core.workspace_trusted_for_roots(&run_workspace.roots);
        let loop_ctx = SessionLoopCtx {
            subagents: None,
            parent_tool_run: core.parent_tool_run.clone(),
            scheduler: std::sync::Arc::new(
                core.scheduler
                    .with_approval_snapshot(core.approval.mode(), workspace_trusted),
            ),
            workspace_id: run_workspace.id.clone(),
            run_id,
            next_message: &core.next_message,
            next_request: &core.next_request,
            policy: PolicyEngine::new(core.approval.mode()),
            approval_mode: core.approval.mode(),
            workspace_trusted,
            descriptors: core.descriptors.clone(),
            approval_host: core.approval.host(),
            store: Some(core.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(core.session_estimator.clone()),
            checkpoints: core.checkpoints.clone(),
            workspace_roots: run_workspace.roots.clone(),
        };
        Ok(run_manual_compaction(
            provider.as_ref(),
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

    use crate::testsupport::{mock_core, user_hello, RecordingEvents};
    use crate::AppCore;

    /// 账本幂等键含 request_id：Host 重启计数器归零后，不同 run 的 id
    /// 不得相同，否则新用量被 (tenant, account, request_id, attempt)
    /// 去重拒收（2026-09-14 vfix 实证：echo run 2474/29 未入帐）。
    #[tokio::test]
    async fn run_request_id_survives_counter_reset_across_host_restarts() {
        let (core_a, _dir_a) = mock_core(vec![ProviderStreamEvent::ResponseCompleted(
            StopReason::Completed,
        )])
        .await;
        let (core_b, _dir_b) = mock_core(vec![ProviderStreamEvent::ResponseCompleted(
            StopReason::Completed,
        )])
        .await;
        let id_a = super::run_request_id(&core_a);
        let id_b = super::run_request_id(&core_b);
        // 修复前两者都是 req-1（计数器各从 1 起）必然撞键；仅毫秒也不够，
        // 本测试两个 AppCore 即在同毫秒内构造完成。
        assert_ne!(id_a.as_str(), id_b.as_str());
        for id in [id_a.as_str(), id_b.as_str()] {
            let body = id.strip_prefix("req-").expect("req- prefix");
            let parts: Vec<&str> = body.split('-').collect();
            assert_eq!(parts.len(), 3, "req-<pid>-<nanos>-<n>: {id}");
            assert!(u64::from_str_radix(parts[0], 16).is_ok());
            assert!(u128::from_str_radix(parts[1], 16).is_ok());
            assert!(parts[2].parse::<u64>().is_ok());
        }
    }

    /// SEARCH-1：web_search = true 且模型声明 WebSearch 时，请求注入
    /// hosted 工具并送达 Provider（MockProvider 调用记录断言）。
    #[tokio::test]
    async fn web_search_config_injects_hosted_tool_when_model_declares_it() {
        use pawork_domain::{ModelCapabilities, ModelId, ProviderId, ToolCapabilityTag};
        use pawork_providers::{CatalogEntry, ModelRegistry};
        use pawork_storage::session::SessionStore;

        let mut registry = ModelRegistry::builtin();
        registry
            .register(CatalogEntry {
                id: ModelId::from("mock-search"),
                provider: ProviderId::from("mock"),
                display_name: "mock-search".into(),
                context_window_tokens: 0,
                max_output_tokens: 0,
                capabilities: ModelCapabilities {
                    text: true,
                    tool_calls: true,
                    hosted_tool_tags: [ToolCapabilityTag::WebSearch].into_iter().collect(),
                    ..ModelCapabilities::default()
                },
                pricing: None,
                aliases: Vec::new(),
            })
            .expect("register mock-search");
        let provider = MockProvider::new(MockScript::new().text("ok").complete())
            .with_id(ProviderId::from("mock"));
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(&dir.path().join("session.db"))
            .await
            .expect("store");
        let mut core = AppCore::from_parts_with_protocol(
            Arc::new(provider.clone()),
            None,
            ModelId::from("mock-search"),
            ProviderId::from("mock"),
            crate::protocol::AdapterProtocol::ChatCompletions,
            Some(store),
            registry,
        );
        core.config.web_search = Some(true);
        // 主会话搜索不借用子代理模型规则的 network 权限。
        core.config
            .subagents
            .get_or_insert_with(Default::default)
            .models
            .push(pawork_workspace::config::SubagentModelConfig {
                provider_id: "mock".into(),
                model_id: "mock-search".into(),
                permissions: vec!["read".into()],
                ..Default::default()
            });

        let session = core.create_session("search").await.expect("create");
        let sink = RecordingEvents::default();
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("turn");
        let calls = provider.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0]
                .hosted_tools
                .contains(&ToolCapabilityTag::WebSearch),
            "web_search = true 须注入 hosted WebSearch：{:?}",
            calls[0].hosted_tools
        );
        assert!(!calls[0].has_image);

        let sink = RecordingEvents::default();
        core.chat_turn_with_run_id(
            pawork_domain::RunId::from("run-search-off"),
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
            Some(false),
        )
        .await
        .expect("explicit off must skip hosted search");
        let calls = provider.calls();
        assert_eq!(calls.len(), 2);
        assert!(
            !calls[1]
                .hosted_tools
                .contains(&ToolCapabilityTag::WebSearch),
            "RunStart.web_search=false 须覆盖 Global true：{:?}",
            calls[1].hosted_tools
        );
        core.config.web_search = Some(false);
        core.chat_turn_with_run_id(
            pawork_domain::RunId::from("run-search-on"),
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
            Some(true),
        )
        .await
        .expect("explicit on must override Global false");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        )
        .await
        .expect("next turn must keep the Global default");
        let calls = provider.calls();
        assert_eq!(calls.len(), 4);
        assert!(calls[2]
            .hosted_tools
            .contains(&ToolCapabilityTag::WebSearch));
        assert!(!calls[3]
            .hosted_tools
            .contains(&ToolCapabilityTag::WebSearch));
        assert_eq!(core.config.web_search, Some(false));
    }

    /// VISION-1 / SEARCH-1：模型未声明能力时 gate 在发 HTTP 前拒绝——
    /// hosted WebSearch 注入被拒、图片消息被拒，Provider 零调用。
    #[tokio::test]
    async fn capability_gate_rejects_undeclared_web_search_and_image() {
        use pawork_domain::{ImageContent, ImageSource, Message, MessageId};

        // glm-5.2 静态条目为 text_tools（无 image_input / WebSearch 声明）。
        let (mut core, _dir) = mock_core(vec![ProviderStreamEvent::ResponseCompleted(
            StopReason::Completed,
        )])
        .await;
        core.config.web_search = Some(true);
        let session = core.create_session("gate").await.expect("create");
        let sink = RecordingEvents::default();
        let error = core
            .chat_turn(
                &session,
                vec![user_hello()],
                &sink,
                CancellationToken::new(),
            )
            .await
            .err()
            .expect("undeclared web search must fail closed");
        assert!(matches!(error, crate::AppError::Provider(_)));

        core.config.web_search = None;
        let image_message = Message {
            id: MessageId::from("message-img"),
            role: MessageRole::User,
            content: vec![ContentPart::Image(ImageContent {
                source: ImageSource::Url("https://example.test/a.png".into()),
                media_type: "image/png".into(),
                alt_text: None,
            })],
            metadata: Default::default(),
        };
        let error = core
            .chat_turn(
                &session,
                vec![image_message.clone()],
                &sink,
                CancellationToken::new(),
            )
            .await
            .err()
            .expect("image without declaration must fail closed");
        assert!(matches!(error, crate::AppError::Provider(_)));

        // 同名模型在其它供应商声明图像能力，不得成为当前通道的授权。
        core.model = pawork_domain::ModelId::from("glm-5.3-flash");
        let foreign = core
            .registry
            .capability_evidence(core.model.as_str())
            .unwrap();
        assert!(foreign.merged().image_input);
        assert_ne!(foreign.provider.as_ref(), Some(&core.provider_id));
        let error = core
            .chat_turn(
                &session,
                vec![image_message],
                &sink,
                CancellationToken::new(),
            )
            .await
            .expect_err("foreign provider capabilities must not authorize an image");
        assert!(matches!(error, crate::AppError::Provider(_)));
    }

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
            core.resolve_session("latest")
                .await
                .expect("latest")
                .as_str(),
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
            .find(|event| matches!(&event.payload, AgentEvent::RunCompleted { .. }))
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
        core.chat_turn(&first, vec![user_hello()], &sink, CancellationToken::new())
            .await
            .expect("first turn");
        core.chat_turn(&second, vec![user_hello()], &sink, CancellationToken::new())
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
        core.chat_turn(
            &session,
            vec![user],
            &RecordingEvents::default(),
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let path = core.store().expect("store").path().to_path_buf();
        let bytes = std::fs::read(&path).expect("read db");
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(!haystack.contains(secret), "secret leaked into session.db");
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
        assert!(messages
            .iter()
            .any(|message| message.role == MessageRole::Tool));
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
        assert!(
            found,
            "run sink must receive tasks_finish_failed Diagnostic"
        );
        core.shutdown().await.expect("shutdown");
    }
}
