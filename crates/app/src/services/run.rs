//! Run 领域服务：事件化单轮执行（persist-first 双写）与追补事件入口。

use std::sync::atomic::{AtomicU64, Ordering};

use pawork_domain::{
    AgentEvent, AgentEventEnvelope, CancellationToken, ContentPart, DegradeEvent, EventId,
    EventSequence, Message, MessageId, MessageRole, ModelResponseSummary, RequestId, RunId,
    SessionId, TaskUsageContext, TaskUsageCost, TaskUsageCostKind, TaskUsageOperation,
    TaskUsageRecord, TaskUsageSource, TaskUsageStatus, TextContent, UsageTaskRef,
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

/// Closes the native target scope when a turn leaves before the explicit
/// terminal close. Disarm after that close so later usage accounting cannot
/// observe a scope that should already be closed.
struct CloseNativeScope<'a> {
    core: &'a AppCore,
    scope: pawork_computer_use::target::Scope,
    armed: bool,
}

impl Drop for CloseNativeScope<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.core.close_target_scope(&self.scope);
        }
    }
}

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
        self.chat_turn_with_run_id(
            core, run_id, session_id, messages, render, cancel, None, None, false,
        )
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
        goal_budget: Option<u64>,
        // CU-10 #5：本 Run 的 bind/launch 是否等待 GUI 目标审批（显式
        // opt-in 的 CLI 客户端经 GUI Host 运行时为 true）。
        wait_target_approval: bool,
    ) -> Result<ModelResponseSummary, AppError> {
        let request_id = run_request_id(core);
        let trigger = messages.last_mut().ok_or(AppError::EmptyTurn)?;
        if trigger.role != MessageRole::User {
            return Err(AppError::EmptyTurn);
        }
        // 直接启动 / 恢复会话同样受 text 闸门约束，不能只在切模型时校验。
        // 同 Provider 的目录证据优先；尚未探测时沿用模型默认声明。
        let text = core
            .registry
            .capability_evidence(core.model.as_str())
            .filter(|evidence| evidence.provider.as_ref() == Some(&core.provider_id))
            .map(|evidence| evidence.merged().text)
            .or_else(|| pawork_models::registry::default_text(core.model.as_str()))
            .unwrap_or(true);
        if !text {
            return Err(AppError::ModelNotText {
                provider: core.provider_id.to_string(),
                model: core.model.to_string(),
            });
        }
        // trigger 与 assistant/tool 消息共用 next_message 命名空间；
        // 若误用 next_request，两个计数器同从 1 起且同毫秒时会产生相同
        // message_id（messages.message_id 全局主键 → UNIQUE 冲突）。
        let message_n = core.next_message.fetch_add(1, Ordering::Relaxed);
        trigger.id = MessageId::from(format!("msg-{}-{message_n}", run_id.as_str()));
        let trigger = trigger.clone();
        core.ensure_plan_allows_execution(session_id).await?;
        // A recovered goal never silently absorbs a new manual turn after Host restart.
        // Live GUI goals reject manual RunStart at the synchronous acceptance boundary.
        if goal_budget.is_none() {
            if let Some(goal) = core.goal_snapshot(session_id).await? {
                if goal.status == pawork_domain::GoalStatus::Active {
                    core.pause_goal(
                        session_id,
                        &goal.goal_id,
                        "manual_run; explicit resume required",
                    )
                    .await?;
                }
            }
        }
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
        // Child sessions have separate usage ledgers. Until budget sharing is supported,
        // bounded goals execute on their owning Run and cannot create unbudgeted children.
        let subagents = if goal_budget.is_some() {
            subagents.without_spawning()
        } else {
            subagents
        };
        let config = core.config.subagents.clone().unwrap_or_default();
        let model_rule =
            crate::subagents::rule(&config, core.provider_id.as_str(), core.model.as_str());
        // VISION-1 / SEARCH-1 / CU-11 前置闸门共用同一份证据：图片输入与
        // hosted 工具按当前模型证据 fail-closed（发 HTTP 前拒绝，不静默
        // 丢弃、不伪造支持）。无任何证据（未知模型）时按空证据处理：
        // 纯文本请求照常放行，带图片 / hosted 工具的请求 fail-closed。
        let evidence = core
            .registry
            .capability_evidence(core.model.as_str())
            .filter(|evidence| evidence.provider.as_ref() == Some(&core.provider_id))
            .unwrap_or_else(|| pawork_models::registry::CapabilityEvidence {
                model: core.model.clone(),
                provider: None,
                static_declared: None,
                probe_declared: None,
                override_declared: None,
            });
        // CU-11：computer 截图/观测是视觉操作，结果必须作为图像回到模型。
        // 未声明 image_input 的模型不暴露 ComputerUse 工具——同时离开本轮
        // 派发允许表（loop_ctx 拒绝模型臆造的调用），文本模型在截图或连接
        // 桌面后端前被拒，后端访问次数为零；能力裁决只依赖目录证据。
        let build_descriptors = |image_input: bool| {
            let mut descriptors: Vec<_> = core
                .descriptors
                .iter()
                .filter(|d| crate::subagents::allows_tool(&model_rule, d))
                .filter(|d| {
                    image_input
                        || !d
                            .capabilities
                            .contains(&pawork_domain::ToolCapabilityTag::ComputerUse)
                })
                .cloned()
                .collect();
            if subagents.enabled() {
                descriptors.extend(crate::subagents::definitions());
            }
            descriptors
        };
        let image_input = evidence.merged().image_input;
        let mut descriptors = build_descriptors(image_input);
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
        request.max_output_tokens = goal_budget.map(|budget| {
            core.registry
                .resolve(core.model.as_str())
                .map(|entry| entry.max_output_tokens)
                .filter(|limit| *limit > 0)
                .map_or(budget, |limit| budget.min(limit))
        });
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
        if let Err(error) = pawork_models::negotiate::capability_gate(&evidence, &request) {
            if evidence.provider.is_some() {
                return Err(AppError::Provider(error));
            }
            // A startup model may exist only in this provider's remote catalog.
            // Resolve it through the same bounded discovery used when switching
            // models; never borrow a same-named model's foreign capabilities.
            let mut registry = (*core.registry).clone();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(AppError::Provider(
                    pawork_domain::ProviderError::cancelled("model discovery cancelled"),
                )),
                result = crate::provider_assembly::resolve_provider_model(
                    &mut registry, provider.as_ref(), core.credential.as_ref(),
                    &core.provider_id, core.model.as_str(), &core.config,
                ) => { result?; }
            }
            let discovered = registry
                .capability_evidence(core.model.as_str())
                .filter(|entry| entry.provider.as_ref() == Some(&core.provider_id))
                .ok_or(AppError::Provider(error))?;
            if !discovered.merged().text {
                return Err(AppError::ModelNotText {
                    provider: core.provider_id.to_string(),
                    model: core.model.to_string(),
                });
            }
            pawork_models::negotiate::capability_gate(&discovered, &request)
                .map_err(AppError::Provider)?;
            // CU-11 复审：目录发现把 image_input 从「无证据」升级为已声明
            // 时，本轮工具定义与派发允许表须按最终证据重建——启动时缺当前
            // Provider 证据的视觉模型不能因发现前的过滤丢掉 computer。
            // 发现只在空证据（image_input=false）后发生，不存在降级路径。
            if !image_input && discovered.merged().image_input {
                descriptors = build_descriptors(true);
                request.tools = descriptors
                    .iter()
                    .map(|d| pawork_domain::ToolDefinition {
                        name: d.name.clone(),
                        description: d.description.clone(),
                        input_schema: d.input_schema.clone(),
                    })
                    .collect();
            }
        }
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
            policy: PolicyEngine::new(),
            approval_mode: core.approval.mode(),
            workspace_trusted,
            descriptors,
            approval_host: core.approval.host(),
            store: Some(core.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(core.session_estimator.clone()),
            checkpoints: core.checkpoints.clone(),
            workspace_roots: run_workspace.roots.clone(),
            wait_target_approval,
            native: core.native.clone(),
        };
        let task_id = match core.tasks_start_agent(&cancel) {
            Ok(task_id) => Some(task_id),
            Err(error) => {
                tracing::warn!(error=%error, "tasks_start_agent failed; run proceeds without task ledger entry");
                None
            }
        };
        let mut usage_log = TaskUsageRecord {
            id: format!("native-{}", run_id.as_str()),
            // 斜线命名空间不能签发为 Gateway client，防止外部 token 读取原生 Run。
            client: "local/pawork".into(),
            source: TaskUsageSource::NativeRun,
            context: Some(TaskUsageContext {
                task: UsageTaskRef {
                    id: format!("session:{}", session_id.as_str()),
                    title: "Pawork 会话".into(),
                },
                group: None,
                subtask: Some(UsageTaskRef {
                    id: run_id.as_str().into(),
                    title: run_id.as_str().into(),
                }),
                operation: TaskUsageOperation::Coding,
                retry_of: None,
            }),
            operation: TaskUsageOperation::Coding,
            provider: Some(core.provider_id.as_str().into()),
            model: Some(core.model.as_str().into()),
            started_at_ms: pawork_engine::now_timestamp().as_unix_millis(),
            finished_at_ms: None,
            status: TaskUsageStatus::Running,
            tokens: None,
            cost: None,
            output_images: None,
            planned_video_seconds: None,
            upstream_task_id: None,
            upstream_status: None,
            related_call_id: None,
            error_code: None,
        };
        core.usage
            .control
            .ledger
            .start_task_usage(usage_log.clone())
            .await
            .map_err(|e| AppError::ControlPlane(e.to_string()))?;
        let native_scope = pawork_computer_use::target::Scope::new(
            loop_ctx.workspace_id.as_str(),
            run_id.as_str(),
        );
        core.open_target_scope(&native_scope);
        let mut close_native_scope = CloseNativeScope {
            core,
            scope: native_scope.clone(),
            armed: true,
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
        // CU-09：Run 终态（完成/取消/失败）统一释放 isolated desktop 的
        // 占用，不把 60 秒所有权留给下一个 Run 撞 Conflict；未占用过
        // 则为 no-op。GUI 断连不取消已进入 Host 的 Run，也不走到这里。
        pawork_tools::ComputerTool::release_shared(&loop_ctx.workspace_id, &run_id);
        // CU-17：授权随 run 消亡（CU-16 契约）——目标级 grant、句柄与
        // 占用一并清理并关闭 scope（终态后到达的 approve 拒绝）；
        // GUI 断连不影响已进入 Host 的 Run 的终态清理。
        core.close_target_scope(&native_scope);
        close_native_scope.armed = false;
        let usage = match &result {
            Ok(summary) => Some(summary.usage.clone()),
            Err(_) => core.projected_run_usage(session_id, &run_id).await,
        };
        usage_log.tokens = usage.clone().filter(|u| !u.is_zero());
        usage_log.cost = usage_log
            .tokens
            .as_ref()
            .and_then(|u| core.estimate_cost_for(&core.model, u))
            .map(|value| TaskUsageCost {
                value,
                kind: TaskUsageCostKind::Estimated,
            });
        if let Some(usage) = usage.filter(|item| !item.is_zero()) {
            if let Err(error) = core
                .record_completed_usage(session_id, &run_id, &request_id, &usage)
                .await
            {
                tracing::warn!(error = %error, "usage ledger record failed");
            }
        }
        // R-05：任务终态以真实 run 终态映射——取消（GUI RunCancel /
        // tasks cancel 共用同一令牌）记 Canceled，不再谎报 Failed。
        let finish_status = if result.is_ok() {
            pawork_domain::TaskStatus::Completed
        } else if result.as_ref().is_err_and(|error| error.is_cancelled()) {
            pawork_domain::TaskStatus::Canceled
        } else {
            pawork_domain::TaskStatus::Failed
        };
        usage_log.status = match finish_status {
            pawork_domain::TaskStatus::Completed => TaskUsageStatus::Succeeded,
            pawork_domain::TaskStatus::Canceled => TaskUsageStatus::Cancelled,
            _ => TaskUsageStatus::Failed,
        };
        usage_log.finished_at_ms = Some(
            pawork_engine::now_timestamp()
                .as_unix_millis()
                .max(usage_log.started_at_ms),
        );
        usage_log.error_code = result.as_ref().err().map(|e| {
            if e.is_cancelled() {
                "cancelled"
            } else {
                "run_failed"
            }
            .into()
        });
        let journal_result = core
            .usage
            .control
            .ledger
            .finish_task_usage(usage_log)
            .await
            .map_err(|e| AppError::ControlPlane(e.to_string()));
        if let Some(task_id) = &task_id {
            match core.tasks_finish_from_run(task_id, finish_status, None) {
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
        if let Err(error) = journal_result {
            // Run 的真实终态已持久化，统计故障不能改写为另一条 RunFailed。
            tracing::warn!(error = %error, call_id = %format!("native-{}", run_id.as_str()), "task usage journal finish failed");
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
            policy: PolicyEngine::new(),
            approval_mode: core.approval.mode(),
            workspace_trusted,
            descriptors: core.descriptors.clone(),
            approval_host: core.approval.host(),
            store: Some(core.store()?),
            session_id: Some(session_id.clone()),
            token_estimator: Some(core.session_estimator.clone()),
            checkpoints: core.checkpoints.clone(),
            workspace_roots: run_workspace.roots.clone(),
            wait_target_approval: false,
            native: core.native.clone(),
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use pawork_domain::{
        AgentEvent, AgentEventEnvelope, ApprovalDecision, CancellationToken, CanonicalModelRequest,
        ContentPart, MessageRole, ModelDefinition, ModelProvider, ModelResponseSummary,
        ProviderError, ProviderErrorKind, ProviderEventSink, ProviderId, ProviderStreamEvent,
        StopReason, TokenUsage, ToolCallId,
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
        use pawork_models::{CatalogEntry, ModelRegistry};
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
        // CU-11：错误指向实际缺失（模型 + 能力名），不误报后端断开。
        let crate::AppError::Provider(provider_error) = &error else {
            panic!("image rejection must stay a provider capability error: {error:?}");
        };
        assert!(
            provider_error.message.contains("glm-5.2")
                && provider_error.message.contains("image input"),
            "error must name the model and missing capability: {}",
            provider_error.message
        );

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
        assert!(matches!(
            error,
            crate::AppError::ModelBelongsToProvider { .. }
        ));
        // CLI --model / 已恢复会话可以绕过 switch_model；不得进入 Agent loop。
        core.model = pawork_domain::ModelId::from("wan2.7-image");
        let error = core
            .chat_turn(
                &session,
                vec![user_hello()],
                &sink,
                CancellationToken::new(),
            )
            .await
            .expect_err("image generation must not run as a conversation");
        assert!(matches!(error, crate::AppError::ModelNotText { .. }));
        assert!(sink.types().is_empty());
    }

    fn vision_gate_registry(model: &str, image_input: bool) -> pawork_models::ModelRegistry {
        use pawork_domain::{ModelCapabilities, ModelId, ProviderId};
        use pawork_models::{CatalogEntry, ModelRegistry};

        let mut registry = ModelRegistry::empty();
        registry
            .register(CatalogEntry {
                id: ModelId::from(model),
                provider: ProviderId::from("mock"),
                display_name: model.into(),
                context_window_tokens: 0,
                max_output_tokens: 0,
                capabilities: ModelCapabilities {
                    text: true,
                    image_input,
                    ..ModelCapabilities::default()
                },
                pricing: None,
                aliases: Vec::new(),
            })
            .expect("register mock model");
        registry
    }

    async fn vision_gate_core(
        model: &str,
        image_input: bool,
        provider: pawork_testkit::MockProvider,
    ) -> (
        AppCore,
        tempfile::TempDir,
        std::sync::Arc<pawork_testkit::MockTool>,
    ) {
        use pawork_domain::{AgentTool, ProviderId, ToolResult};

        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let mut core = AppCore::from_parts_with_protocol(
            std::sync::Arc::new(provider),
            None,
            pawork_domain::ModelId::from(model),
            ProviderId::from("mock"),
            crate::protocol::AdapterProtocol::ChatCompletions,
            Some(store),
            vision_gate_registry(model, image_input),
        );
        core.attach_workspace(dir.path()).expect("attach");
        // 用可计数的 MockTool 顶替调度器里的 computer：门控失效时它会留下
        // 调用记录（等价于触达桌面后端）。
        let computer = std::sync::Arc::new(
            pawork_testkit::MockTool::new("computer", ToolResult::success(Vec::new()))
                .with_descriptor(pawork_tools::ComputerTool::default().descriptor()),
        );
        core.scheduler = std::sync::Arc::new(
            core.scheduler
                .with_tools([computer.clone() as std::sync::Arc<dyn AgentTool>])
                .expect("register mock computer"),
        );
        (core, dir, computer)
    }

    /// CU-11：文本模型不暴露 computer 工具；模型臆造的调用在触达调度器与
    /// 桌面后端前被拒（MockTool 零调用），模型看到明确拒绝而非静默缺口。
    #[tokio::test]
    async fn vision_gate_withholds_computer_and_never_dispatches_for_text_models() {
        use pawork_testkit::{MockProvider, MockScript};

        let provider = MockProvider::sequence(vec![
            MockScript::new()
                .tool_call("computer", serde_json::json!({"action": "screenshot"}))
                .complete_with(pawork_domain::StopReason::ToolUse),
            MockScript::new().text("tool unavailable").complete(),
        ])
        .with_id(pawork_domain::ProviderId::from("mock"));
        let (core, _dir, computer) = vision_gate_core("mock-text", false, provider.clone()).await;
        assert!(
            core.descriptors
                .iter()
                .any(|descriptor| descriptor.name == "computer_native"),
            "builtin tools must register computer_native before the vision gate"
        );
        let session = core.create_session("vision gate").await.expect("create");
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
        assert!(!calls.is_empty());
        assert!(
            calls.iter().all(|call| {
                !call
                    .tool_names
                    .iter()
                    .any(|name| name == "computer" || name == "computer_native")
            }),
            "text model must not see computer or computer_native: {:?}",
            calls[0].tool_names
        );
        assert_eq!(
            computer.calls().len(),
            0,
            "hallucinated computer call must be denied before dispatch"
        );
        let envelopes = sink.0.lock().expect("events").clone();
        let events = serde_json::to_string(&envelopes).unwrap();
        assert!(
            events.contains("this model cannot use this tool"),
            "the model must see an explicit denial: {events}"
        );
    }

    /// CU-11：声明 image_input 的模型照常获得 computer 工具，既有视觉链路
    /// 不被门控收窄。
    #[tokio::test]
    async fn vision_gate_keeps_computer_tool_for_vision_models() {
        use pawork_testkit::{MockProvider, MockScript};

        let provider = MockProvider::new(MockScript::new().text("ok").complete())
            .with_id(pawork_domain::ProviderId::from("mock"));
        let (core, _dir, _computer) = vision_gate_core("mock-vision", true, provider.clone()).await;
        let session = core.create_session("vision gate").await.expect("create");
        core.chat_turn(
            &session,
            vec![user_hello()],
            &RecordingEvents::default(),
            CancellationToken::new(),
        )
        .await
        .expect("turn");

        let calls = provider.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].tool_names.iter().any(|name| name == "computer"),
            "vision model must keep the computer tool: {:?}",
            calls[0].tool_names
        );
    }

    #[tokio::test]
    async fn startup_model_discovers_current_provider_image_capability() {
        use pawork_domain::{ImageContent, ImageSource, ModelCapabilities, ModelDefinition};

        let (mut core, _dir) = mock_core(vec![]).await;
        core.model = "glm-5.3-flash".into();
        let provider = MockProvider::new(MockScript::new().text("two crossing lines").complete())
            .with_models(vec![ModelDefinition {
                id: core.model.clone(),
                display_name: "Remote vision model".into(),
                context_window_tokens: 32_000,
                max_output_tokens: 1024,
                capabilities: ModelCapabilities {
                    text: true,
                    image_input: true,
                    ..Default::default()
                },
            }]);
        core.provider = Arc::new(provider.clone());
        core.attach_workspace(_dir.path()).expect("attach");
        let mut message = user_hello();
        message.content.push(ContentPart::Image(ImageContent {
            source: ImageSource::Url("https://example.test/drawing.png".into()),
            media_type: "image/png".into(),
            alt_text: None,
        }));
        let session = core.create_session("startup vision").await.unwrap();
        core.chat_turn(
            &session,
            vec![message],
            &RecordingEvents::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let calls = provider.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].has_image);
        assert!(
            calls[0].tool_names.iter().any(|name| name == "computer"),
            "discovered vision model must regain the computer tool this turn: {:?}",
            calls[0].tool_names
        );
        let messages = core.resume_messages(&session).await.unwrap();
        assert!(messages.last().unwrap().content.iter().any(
            |part| matches!(part, ContentPart::Text(text) if text.text == "two crossing lines")
        ));
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
        let report = core
            .usage
            .control
            .ledger
            .task_usage_report(&pawork_domain::TaskUsageQuery {
                client: Some("local/pawork".into()),
                ..Default::default()
            })
            .await
            .expect("native journal");
        assert_eq!(report.totals.records, 1);
        assert_eq!(
            report.totals.generation_calls, 0,
            "Run summaries are not individual generations"
        );
        assert_eq!(
            report.records[0].status,
            pawork_domain::TaskUsageStatus::Succeeded
        );
        assert!(
            report.records[0].tokens.is_none(),
            "no usage event is not a confirmed zero"
        );
        core.shutdown().await.expect("shutdown");
    }

    /// R-02：上游流错误 message 回显敏感文本时，经「SSE 解析 → engine →
    /// 持久化 → 投影」全链路后原文不出现；错误对象只保留白名单 type，
    /// 错误类别仍可辨认。
    #[tokio::test]
    async fn provider_stream_error_is_persisted_without_upstream_message() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let secret = "sk-FAKE-SECRET-9f8e7d";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!(
                        "data: {{\"type\":\"error\",\"error\":{{\"type\":\"rate_limit_error\",\"message\":\"slow down, key {secret} throttled\"}}}}\n\n"
                    )),
            )
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(&dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = pawork_providers::AnthropicProvider::new(
            pawork_providers::AnthropicConfig::new(server.uri()),
            None,
        )
        .expect("provider");
        let provider_id = pawork_domain::ModelProvider::id(&provider);
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("claude-3-5-sonnet"),
            provider_id,
            Some(store),
        );
        let session = core
            .create_session("stream-error-redaction")
            .await
            .expect("create");
        let sink = RecordingEvents::default();
        let error = core
            .chat_turn(
                &session,
                vec![user_hello()],
                &sink,
                CancellationToken::new(),
            )
            .await
            .expect_err("stream error must fail the turn");
        assert!(
            !format!("{error:?}").contains(secret),
            "returned error object must not carry upstream message: {error:?}"
        );

        let events = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 100)
            .await
            .expect("replay");
        let serialized = serde_json::to_string(&events).expect("serialize events");
        assert!(
            !serialized.contains(secret),
            "persisted events must not carry upstream message: {serialized}"
        );
        let run_failed = events
            .iter()
            .find_map(|envelope| match &envelope.payload {
                AgentEvent::RunFailed { error, .. } => Some(error),
                _ => None,
            })
            .expect("RunFailed persisted");
        assert_eq!(run_failed.category, pawork_domain::ErrorCategory::RateLimit);
        assert_eq!(
            run_failed.message,
            "anthropic stream error (type=rate_limit_error)"
        );

        let messages = core.resume_messages(&session).await.expect("resume");
        let projected = serde_json::to_string(&messages).expect("serialize messages");
        assert!(
            !projected.contains(secret),
            "projection must not carry upstream message: {projected}"
        );
        core.shutdown().await.expect("shutdown");
    }

    /// 上游可直接返回取消错误，任务终态必须按 Engine 结果映射。
    #[tokio::test]
    async fn provider_cancellation_sets_task_terminal_without_cancelling_input_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::new(MockScript::new().fail(
            pawork_domain::ProviderError::cancelled("upstream cancelled"),
        ));
        let core = AppCore::from_parts(
            Arc::new(provider),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core
            .create_session("provider-cancel")
            .await
            .expect("session");
        let cancel = CancellationToken::new();
        let result = core
            .chat_turn(
                &session,
                vec![user_hello()],
                &RecordingEvents::default(),
                cancel.clone(),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(
            core.tasks_list()[0].status,
            pawork_domain::TaskStatus::Canceled
        );
        let runs = core
            .store()
            .expect("store")
            .projection_snapshot(&session)
            .await
            .expect("snapshot")
            .runs;
        assert_eq!(runs[0].state, "cancelled");
        core.shutdown().await.expect("shutdown");
    }

    /// R-05 task 入口：tasks_cancel 经共享令牌停止同一阻塞 run。
    #[tokio::test]
    async fn tasks_cancel_stops_blocking_run_and_records_canceled_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let provider = MockProvider::sequence(vec![MockScript::new().wait_for_cancellation()]);
        let core = AppCore::from_parts(
            Arc::new(provider.clone()),
            None,
            pawork_domain::ModelId::from("model-1"),
            pawork_domain::ProviderId::from("mock"),
            Some(store),
        );
        let session = core.create_session("task-cancel").await.expect("session");
        let sink = RecordingEvents::default();
        let run = core.chat_turn(
            &session,
            vec![user_hello()],
            &sink,
            CancellationToken::new(),
        );
        let driver = async {
            for _ in 0..200 {
                if !provider.calls().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            assert_eq!(
                provider.calls().len(),
                1,
                "run must reach the blocking provider stream"
            );
            let task = core
                .tasks_list()
                .into_iter()
                .find(|task| task.task_kind == pawork_domain::TaskKind::Agent)
                .expect("agent task registered for the run");
            let cancelled = core
                .tasks_cancel(task.task_id.as_str())
                .expect("task cancel");
            assert!(cancelled.contains(&task.task_id));
            task.task_id
        };
        let (result, task_id) = tokio::join!(run, driver);
        assert!(result.is_err(), "cancelled run must not succeed");

        let snapshot = core.tasks_status(task_id.as_str()).expect("task status");
        assert_eq!(snapshot.status, pawork_domain::TaskStatus::Canceled);
        let finish_events = core
            .tasks
            .tasks
            .event_log()
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    pawork_domain::TaskEvent::Finished { task_id: id, .. } if *id == task_id
                )
            })
            .count();
        assert_eq!(finish_events, 1, "run 收尾不得对已收口任务再写第二份终态");

        let events = core
            .store()
            .expect("store")
            .replay_events(&session, 1, 100)
            .await
            .expect("replay");
        let cancelled = events
            .iter()
            .filter(|envelope| matches!(envelope.payload, AgentEvent::RunCancelled { .. }))
            .count();
        let failed = events
            .iter()
            .filter(|envelope| matches!(envelope.payload, AgentEvent::RunFailed { .. }))
            .count();
        assert_eq!(cancelled, 1, "run 终态为 RunCancelled: {events:?}");
        assert_eq!(failed, 0, "取消不得谎报 RunFailed: {events:?}");
        core.shutdown().await.expect("shutdown");
    }

    const NATIVE_BUNDLE: &str = "com.example.notes";
    const NATIVE_JPEG: [u8; 4] = [0xFF, 0xD8, 0xFF, 0xD9];

    struct ApproveNative;

    #[async_trait::async_trait]
    impl crate::ApprovalPromptHost for ApproveNative {
        async fn decide(
            &self,
            _ask: &crate::ApprovalAsk,
            _cancel: CancellationToken,
        ) -> pawork_domain::ApprovalDecision {
            pawork_domain::ApprovalDecision::ApprovedOnce
        }
    }

    /// request 与 bind 各放行一次，第三次（move_window）拒绝。
    struct DenyMoveWindow {
        asks: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl crate::ApprovalPromptHost for DenyMoveWindow {
        async fn decide(
            &self,
            ask: &crate::ApprovalAsk,
            _cancel: CancellationToken,
        ) -> ApprovalDecision {
            assert_eq!(ask.tool_name, "computer_native");
            match self.asks.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => ApprovalDecision::ApprovedOnce,
                2 => ApprovalDecision::Denied,
                _ => panic!("computer_native asked more than three times"),
            }
        }
    }

    #[derive(Clone)]
    struct EngineSurface(Arc<EngineState>);

    struct EngineState {
        captures: std::sync::atomic::AtomicUsize,
        moves: std::sync::atomic::AtomicUsize,
    }

    impl EngineSurface {
        fn new() -> Self {
            Self(Arc::new(EngineState {
                captures: std::sync::atomic::AtomicUsize::new(0),
                moves: std::sync::atomic::AtomicUsize::new(0),
            }))
        }

        fn captures(&self) -> usize {
            self.0.captures.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn moves(&self) -> usize {
            self.0.moves.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn native_identity() -> pawork_computer_use::target::AppIdentity {
        pawork_computer_use::target::AppIdentity {
            bundle_id: NATIVE_BUNDLE.into(),
            family: pawork_computer_use::target::AppFamily::Appkit,
        }
    }

    fn native_instance() -> pawork_computer_use::target::ProcessInstance {
        pawork_computer_use::target::ProcessInstance {
            pid: 42,
            start_token: 7,
        }
    }

    fn native_target() -> pawork_computer_use::approval::TargetIdentity {
        pawork_computer_use::approval::TargetIdentity::application(native_identity())
    }

    fn live_instance(instance: &pawork_computer_use::target::ProcessInstance) -> bool {
        instance.pid == 42 && instance.start_token == 7
    }

    impl pawork_computer_use::target::NativeProbe for EngineSurface {
        fn process_instance_alive(
            &self,
            instance: &pawork_computer_use::target::ProcessInstance,
        ) -> bool {
            live_instance(instance)
        }

        fn window_generation(
            &self,
            instance: &pawork_computer_use::target::ProcessInstance,
            window_id: u64,
        ) -> Option<u64> {
            (live_instance(instance) && window_id == 7).then_some(1)
        }

        fn tree_revision(
            &self,
            window: &pawork_computer_use::target::WindowIdentity,
        ) -> Option<u64> {
            (window.window_id == 7 && window.generation == 1).then_some(1)
        }

        fn current_origin(
            &self,
            _: &pawork_computer_use::target::WindowIdentity,
        ) -> Option<String> {
            None
        }

        fn window_size(
            &self,
            instance: &pawork_computer_use::target::ProcessInstance,
            window_id: u64,
        ) -> Option<(f64, f64)> {
            (live_instance(instance) && window_id == 7).then_some((200.0, 100.0))
        }
    }

    impl pawork_computer_use::NativeSurface for EngineSurface {
        fn activity(&self, _: &str) -> pawork_computer_use::target::ActivityProof {
            pawork_computer_use::target::ActivityProof::Idle
        }

        fn discover(&self) -> Vec<pawork_computer_use::AppSnapshot> {
            vec![pawork_computer_use::AppSnapshot {
                identity: native_identity(),
                instance: native_instance(),
                localized_name: "Notes".into(),
                window_count: 1,
            }]
        }

        fn resolve(&self, bundle_id: &str) -> Option<pawork_computer_use::target::AppIdentity> {
            bundle_id
                .eq_ignore_ascii_case(NATIVE_BUNDLE)
                .then(native_identity)
        }

        fn list_windows(
            &self,
            app: &pawork_computer_use::target::AppIdentity,
            instance: &pawork_computer_use::target::ProcessInstance,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<
            Vec<pawork_computer_use::WindowSnapshot>,
            pawork_computer_use::target::TargetError,
        > {
            if cancelled() {
                return Err(pawork_computer_use::target::TargetError::Cancelled);
            }
            if app.bundle_id.eq_ignore_ascii_case(NATIVE_BUNDLE) && live_instance(instance) {
                Ok(vec![pawork_computer_use::WindowSnapshot {
                    window: pawork_computer_use::target::WindowIdentity {
                        window_id: 7,
                        generation: 1,
                    },
                    width: 200.0,
                    height: 100.0,
                    title: None,
                }])
            } else {
                Ok(Vec::new())
            }
        }

        fn launch(
            &self,
            _: &pawork_computer_use::target::AppIdentity,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<
            pawork_computer_use::target::ProcessInstance,
            pawork_computer_use::target::TargetError,
        > {
            if cancelled() {
                return Err(pawork_computer_use::target::TargetError::Cancelled);
            }
            Ok(native_instance())
        }

        fn capture(
            &self,
            _: &pawork_computer_use::target::ValidatedWindow,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<pawork_computer_use::CapturedFrame, pawork_computer_use::target::TargetError>
        {
            self.0
                .captures
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if cancelled() {
                return Err(pawork_computer_use::target::TargetError::Cancelled);
            }
            Ok(pawork_computer_use::CapturedFrame {
                image_width: 2,
                image_height: 1,
                window_width: 200.0,
                window_height: 100.0,
                jpeg: NATIVE_JPEG.to_vec(),
            })
        }

        fn read_tree(
            &self,
            _: &pawork_computer_use::target::ValidatedWindow,
            _: &dyn Fn() -> bool,
        ) -> Result<pawork_computer_use::target::AxTreeRead, pawork_computer_use::target::TargetError>
        {
            Err(pawork_computer_use::target::TargetError::ProbeUnavailable(
                "engine test does not read a tree".into(),
            ))
        }

        fn semantic(
            &self,
            _: &pawork_computer_use::target::ValidatedElement,
            _: &[u32],
            _: &pawork_computer_use::target::SemanticAction,
            _: &dyn Fn() -> bool,
        ) -> Result<
            pawork_computer_use::target::SemanticOutcome,
            pawork_computer_use::target::TargetError,
        > {
            Err(pawork_computer_use::target::TargetError::ProbeUnavailable(
                "engine test does not dispatch semantic input".into(),
            ))
        }

        fn targeted(
            &self,
            _: &pawork_computer_use::target::ValidatedElement,
            _: &[u32],
            _: &pawork_computer_use::target::TargetedInput,
            _: &dyn Fn() -> bool,
        ) -> Result<
            pawork_computer_use::target::TargetedOutcome,
            pawork_computer_use::target::TargetError,
        > {
            Err(pawork_computer_use::target::TargetError::ProbeUnavailable(
                "engine test does not dispatch targeted input".into(),
            ))
        }

        fn move_window(
            &self,
            _: &pawork_computer_use::target::ValidatedWindowDispatch,
            _: &pawork_computer_use::target::WindowMove,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<
            pawork_computer_use::target::SemanticOutcome,
            pawork_computer_use::target::TargetError,
        > {
            if cancelled() {
                return Err(pawork_computer_use::target::TargetError::Cancelled);
            }
            self.0
                .moves
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(pawork_computer_use::target::SemanticOutcome::Dispatched)
        }
    }

    struct NativeEngineRun {
        core: Arc<tokio::sync::RwLock<AppCore>>,
        adapter: crate::gui_host::GuiHostAdapter,
        _dir: tempfile::TempDir,
        surface: EngineSurface,
        session: pawork_domain::SessionId,
        sink: Arc<RecordingEvents>,
        control: Arc<dyn pawork_computer_use::NativeControl>,
        scope: pawork_computer_use::target::Scope,
    }

    struct ApprovalGate {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    struct ReleaseOnDrop(Arc<ApprovalGate>);

    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.release.notify_one();
        }
    }

    struct NativeEngineProvider {
        /// in-process 直跑 harness 用它把轮次停在 bind 前；wire 驱动由
        /// Host 的目标审批等待 seam 持住，传 None 即不在 Provider 层等待。
        gate: Option<Arc<ApprovalGate>>,
        round: AtomicUsize,
        offered: Mutex<Vec<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl ModelProvider for NativeEngineProvider {
        fn id(&self) -> ProviderId {
            ProviderId::from("mock")
        }

        async fn list_models(
            &self,
            _credential: Option<&pawork_domain::ResolvedCredential>,
        ) -> Result<Vec<ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            request: &CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            self.offered
                .lock()
                .expect("offered")
                .push(request.tools.iter().map(|tool| tool.name.clone()).collect());
            let round = self.round.fetch_add(1, Ordering::SeqCst);
            match round {
                0 => {
                    emit_tool_round(
                        sink,
                        "native-request",
                        serde_json::json!({"action": "request", "bundle_id": NATIVE_BUNDLE}),
                    )
                    .await
                }
                1 => {
                    if let Some(gate) = self.gate.as_ref() {
                        let waiting = gate.release.notified();
                        tokio::pin!(waiting);
                        waiting.as_mut().enable();
                        gate.entered.notify_one();
                        tokio::select! {
                            _ = &mut waiting => {}
                            _ = cancel.cancelled() => {
                                return Err(ProviderError::cancelled(
                                    "native engine approval wait cancelled",
                                ));
                            }
                        }
                    }
                    emit_tool_round(
                        sink,
                        "native-bind",
                        serde_json::json!({"action": "bind", "bundle_id": NATIVE_BUNDLE}),
                    )
                    .await
                }
                2 => {
                    let window = host_window_id(request).map_err(|error| {
                        ProviderError::new(ProviderErrorKind::InvalidRequest, error)
                    })?;
                    emit_tool_round(
                        sink,
                        "native-move",
                        serde_json::json!({
                            "action": "move_window",
                            "window": window,
                            "x": 12.0,
                            "y": 8.0
                        }),
                    )
                    .await
                }
                3 => {
                    sink.emit(ProviderStreamEvent::TextDelta("saw the window".into()))
                        .await?;
                    sink.emit(ProviderStreamEvent::ResponseCompleted(
                        StopReason::Completed,
                    ))
                    .await?;
                    Ok(response_summary(StopReason::Completed))
                }
                _ => Err(ProviderError::new(
                    ProviderErrorKind::StreamInterrupted,
                    "native engine provider script exhausted",
                )),
            }
        }
    }

    fn response_summary(stop_reason: StopReason) -> ModelResponseSummary {
        ModelResponseSummary {
            stop_reason,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: serde_json::Value::Null,
        }
    }

    async fn emit_tool_round(
        sink: &dyn ProviderEventSink,
        id: &str,
        arguments: serde_json::Value,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let id = ToolCallId::from(id);
        sink.emit(ProviderStreamEvent::ToolCallStarted {
            id: id.clone(),
            name: "computer_native".into(),
        })
        .await?;
        sink.emit(ProviderStreamEvent::ToolCallArgumentsDelta {
            id: id.clone(),
            json: serde_json::to_string(&arguments).expect("tool arguments serialize"),
        })
        .await?;
        sink.emit(ProviderStreamEvent::ToolCallCompleted { id })
            .await?;
        sink.emit(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse))
            .await?;
        Ok(response_summary(StopReason::ToolUse))
    }

    fn is_host_handle(id: &str, prefix: &str) -> bool {
        let Some(rest) = id
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('-'))
        else {
            return false;
        };
        let parts: Vec<&str> = rest.split('-').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
    }

    fn host_window_id(request: &CanonicalModelRequest) -> Result<String, String> {
        let mut found = None;
        for message in &request.messages {
            if message.role != MessageRole::Tool {
                continue;
            }
            for part in &message.content {
                let ContentPart::ToolResult(result) = part else {
                    continue;
                };
                let Some(metadata_id) = result
                    .metadata
                    .get("window_id")
                    .and_then(|value| value.as_str())
                else {
                    continue;
                };
                if !is_host_handle(metadata_id, "w") {
                    continue;
                }
                let line = match result.content.first() {
                    Some(ContentPart::Text(text_part)) => {
                        text_part.text.lines().next().unwrap_or("")
                    }
                    _ => return Err("bind tool result is missing text".into()),
                };
                let parsed: serde_json::Value = serde_json::from_str(line)
                    .map_err(|error| format!("bind tool text is not json: {error}"))?;
                let text_window = parsed
                    .get("window_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if text_window != metadata_id {
                    return Err(format!(
                        "bind window id metadata {metadata_id} disagrees with text {text_window}"
                    ));
                }
                found = Some(metadata_id.to_string());
            }
        }
        found.ok_or_else(|| "bind tool result did not include a host window id".into())
    }

    fn jpeg_at(content: &[ContentPart]) -> Vec<u8> {
        let ContentPart::Image(image) = content.get(1).expect("content part 1") else {
            panic!("content part 1 must be an image, got {content:?}");
        };
        let pawork_domain::ImageSource::Base64(encoded) = &image.source else {
            panic!("content part 1 must be inline base64");
        };
        assert_eq!(image.media_type, "image/jpeg");
        let bytes = crate::services::extension::base64_decode(encoded).expect("decode jpeg");
        assert_eq!(bytes, NATIVE_JPEG);
        bytes
    }

    fn tool_message_jpeg(message: &pawork_domain::Message) -> Vec<u8> {
        assert_eq!(message.role, MessageRole::Tool);
        let ContentPart::ToolResult(result) =
            message.content.first().expect("tool message content")
        else {
            panic!("tool message must carry one ToolResult, got {message:?}");
        };
        jpeg_at(&result.content)
    }

    struct NativeEngineCore {
        core: Arc<tokio::sync::RwLock<AppCore>>,
        adapter: crate::gui_host::GuiHostAdapter,
        _dir: tempfile::TempDir,
        surface: EngineSurface,
        control: Arc<dyn pawork_computer_use::NativeControl>,
        provider: Arc<NativeEngineProvider>,
    }

    /// EngineSurface + NativeEngineProvider 的共享装配：in-process 直跑
    /// （run_computer_native_engine）与 wire 驱动（native_wire_harness）
    /// 复用同一套表面、Provider 脚本与审批配置，不维护第二套镜像。
    async fn native_engine_core(
        gate: Option<Arc<ApprovalGate>>,
        approval_host: Arc<dyn crate::ApprovalPromptHost>,
        connections: Option<Arc<pawork_gui_server::ConnectionManager>>,
    ) -> NativeEngineCore {
        let surface = EngineSurface::new();
        let host = Arc::new(pawork_computer_use::NativeHost::new(surface.clone()));
        let control: Arc<dyn pawork_computer_use::NativeControl> = host;
        let performer: Arc<dyn pawork_computer_use::NativePerform> = control.clone();
        let dir = tempfile::tempdir().expect("tempdir");
        let (store, _) = SessionStore::open(dir.path().join("session.db"))
            .await
            .expect("store");
        let mut registry = pawork_models::ModelRegistry::empty();
        registry
            .register(pawork_models::CatalogEntry {
                id: pawork_domain::ModelId::from("mock-native-vision"),
                provider: pawork_domain::ProviderId::from("mock"),
                display_name: "mock-native-vision".into(),
                context_window_tokens: 0,
                max_output_tokens: 0,
                capabilities: pawork_domain::ModelCapabilities {
                    text: true,
                    image_input: true,
                    tool_calls: true,
                    ..pawork_domain::ModelCapabilities::default()
                },
                pricing: None,
                aliases: Vec::new(),
            })
            .expect("register vision model");
        let placeholder = MockProvider::new(MockScript::new().text("placeholder").complete())
            .with_id(pawork_domain::ProviderId::from("mock"));
        let mut core = AppCore::from_parts_with_protocol(
            Arc::new(placeholder),
            None,
            pawork_domain::ModelId::from("mock-native-vision"),
            pawork_domain::ProviderId::from("mock"),
            crate::protocol::AdapterProtocol::ChatCompletions,
            Some(store),
            registry,
        );
        core.native = control.clone();
        assert!(
            Arc::ptr_eq(&core.native, &control),
            "the turn must execute on AppCore.native"
        );
        let provider = Arc::new(NativeEngineProvider {
            gate,
            round: AtomicUsize::new(0),
            offered: Mutex::new(Vec::new()),
        });
        core.provider = Arc::clone(&provider) as Arc<dyn ModelProvider>;
        assert!(!core.provider_pending);
        assert!(!core.provider_stale);
        // ReadOnly 会在询问宿主前拒绝。attach 会按当前审批快照重装 scheduler。
        core.configure_approval(crate::ApprovalMode::AskForWrites, true, approval_host);
        core.attach_workspace(dir.path()).expect("attach");
        assert_eq!(core.workspace_id().as_str(), "ws-default");
        let tool: Arc<dyn pawork_domain::AgentTool> =
            Arc::new(pawork_tools::NativeComputerTool::new(performer));
        core.scheduler = Arc::new(
            core.scheduler
                .with_tools([tool])
                .expect("replace only computer_native"),
        );
        let locked = Arc::new(tokio::sync::RwLock::new(core));
        let mut adapter = crate::gui_host::GuiHostAdapter::from_locked(
            Arc::clone(&locked),
            Arc::new(crate::GuiApprovalHost::new()),
        );
        if let Some(connections) = connections {
            adapter = adapter.with_connections(connections);
        }
        NativeEngineCore {
            core: locked,
            adapter,
            _dir: dir,
            surface,
            control,
            provider,
        }
    }

    async fn run_computer_native_engine(approve: bool) -> NativeEngineRun {
        let gate = Arc::new(ApprovalGate {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let approval_host: Arc<dyn crate::ApprovalPromptHost> = if approve {
            Arc::new(ApproveNative)
        } else {
            Arc::new(DenyMoveWindow {
                asks: AtomicUsize::new(0),
            })
        };
        let parts = native_engine_core(Some(Arc::clone(&gate)), approval_host, None).await;
        let scope = pawork_computer_use::target::Scope::new("ws-default", "run-native");
        let session = parts
            .core
            .read()
            .await
            .create_session("native engine")
            .await
            .expect("session");
        let sink = Arc::new(RecordingEvents::default());
        // from_locked 与本轮共用 AppCore，且不替换工具审批宿主。
        // new / with_approvals 会 set_approval_host，把 ApproveNative 换掉。
        let entered = gate.entered.notified();
        tokio::pin!(entered);
        entered.as_mut().enable();
        let release_on_drop = ReleaseOnDrop(Arc::clone(&gate));
        let core_for_turn = Arc::clone(&parts.core);
        let sink_for_turn = Arc::clone(&sink);
        let session_for_turn = session.clone();
        // Tokio RwLock 不可重入。轮次任务持读锁停在 provider.stream；
        // 目标批准在测试任务上再取一把读锁。
        let turn = tokio::spawn(async move {
            let guard = core_for_turn.read().await;
            guard
                .chat_turn_with_run_id(
                    pawork_domain::RunId::from("run-native"),
                    &session_for_turn,
                    vec![user_hello()],
                    &*sink_for_turn as &dyn pawork_engine::AgentEventSink,
                    CancellationToken::new(),
                    None,
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30), entered)
            .await
            .expect("provider did not reach the bind round");
        assert!(
            parts
                .control
                .active_scopes()
                .iter()
                .any(|open| open == &scope),
            "the turn opens ws-default/run-native before bind"
        );
        assert_eq!(
            parts.control.grant_kind(&scope, &native_target()),
            None,
            "request records a choice and does not grant"
        );
        assert!(
            sink.0
                .lock()
                .expect("events")
                .iter()
                .any(|envelope| { matches!(envelope.payload, AgentEvent::RunStarted { .. }) }),
            "RunStarted is persisted before the provider loop"
        );
        let receipt = approve_running_target(&parts.adapter, &session).await;
        assert_eq!(receipt["kind"], "for_run", "{receipt}");
        assert_eq!(receipt["target"]["kind"], "application", "{receipt}");
        assert_eq!(
            receipt["target"]["app"]["bundle_id"], NATIVE_BUNDLE,
            "{receipt}"
        );
        assert_eq!(
            parts.control.grant_kind(&scope, &native_target()),
            Some(pawork_computer_use::approval::GrantKind::ForRun),
            "GuiHostAdapter grants ForRun on ws-default/run-native"
        );
        drop(release_on_drop);
        let joined = tokio::time::timeout(std::time::Duration::from_secs(30), turn)
            .await
            .expect("native engine turn timed out");
        joined
            .expect("native engine task")
            .expect("native engine turn");
        let calls = parts.provider.offered.lock().expect("offered").clone();
        assert!(
            calls.len() == 4
                && calls
                    .iter()
                    .all(|names| { names.iter().any(|name| name == "computer_native") }),
            "vision model must be offered computer_native: {calls:?}"
        );
        let NativeEngineCore {
            core,
            adapter,
            _dir,
            surface,
            control,
            provider: _,
        } = parts;
        NativeEngineRun {
            core,
            adapter,
            _dir,
            surface,
            session,
            sink,
            control,
            scope,
        }
    }

    /// CU-10：视觉模型的 computer_native 截图走 AgentTool、Scheduler 和 Engine。
    /// 一次真实 Run：request → GuiHostAdapter ForRun → bind → move_window。
    /// 观察 JPEG 落在 move 结果的 content part 1；resume 和 GUI 读取只消费它。
    #[tokio::test]
    async fn computer_native_engine_persists_screenshot_for_gui_read() {
        let run = run_computer_native_engine(true).await;
        use pawork_gui_server::GuiHost;
        assert_eq!(run.surface.moves(), 1, "move_window dispatches once");
        assert_eq!(
            run.surface.captures(),
            1,
            "for-run observation captures once"
        );
        let envelopes = run.sink.0.lock().expect("events").clone();
        let completed = move_completed(&envelopes);
        assert_move_observation(completed);
        let move_id = envelopes
            .iter()
            .find_map(|envelope| match &envelope.payload {
                AgentEvent::ToolExecutionCompleted { tool_call_id, .. }
                    if tool_action(&envelopes, tool_call_id) == "move_window" =>
                {
                    Some(tool_call_id)
                }
                _ => None,
            });
        let move_window = tool_arguments(&envelopes, move_id.expect("move id"))["window"]
            .as_str()
            .expect("move window argument")
            .to_string();
        assert_eq!(
            move_window,
            completed.metadata["window_id"]
                .as_str()
                .expect("result window")
        );
        tool_message_jpeg(image_tool_message(&envelopes));
        let messages = {
            let core = run.core.read().await;
            core.resume_messages(&run.session).await.expect("resume")
        };
        let tool_messages: Vec<_> = messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .collect();
        assert!(
            tool_messages.len() >= 3,
            "resume keeps request, bind, and move"
        );
        assert!(!message_has_image(tool_messages[0]));
        assert!(!message_has_image(tool_messages[1]));
        tool_message_jpeg(
            tool_messages
                .into_iter()
                .find(|message| message_has_image(message))
                .expect("resumed move image"),
        );

        let listed = run
            .adapter
            .query(&pawork_protocol::AppQueryEnvelope {
                api_version: pawork_protocol::API_VERSION,
                request_id: pawork_domain::QueryId::from("query-native-list"),
                source: pawork_protocol::CommandSource::Automation,
                identity: pawork_protocol::ActorIdentity::System,
                issued_at: pawork_domain::Timestamp::from_unix_millis(1),
                query: pawork_protocol::AppQuery::ObservationList {
                    session_id: run.session.clone(),
                    after_sequence: None,
                    limit: None,
                },
            })
            .await
            .expect("observation list");
        let pawork_protocol::AppResponse::Data(data) = listed else {
            panic!("observation list returns data: {listed:?}");
        };
        let entries = data["entries"].as_array().expect("entries");
        assert_eq!(entries.len(), 1, "one persisted screenshot: {data}");
        let entry = &entries[0];
        assert_eq!(entry["tool_name"], "computer_native");
        assert_eq!(entry["part_index"], 1);
        assert_eq!(entry["media_type"], "image/jpeg");
        assert_eq!(entry["byte_length"], NATIVE_JPEG.len());
        let sequence = entry["sequence"].as_u64().expect("sequence");
        let read = run
            .adapter
            .query(&pawork_protocol::AppQueryEnvelope {
                api_version: pawork_protocol::API_VERSION,
                request_id: pawork_domain::QueryId::from("query-native-read"),
                source: pawork_protocol::CommandSource::Automation,
                identity: pawork_protocol::ActorIdentity::System,
                issued_at: pawork_domain::Timestamp::from_unix_millis(1),
                query: pawork_protocol::AppQuery::ObservationImageRead {
                    session_id: run.session.clone(),
                    sequence,
                    part_index: 1,
                    offset: 0,
                    limit: 64,
                },
            })
            .await
            .expect("observation image read");
        let pawork_protocol::AppResponse::Data(image) = read else {
            panic!("observation read returns data: {read:?}");
        };
        assert_eq!(image["eof"], true);
        let encoded = image["data"].as_str().expect("base64");
        assert_eq!(
            crate::services::extension::base64_decode(encoded).expect("decode read"),
            NATIVE_JPEG
        );
        assert_eq!(
            run.surface.moves(),
            1,
            "gui observation reads must not move again"
        );
        assert_eq!(
            run.surface.captures(),
            1,
            "gui observation reads must not capture again"
        );
        assert!(
            !run.control
                .active_scopes()
                .iter()
                .any(|open| open == &run.scope),
            "turn end leaves the native scope inactive"
        );
        assert_eq!(
            run.control.grant_kind(&run.scope, &native_target()),
            None,
            "turn end drops the native grant"
        );
        // try_unwrap 要求 Core 只剩 adapter 这一份；_dir 活到 shutdown 结束。
        drop(run.core);
        run.adapter.shutdown().await.expect("shutdown");
    }

    /// CU-10：request 与 bind 获批后拒绝 move_window。拒绝发生在执行前，
    /// 不产生 move、capture、该调用的 ToolExecutionStarted 或持久化图像。
    #[tokio::test]
    async fn computer_native_engine_denial_does_not_capture() {
        let run = run_computer_native_engine(false).await;
        assert_eq!(run.surface.moves(), 0, "denied move must not dispatch");
        assert_eq!(run.surface.captures(), 0, "denied call must not capture");
        assert!(
            !run.control
                .active_scopes()
                .iter()
                .any(|open| open == &run.scope),
            "turn end leaves the native scope inactive"
        );
        assert_eq!(
            run.control.grant_kind(&run.scope, &native_target()),
            None,
            "turn end drops the native grant"
        );
        let envelopes = run.sink.0.lock().expect("events").clone();
        let completions = ordered_completions(&envelopes);
        let actions: Vec<_> = completions
            .iter()
            .map(|(action, _)| action.as_str())
            .collect();
        assert_eq!(actions.first().copied(), Some("request"), "{actions:?}");
        assert_eq!(actions.get(1).copied(), Some("bind"), "{actions:?}");
        assert!(
            completions.iter().all(|(action, result)| {
                action != "move_window"
                    || result
                        .metadata
                        .get("outcome")
                        .and_then(|value| value.as_str())
                        != Some("dispatched")
            }),
            "denied move must not dispatch: {actions:?}"
        );
        let bind_window = completions[1].1.metadata["window_id"]
            .as_str()
            .expect("bind window");
        assert!(is_host_handle(bind_window, "w"), "{bind_window}");
        let approvals: Vec<_> = envelopes
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                AgentEvent::ToolApprovalResponded {
                    tool_call_id,
                    decision,
                    ..
                } => Some((
                    tool_action(&envelopes, tool_call_id),
                    decision.clone(),
                    tool_call_id.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(approvals.len(), 3, "{approvals:?}");
        assert_eq!(approvals[0].0, "request");
        assert_eq!(approvals[0].1, ApprovalDecision::ApprovedOnce);
        assert_eq!(approvals[1].0, "bind");
        assert_eq!(approvals[1].1, ApprovalDecision::ApprovedOnce);
        assert_eq!(approvals[2].0, "move_window");
        assert_eq!(approvals[2].1, ApprovalDecision::Denied);
        let move_window = tool_arguments(&envelopes, &approvals[2].2)["window"]
            .as_str()
            .expect("move window argument")
            .to_string();
        assert_eq!(move_window, bind_window);
        assert!(
            envelopes.iter().any(|envelope| {
                matches!(
                    &envelope.payload,
                    AgentEvent::ToolExecutionStarted { tool_call_id }
                        if tool_call_id == &approvals[0].2
                )
            }),
            "request still executes"
        );
        assert!(
            envelopes.iter().any(|envelope| {
                matches!(
                    &envelope.payload,
                    AgentEvent::ToolExecutionStarted { tool_call_id }
                        if tool_call_id == &approvals[1].2
                )
            }),
            "bind still executes"
        );
        assert!(
            envelopes.iter().all(|envelope| {
                !matches!(
                    &envelope.payload,
                    AgentEvent::ToolExecutionStarted { tool_call_id }
                        if tool_call_id == &approvals[2].2
                )
            }),
            "denied move_window must not start execution"
        );
        assert!(
            envelopes.iter().all(|envelope| match &envelope.payload {
                AgentEvent::ToolExecutionCompleted { result, .. } => {
                    !result
                        .content
                        .iter()
                        .any(|part| matches!(part, ContentPart::Image(_)))
                }
                AgentEvent::MessageCommitted { message } => !message.content.iter().any(|part| {
                    match part {
                        ContentPart::Image(_) => true,
                        ContentPart::ToolResult(result) => result
                            .content
                            .iter()
                            .any(|inner| matches!(inner, ContentPart::Image(_))),
                        _ => false,
                    }
                }),
                _ => true,
            }),
            "denial must not persist a screenshot"
        );
        drop(run.core);
        run.adapter.shutdown().await.expect("shutdown");
    }

    async fn approve_running_target(
        adapter: &crate::gui_host::GuiHostAdapter,
        session: &pawork_domain::SessionId,
    ) -> serde_json::Value {
        use pawork_gui_server::GuiHost;
        let envelope = pawork_protocol::AppCommandEnvelope {
            api_version: pawork_protocol::API_VERSION,
            command_id: pawork_domain::CommandId::from("cmd-native-approve"),
            source: pawork_protocol::CommandSource::LocalGui {
                client_id: pawork_domain::GuiClientId::from("native-engine"),
            },
            identity: pawork_protocol::ActorIdentity::System,
            expected_revision: None,
            idempotency_key: None,
            issued_at: pawork_domain::Timestamp::from_unix_millis(1),
            command: pawork_protocol::AppCommand::ComputerTargetApprove(
                pawork_protocol::ComputerTargetApproveParams {
                    session_id: session.clone(),
                    run_id: pawork_domain::RunId::from("run-native"),
                    target: pawork_protocol::ComputerTargetIdentity::Application {
                        app: pawork_protocol::ComputerAppIdentity {
                            bundle_id: NATIVE_BUNDLE.into(),
                            family: pawork_protocol::ComputerAppFamily::Appkit,
                        },
                    },
                    kind: pawork_protocol::ComputerGrantKind::ForRun,
                },
            ),
        };
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            adapter.command(&envelope),
        )
        .await
        .expect("target approval timed out")
        .expect("target approval");
        let pawork_protocol::AppResponse::Data(receipt) = response else {
            panic!("target approval returns data: {response:?}");
        };
        receipt
    }

    fn tool_arguments(envelopes: &[AgentEventEnvelope], id: &ToolCallId) -> serde_json::Value {
        let mut json = String::new();
        for envelope in envelopes {
            if let AgentEvent::ToolCallArgumentsDelta {
                tool_call_id,
                json_delta,
            } = &envelope.payload
            {
                if tool_call_id == id {
                    json.push_str(json_delta);
                }
            }
        }
        serde_json::from_str(&json).unwrap_or(serde_json::Value::Null)
    }

    fn tool_action(envelopes: &[AgentEventEnvelope], id: &ToolCallId) -> String {
        tool_arguments(envelopes, id)
            .get("action")
            .and_then(|action| action.as_str())
            .unwrap_or("")
            .to_string()
    }

    fn ordered_completions(
        envelopes: &[AgentEventEnvelope],
    ) -> Vec<(String, &pawork_domain::ToolResultContent)> {
        envelopes
            .iter()
            .filter_map(|envelope| match &envelope.payload {
                AgentEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                } => Some((tool_action(envelopes, tool_call_id), result)),
                _ => None,
            })
            .collect()
    }

    fn move_completed(envelopes: &[AgentEventEnvelope]) -> &pawork_domain::ToolResultContent {
        let actions: Vec<String> = ordered_completions(envelopes)
            .into_iter()
            .map(|(action, _)| action)
            .collect();
        assert!(
            actions.len() >= 3,
            "request and bind complete before move: {actions:?}"
        );
        assert_eq!(actions[0], "request", "{actions:?}");
        assert_eq!(actions[1], "bind", "{actions:?}");
        let index = actions
            .iter()
            .position(|action| action == "move_window")
            .expect("move_window completion");
        assert!(
            index >= 2,
            "move completion must follow request and bind: {actions:?}"
        );
        envelopes
            .iter()
            .find_map(|envelope| match &envelope.payload {
                AgentEvent::ToolExecutionCompleted {
                    tool_call_id,
                    result,
                } if tool_action(envelopes, tool_call_id) == "move_window" => Some(result),
                _ => None,
            })
            .expect("move_window completion")
    }

    fn assert_move_observation(result: &pawork_domain::ToolResultContent) {
        let text = match result.content.first() {
            Some(ContentPart::Text(text)) => text.text.as_str(),
            other => panic!("move result must start with text, got {other:?}"),
        };
        assert!(
            text.contains("\"input_dispatched\":true"),
            "tool_result json text, not input_dispatched=true: {text}"
        );
        assert_eq!(result.metadata["input_dispatched"], true);
        assert_eq!(result.metadata["outcome"], "dispatched");
        let observation = result.metadata["observation"]
            .as_str()
            .expect("observation id");
        assert!(
            is_host_handle(observation, "o"),
            "observation {observation}"
        );
        let window = result.metadata["window_id"].as_str().expect("window id");
        assert!(is_host_handle(window, "w"), "window {window}");
        jpeg_at(&result.content);
    }

    fn message_has_image(message: &pawork_domain::Message) -> bool {
        message.content.iter().any(|part| match part {
            ContentPart::Image(_) => true,
            ContentPart::ToolResult(result) => result
                .content
                .iter()
                .any(|inner| matches!(inner, ContentPart::Image(_))),
            _ => false,
        })
    }

    fn image_tool_message(envelopes: &[AgentEventEnvelope]) -> &pawork_domain::Message {
        let mut seen = 0usize;
        let mut found = None;
        for envelope in envelopes {
            let AgentEvent::MessageCommitted { message } = &envelope.payload else {
                continue;
            };
            if message.role != MessageRole::Tool {
                continue;
            }
            if message_has_image(message) {
                assert!(seen >= 2, "image tool message must follow request and bind");
                found = Some(message);
            }
            seen += 1;
        }
        found.expect("move tool message")
    }

    // ------------------------------------------------------------------
    // CU-10 #5 wire 驱动：真实 GuiServer + GuiClient 的产品链证据。
    // 复用上面的 EngineSurface / NativeEngineProvider；RunStart 经握手名
    // 恰为 pawork-cli 的真实客户端发出，目标批准经第二个 GUI 客户端走
    // wire，断线恢复与取消也在同一条连接路径上验证。
    // ------------------------------------------------------------------
    use pawork_client::{ClientConfig, ClientError, GuiClient};
    use pawork_domain::{RunId, SessionId};
    use pawork_protocol::{
        ActorIdentity, AppCommand, AppEvent, AppQuery, AppResponse, CommandSource,
        ResumeDisposition, RunState, TimelineItemKind, TimelinePage,
    };
    use pawork_transport::{
        ConnectOptions, GuiConnection, GuiListener, GuiTransportClient, MemoryTransport,
        TransportEndpoint,
    };

    struct NativeWireHarness {
        core: Arc<tokio::sync::RwLock<AppCore>>,
        adapter: crate::gui_host::GuiHostAdapter,
        _dir: tempfile::TempDir,
        surface: EngineSurface,
        control: Arc<dyn pawork_computer_use::NativeControl>,
        provider: Arc<NativeEngineProvider>,
        connections: Arc<pawork_gui_server::ConnectionManager>,
        transport: Arc<MemoryTransport>,
        listener: Arc<dyn GuiListener>,
        channel: String,
        accepted: Vec<Box<dyn GuiConnection>>,
    }

    static WIRE_CHANNEL_NEXT: AtomicUsize = AtomicUsize::new(1);

    async fn native_wire_harness(label: &str) -> NativeWireHarness {
        let connections = Arc::new(pawork_gui_server::ConnectionManager::new());
        let parts = native_engine_core(
            None,
            Arc::new(ApproveNative),
            Some(Arc::clone(&connections)),
        )
        .await;
        let channel = format!(
            "cu10-native-wire-{label}-{}",
            WIRE_CHANNEL_NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let transport = Arc::new(MemoryTransport::new());
        let server = pawork_gui_server::GuiServer::new(pawork_gui_server::GuiServerConfig {
            host: Arc::new(parts.adapter.clone()),
            handshake: pawork_protocol::HandshakeService::new(
                pawork_gui_server::GuiHost::instance_id(&parts.adapter),
                pawork_protocol::SUPPORTED_API_VERSIONS.to_vec(),
                vec![pawork_protocol::GuiCapability::Events],
            ),
            transport: transport.clone(),
            connections: Some(Arc::clone(&connections)),
        });
        let listener = server
            .bind(TransportEndpoint::Memory {
                channel: channel.clone(),
            })
            .await
            .expect("bind memory transport");
        drop(server);
        NativeWireHarness {
            core: parts.core,
            adapter: parts.adapter,
            _dir: parts._dir,
            surface: parts.surface,
            control: parts.control,
            provider: parts.provider,
            connections,
            transport,
            listener: Arc::from(listener),
            channel,
            accepted: Vec::new(),
        }
    }

    impl NativeWireHarness {
        fn endpoint(&self) -> TransportEndpoint {
            TransportEndpoint::Memory {
                channel: self.channel.clone(),
            }
        }

        fn connect_options(name: &str) -> ConnectOptions {
            ConnectOptions {
                timeout_ms: 5_000,
                client_label: Some(name.into()),
                max_frame_bytes: 1024 * 1024,
            }
        }

        fn client_config(name: &str) -> ClientConfig {
            ClientConfig {
                client_name: name.into(),
                ..ClientConfig::default()
            }
        }

        async fn connect_wire(&mut self, name: &str) -> GuiClient {
            let accept = {
                let listener = Arc::clone(&self.listener);
                tokio::spawn(async move { listener.accept().await })
            };
            let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
            let client = GuiClient::connect_with_config(
                transport,
                self.endpoint(),
                Self::connect_options(name),
                None,
                Self::client_config(name),
            )
            .await
            .expect("wire connect + handshake");
            let connection = accept.await.expect("accept task").expect("wire accept");
            self.accepted.push(connection);
            client
        }

        async fn connect_wire_resume(
            &mut self,
            name: &str,
            last_acked: pawork_protocol::GlobalSequence,
        ) -> (GuiClient, Option<pawork_client::ResumeOutcome>) {
            let accept = {
                let listener = Arc::clone(&self.listener);
                tokio::spawn(async move { listener.accept().await })
            };
            let transport: Arc<dyn GuiTransportClient> = self.transport.clone();
            let (client, outcome) = GuiClient::connect_with_resume_config(
                transport,
                self.endpoint(),
                Self::connect_options(name),
                None,
                Some(last_acked),
                Self::client_config(name),
            )
            .await
            .expect("wire reconnect + resume");
            let connection = accept.await.expect("accept task").expect("wire accept");
            self.accepted.push(connection);
            (client, outcome)
        }

        async fn finish(self) {
            let NativeWireHarness {
                core,
                adapter,
                _dir,
                surface: _,
                control: _,
                provider: _,
                connections: _,
                transport: _,
                listener,
                channel: _,
                accepted,
            } = self;
            // close 只是停止信号；listener / accepted 对象本身持
            // Inner→host→adapter 副本，必须显式 drop 才解除 core 引用。
            for connection in accepted {
                let _ = connection.close().await;
            }
            let _ = listener.close().await;
            drop(listener);
            // 客户端已断开、泵任务异步退出；给一个有界收敛窗口。
            for _ in 0..200 {
                if Arc::strong_count(&core) == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(
                Arc::strong_count(&core),
                2,
                "listener/connection holders must release the core lock before shutdown"
            );
            drop(core);
            adapter.shutdown().await.expect("adapter shutdown");
            drop(_dir);
        }
    }

    fn wire_source(client: &GuiClient) -> CommandSource {
        // 服务端会在入站时盖章为 LocalGui{client_id}，这里按 CLI 真实
        // 调用形状传 RemoteGui。
        CommandSource::RemoteGui {
            client_id: client.client_id().clone(),
            connection_id: client.connection_id().clone(),
        }
    }

    fn wire_identity() -> ActorIdentity {
        ActorIdentity::LocalUser {
            actor_id: pawork_domain::ActorId::from("pawork-cli"),
            display_name: None,
        }
    }

    async fn wire_session(client: &GuiClient, title: &str) -> SessionId {
        let envelope = client
            .command(
                AppCommand::SessionCreate {
                    workspace_id: Some(pawork_domain::WorkspaceId::from("ws-default")),
                    title: Some(title.into()),
                },
                wire_source(client),
                wire_identity(),
            )
            .await
            .expect("wire SessionCreate");
        match envelope.response {
            AppResponse::Data(data) => SessionId::from(
                data.get("session_id")
                    .and_then(serde_json::Value::as_str)
                    .expect("session_id in SessionCreate data"),
            ),
            other => panic!("wire SessionCreate should return Data, got {other:?}"),
        }
    }

    async fn wire_run_start(client: &GuiClient, session: &SessionId, prompt: &str) -> RunId {
        let envelope = client
            .command(
                AppCommand::RunStart {
                    session_id: session.clone(),
                    user_message: prompt.into(),
                    model: None,
                    provider: None,
                    profile: None,
                    effort: None,
                    attachment_ids: Vec::new(),
                    web_search: None,
                    video_urls: Vec::new(),
                },
                wire_source(client),
                wire_identity(),
            )
            .await
            .expect("wire RunStart");
        match envelope.response {
            AppResponse::Accepted {
                run_id: Some(run_id),
                ..
            } => run_id,
            other => panic!("wire RunStart should return Accepted with run_id, got {other:?}"),
        }
    }

    async fn wire_approve_target(
        gui: &GuiClient,
        session: &SessionId,
        run: &RunId,
    ) -> serde_json::Value {
        let envelope = gui
            .command(
                AppCommand::ComputerTargetApprove(pawork_protocol::ComputerTargetApproveParams {
                    session_id: session.clone(),
                    run_id: run.clone(),
                    target: pawork_protocol::ComputerTargetIdentity::Application {
                        app: pawork_protocol::ComputerAppIdentity {
                            bundle_id: NATIVE_BUNDLE.into(),
                            family: pawork_protocol::ComputerAppFamily::Appkit,
                        },
                    },
                    kind: pawork_protocol::ComputerGrantKind::ForRun,
                }),
                wire_source(gui),
                wire_identity(),
            )
            .await
            .expect("wire ComputerTargetApprove");
        match envelope.response {
            AppResponse::Data(receipt) => receipt,
            other => panic!("wire ComputerTargetApprove should return Data, got {other:?}"),
        }
    }

    fn wire_terminal(run: &RunId, event: &AppEvent) -> Option<RunState> {
        match event {
            AppEvent::RunChanged { run_id, state } if run_id == run => matches!(
                state,
                RunState::Completed | RunState::Cancelled | RunState::Failed
            )
            .then(|| state.clone()),
            _ => None,
        }
    }

    async fn wire_ack_until(
        client: &GuiClient,
        timeout: std::time::Duration,
        mut predicate: impl FnMut(&AppEvent) -> bool,
        what: &str,
    ) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                panic!("wire event wait timed out: {what}");
            };
            match client.next_event_timeout(remaining).await {
                Ok(envelope) => {
                    client.ack(envelope.global_sequence).await.expect("ack");
                    if predicate(&envelope.payload) {
                        return;
                    }
                }
                Err(ClientError::Timeout { .. }) => panic!("wire event wait timed out: {what}"),
                Err(error) => panic!("wire event stream failed: {error}"),
            }
        }
    }

    async fn wire_ack_terminal(client: &GuiClient, run: &RunId) -> RunState {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                panic!("run {} did not reach a terminal state", run.as_str());
            };
            match client.next_event_timeout(remaining).await {
                Ok(envelope) => {
                    client.ack(envelope.global_sequence).await.expect("ack");
                    if let Some(state) = wire_terminal(run, &envelope.payload) {
                        return state;
                    }
                }
                Err(ClientError::Timeout { .. }) => {
                    panic!("run {} did not reach a terminal state", run.as_str())
                }
                Err(error) => panic!("wire event stream failed: {error}"),
            }
        }
    }

    /// 等待 seam 证据：request 已完成、目标未批准时，bind 不得产生第二个
    /// ToolCompleted，run 也不得进入终态（无等待会立刻 NotAuthorized 失败）。
    async fn wire_assert_paused_before_approval(client: &GuiClient, run: &RunId) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        loop {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return;
            };
            match client.next_event_timeout(remaining).await {
                Ok(envelope) => {
                    client.ack(envelope.global_sequence).await.expect("ack");
                    if let Some(state) = wire_terminal(run, &envelope.payload) {
                        panic!("run reached {state:?} before target approval");
                    }
                    if matches!(
                        &envelope.payload,
                        AppEvent::ToolCompleted { run_id, .. } if run_id == run
                    ) {
                        panic!("bind must wait for GUI target approval over the wire");
                    }
                }
                Err(ClientError::Timeout { .. }) => return,
                Err(error) => panic!("wire event stream failed during approval wait: {error}"),
            }
        }
    }

    async fn wire_timeline_contains(
        client: &GuiClient,
        session: &SessionId,
        run: &RunId,
        kind: TimelineItemKind,
    ) -> bool {
        let mut after: Option<u64> = None;
        for _ in 0..100 {
            let envelope = client
                .query(
                    AppQuery::SessionGet {
                        session_id: session.clone(),
                        timeline_after_sequence: after,
                        timeline_limit: Some(200),
                    },
                    wire_source(client),
                    wire_identity(),
                )
                .await
                .expect("wire timeline query");
            let data = match envelope.response {
                AppResponse::Data(data) => data,
                other => panic!("wire SessionGet should return Data, got {other:?}"),
            };
            let page: TimelinePage =
                serde_json::from_value(data.get("timeline_page").cloned().expect("timeline_page"))
                    .expect("timeline page decodes");
            if page
                .items
                .iter()
                .any(|item| item.run_id.as_deref() == Some(run.as_str()) && item.kind == kind)
            {
                return true;
            }
            if page.complete {
                return false;
            }
            match page.next_sequence {
                Some(next) if after.map(|current| next > current).unwrap_or(true) => {
                    after = Some(next);
                }
                _ => return false,
            }
        }
        false
    }

    async fn wire_observation_data(client: &GuiClient, session: &SessionId) -> serde_json::Value {
        let envelope = client
            .query(
                AppQuery::ObservationList {
                    session_id: session.clone(),
                    after_sequence: None,
                    limit: None,
                },
                wire_source(client),
                wire_identity(),
            )
            .await
            .expect("wire ObservationList");
        match envelope.response {
            AppResponse::Data(data) => data,
            other => panic!("wire ObservationList should return Data, got {other:?}"),
        }
    }

    fn first_tool_completed(run: &RunId) -> impl FnMut(&AppEvent) -> bool + '_ {
        move |event| {
            matches!(
                event,
                AppEvent::ToolCompleted {
                    run_id, success: true, ..
                } if run_id == run
            )
        }
    }

    /// CU-10 #5 产品链主路径：握手名恰为 pawork-cli 的真实 GuiClient 经
    /// GuiServer 发 RunStart → request 落地 → bind 在等待 seam 挂住 →
    /// 第二个 GUI 客户端经 wire 批准目标 → bind + move_window 各一次 →
    /// Completed；input_dispatched 事实与截图持久化，ObservationList /
    /// Timeline 经 wire 可读，终态后 scope 收口。
    #[tokio::test]
    async fn computer_native_wire_cli_run_completes_after_gui_target_approval() {
        let mut harness = native_wire_harness("approve").await;
        let cli = harness.connect_wire("pawork-cli").await;
        let registered = harness
            .connections
            .session(cli.client_id())
            .expect("cli handshake registers a connection session");
        assert_eq!(registered.name, "pawork-cli");
        cli.subscribe_all().await.expect("wire subscribe");
        let session = wire_session(&cli, "cu10 wire native").await;
        let run = wire_run_start(&cli, &session, "move the notes window").await;
        wire_ack_until(
            &cli,
            std::time::Duration::from_secs(30),
            first_tool_completed(&run),
            "request tool completes over the wire",
        )
        .await;
        wire_assert_paused_before_approval(&cli, &run).await;
        let gui = harness.connect_wire("pawork-test-desktop").await;
        let receipt = wire_approve_target(&gui, &session, &run).await;
        assert_eq!(receipt["kind"], "for_run", "{receipt}");
        assert_eq!(
            receipt["target"]["app"]["bundle_id"], NATIVE_BUNDLE,
            "{receipt}"
        );
        let state = wire_ack_terminal(&cli, &run).await;
        assert!(
            matches!(state, RunState::Completed),
            "terminal state: {state:?}"
        );
        assert_eq!(harness.surface.moves(), 1, "move_window dispatches once");
        assert_eq!(
            harness.surface.captures(),
            1,
            "for-run observation captures once"
        );
        let calls = harness.provider.offered.lock().expect("offered").clone();
        assert!(
            calls.len() == 4
                && calls
                    .iter()
                    .all(|names| names.iter().any(|name| name == "computer_native")),
            "wire run must offer computer_native on every round: {calls:?}"
        );
        let messages = harness
            .core
            .read()
            .await
            .resume_messages(&session)
            .await
            .expect("resume messages");
        let dispatched = messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .filter_map(|message| match message.content.first() {
                Some(ContentPart::ToolResult(result)) => Some(result),
                _ => None,
            })
            .find(|result| {
                result
                    .metadata
                    .get("input_dispatched")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            })
            .expect("move tool result persists the input_dispatched fact");
        jpeg_at(&dispatched.content);
        let observations = wire_observation_data(&cli, &session).await;
        let entries = observations["entries"].as_array().expect("entries");
        let entry = entries
            .iter()
            .find(|entry| entry["run_id"].as_str() == Some(run.as_str()))
            .expect("observation entry for the wire run");
        assert_eq!(entry["media_type"], "image/jpeg");
        assert_eq!(
            entry["byte_length"].as_u64(),
            Some(NATIVE_JPEG.len() as u64)
        );
        assert!(
            wire_timeline_contains(&cli, &session, &run, TimelineItemKind::RunCompleted).await,
            "timeline must carry RunCompleted for the wire run"
        );
        let scope = pawork_computer_use::target::Scope::new("ws-default", run.as_str());
        assert!(
            !harness
                .control
                .active_scopes()
                .iter()
                .any(|open| open == &scope),
            "terminal closes the wire run scope"
        );
        assert_eq!(
            harness.control.grant_kind(&scope, &native_target()),
            None,
            "terminal drops the grant"
        );
        cli.disconnect().await.expect("cli disconnect");
        gui.disconnect().await.expect("gui disconnect");
        harness.finish().await;
    }

    /// CU-10 #5 断线恢复：CLI 在等待目标审批时断开（不发 RunCancel），
    /// Run 继续推进；GUI 经 wire 批准并观测到 Completed；CLI 以
    /// last_acked 重连，Resume 补发错过的事件，Timeline 找到同一 run。
    #[tokio::test]
    async fn computer_native_wire_cli_resume_recovers_terminal_after_disconnect() {
        let mut harness = native_wire_harness("resume").await;
        let cli = harness.connect_wire("pawork-cli").await;
        cli.subscribe_all().await.expect("wire subscribe");
        let session = wire_session(&cli, "cu10 wire resume").await;
        let run = wire_run_start(&cli, &session, "move the notes window after reconnect").await;
        wire_ack_until(
            &cli,
            std::time::Duration::from_secs(30),
            first_tool_completed(&run),
            "request completes before disconnect",
        )
        .await;
        let last_acked = cli.last_acked_sequence();
        cli.disconnect().await.expect("cli disconnect");
        let gui = harness.connect_wire("pawork-test-desktop").await;
        gui.subscribe_all().await.expect("gui subscribe");
        let receipt = wire_approve_target(&gui, &session, &run).await;
        assert_eq!(receipt["kind"], "for_run", "{receipt}");
        let state = wire_ack_terminal(&gui, &run).await;
        assert!(
            matches!(state, RunState::Completed),
            "gui observes Completed: {state:?}"
        );
        let (cli2, outcome) = harness.connect_wire_resume("pawork-cli", last_acked).await;
        let outcome = outcome.expect("resume executes with last_acked");
        match outcome.disposition {
            ResumeDisposition::Replay { .. } => {
                assert!(
                    outcome.replayed.iter().any(|envelope| {
                        matches!(
                            &envelope.payload,
                            AppEvent::RunChanged {
                                run_id,
                                state: RunState::Completed,
                            } if run_id == &run
                        )
                    }),
                    "replay must carry the missed terminal for {}",
                    run.as_str()
                );
            }
            other => panic!("resume should replay missed events, got {other:?}"),
        }
        assert!(
            wire_timeline_contains(&cli2, &session, &run, TimelineItemKind::RunCompleted).await,
            "timeline must carry RunCompleted for the resumed run"
        );
        assert_eq!(harness.surface.moves(), 1, "resume must not re-execute");
        cli2.disconnect().await.expect("reconnect disconnect");
        gui.disconnect().await.expect("gui disconnect");
        harness.finish().await;
    }

    /// CU-10 #5 取消语义：CLI 在 bind 等待目标审批时发 RunCancel，run 以
    /// Cancelled 收口，不产生任何 move / capture，timeline 记录 RunCancelled。
    #[tokio::test]
    async fn computer_native_wire_cancel_while_waiting_approval_zeros_dispatch() {
        let mut harness = native_wire_harness("cancel").await;
        let cli = harness.connect_wire("pawork-cli").await;
        cli.subscribe_all().await.expect("wire subscribe");
        let session = wire_session(&cli, "cu10 wire cancel").await;
        let run = wire_run_start(&cli, &session, "cancel before any dispatch").await;
        wire_ack_until(
            &cli,
            std::time::Duration::from_secs(30),
            first_tool_completed(&run),
            "request completes before cancel",
        )
        .await;
        wire_assert_paused_before_approval(&cli, &run).await;
        let envelope = cli
            .command(
                AppCommand::RunCancel {
                    run_id: run.clone(),
                },
                wire_source(&cli),
                wire_identity(),
            )
            .await
            .expect("wire RunCancel");
        assert!(
            matches!(envelope.response, AppResponse::Accepted { .. }),
            "RunCancel accepted: {:?}",
            envelope.response
        );
        let state = wire_ack_terminal(&cli, &run).await;
        assert!(
            matches!(state, RunState::Cancelled),
            "terminal state: {state:?}"
        );
        assert_eq!(
            harness.surface.moves(),
            0,
            "cancelled run must not move a window"
        );
        assert_eq!(
            harness.surface.captures(),
            0,
            "cancelled run must not capture"
        );
        assert!(
            wire_timeline_contains(&cli, &session, &run, TimelineItemKind::RunCancelled).await,
            "timeline must carry RunCancelled for the cancelled run"
        );
        let scope = pawork_computer_use::target::Scope::new("ws-default", run.as_str());
        assert!(
            !harness
                .control
                .active_scopes()
                .iter()
                .any(|open| open == &scope),
            "terminal closes the wire run scope"
        );
        assert_eq!(
            harness.control.grant_kind(&scope, &native_target()),
            None,
            "terminal drops the grant"
        );
        cli.disconnect().await.expect("cli disconnect");
        harness.finish().await;
    }
}
