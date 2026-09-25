use std::sync::atomic::Ordering;
use std::sync::Arc;

use pawork_domain::{
    AgentEvent, CancellationToken, ErrorCategory, ErrorContext, Message, MessageId, MessageRole,
    RunId, SessionId,
};
use pawork_engine::{now_timestamp, AgentEventSink};
use pawork_protocol::{AppCommand, AppCommandEnvelope, AppEvent, AppResponse, RunState};
use serde_json::json;

use crate::gui_server::GuiHostError;

use super::super::{ActiveGuiRun, GuiBroadcastSink, GuiHostAdapter, GuiRunRegistry};

/// 同会话 Run 占用槽的 RAII 守卫（R-06）：handler 内任何同步/异步拒绝
/// 路径自动释放；spawn 成功后 disarm，由 run 收尾释放。
pub(super) struct SessionSlotGuard {
    runs: Arc<GuiRunRegistry>,
    session_id: SessionId,
    held: bool,
}

impl SessionSlotGuard {
    pub(super) fn acquire(
        adapter: &GuiHostAdapter,
        session_id: &SessionId,
    ) -> Result<Self, GuiHostError> {
        if !adapter.runs.try_acquire_session(session_id) {
            return Err(GuiHostAdapter::host_error(
                "session_busy",
                "session already has an active operation",
            ));
        }
        Ok(Self {
            runs: Arc::clone(&adapter.runs),
            session_id: session_id.clone(),
            held: true,
        })
    }

    fn disarm(&mut self) {
        self.held = false;
    }
}

impl Drop for SessionSlotGuard {
    fn drop(&mut self) {
        if self.held {
            self.runs.release_session(&self.session_id);
        }
    }
}

/// engine 未报终态即死时的收口闸门（合成终态硬化）：先 best-effort 持久化
/// 真实 `RunFailed`（persist-first，参照非 live ToolApprove durable seal），
/// 成功后经正常映射补广播（携带真实持久化 sequence）；持久化失败才退回
/// `publish_raw` 合成兜底。两种路径都补 `run.failed` 诊断供 GUI 展示原因。
/// 早死若发生在 engine 持久化 `RunStarted` 之前，先补建 runs 行再落
/// `RunFailed`——projection 的终态 UPDATE 要求 run 行存在（R-19）。
pub(crate) async fn seal_run_without_terminal(
    core: &crate::AppCore,
    bus: &Arc<super::super::bus::GuiEventBus>,
    instance: pawork_domain::CoreInstanceId,
    session_id: &SessionId,
    run_id: &RunId,
    error: &crate::AppError,
) {
    // fail/cancel 路径 engine 已在 Err 返回前经 sink 广播真实终态；此处
    // 只处理未报终态即死（plan 闸门拒绝、宿主侧早退等），避免幽灵
    // "Run failed" 重复插入时间线且把 cancel 谎报为 Failed。
    if bus.terminal_reported(run_id.as_str()) {
        return;
    }
    let message = error.to_string();
    let sealed = async {
        // 早死时 engine 尚未持久化 RunStarted：先补建 runs 行，
        // 否则 RunFailed 的 projection UPDATE 无行可命中、整段
        // seal 落入合成兜底，重开 store 后该 run 无持久生命周期。
        // 以 store 投影为准（runs 行也可能绕开 bus 落库，如种子事件）。
        let run_row_exists = core
            .store()?
            .projection_snapshot(session_id)
            .await?
            .runs
            .iter()
            .any(|run| run.run_id == *run_id);
        let mut sequence = core.next_sequence(session_id).await?;
        let mut started = None;
        if !run_row_exists {
            started = Some(
                core.append_payload(
                    session_id,
                    run_id,
                    &mut sequence,
                    AgentEvent::RunStarted {
                        trigger_message_id: MessageId::from(format!(
                            "msg-{}-seal",
                            run_id.as_str()
                        )),
                    },
                )
                .await?,
            );
        }
        let failed = core
            .append_payload(
                session_id,
                run_id,
                &mut sequence,
                AgentEvent::RunFailed {
                    error: ErrorContext {
                        category: ErrorCategory::Internal,
                        message: message.clone(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    },
                    usage: None,
                },
            )
            .await?;
        Ok::<_, crate::AppError>((started, failed))
    }
    .await;
    match sealed {
        Ok((started, failed)) => {
            // persist-first 已落库；复用 live 路径的广播 sink 经正常映射
            // 补实时事件（真实 sequence），先补发 RunStarted 再发 RunFailed。
            let sink = GuiBroadcastSink::new(Arc::clone(bus), instance.clone());
            for envelope in started.into_iter().chain(std::iter::once(failed)) {
                if let Err(broadcast_error) = sink.emit(envelope).await {
                    tracing::warn!(
                        run_id = run_id.as_str(),
                        error = %broadcast_error,
                        "run failed durable seal broadcast failed"
                    );
                }
            }
        }
        Err(seal_error) => {
            tracing::warn!(
                run_id = run_id.as_str(),
                error = %seal_error,
                "run failed durable seal unavailable; falling back to synthetic terminal"
            );
            bus.publish_raw(
                instance.clone(),
                session_id,
                AppEvent::RunChanged {
                    run_id: run_id.clone(),
                    state: RunState::Failed,
                },
            );
        }
    }
    bus.publish_diagnostic(
        instance,
        session_id,
        "run.failed",
        json!({ "message": message }),
    );
}

/// 有 `RunStart.provider` 时按用户所选通道切换，禁止回退 catalog 首项。
pub(crate) fn run_start_requested_provider_switch(
    current_provider: &str,
    current_model: &str,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> Option<(String, Option<String>)> {
    let provider = requested_provider?;
    let already =
        current_provider == provider && requested_model.is_none_or(|model| current_model == model);
    if already {
        None
    } else {
        Some((provider.to_string(), requested_model.map(str::to_string)))
    }
}

/// 旧客户端兼容：仅有 model 时按 overview 顺序取首个同 id。
#[cfg(test)]
pub(crate) fn run_start_overview_owner<'a, P, M>(
    model: &str,
    overview: impl IntoIterator<Item = &'a (P, M)>,
) -> Option<String>
where
    P: AsRef<str> + 'a,
    M: AsRef<str> + 'a,
{
    overview
        .into_iter()
        .find_map(|(provider, id)| (id.as_ref() == model).then(|| provider.as_ref().to_string()))
}

pub(crate) async fn run_start(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
) -> Result<AppResponse, GuiHostError> {
    let _goal_change = adapter.goal_commands.lock().await;
    if let AppCommand::RunStart { session_id, .. } = command {
        if adapter
            .goals
            .lock()
            .unwrap()
            .contains_key(session_id.as_str())
        {
            return Err(GuiHostAdapter::host_error(
                "goal_active",
                "Pause the goal before sending a separate message",
            ));
        }
    }
    start(adapter, envelope, command, None).await
}
pub(crate) async fn start_for_goal(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
    budget: u64,
) -> Result<AppResponse, GuiHostError> {
    start(adapter, envelope, command, Some(budget)).await
}
async fn start(
    adapter: &GuiHostAdapter,
    envelope: &AppCommandEnvelope,
    command: &AppCommand,
    goal_budget: Option<u64>,
) -> Result<AppResponse, GuiHostError> {
    if adapter.runs.is_stopping() {
        return Err(GuiHostAdapter::host_error(
            "host_stopping",
            "Host is stopping",
        ));
    }
    let AppCommand::RunStart {
        session_id,
        user_message,
        model,
        provider,
        profile: _,
        effort,
        attachment_ids,
        web_search,
        video_urls,
    } = command
    else {
        unreachable!("run_start handler receives RunStart")
    };
    if !video_urls.is_empty() {
        if envelope.api_version.minor < 24 {
            return Err(GuiHostAdapter::host_error(
                "unsupported",
                "Video input requires API 1.24",
            ));
        }
        if video_urls.len() + attachment_ids.len() > 4 {
            return Err(GuiHostAdapter::host_error(
                "invalid_attachment",
                "At most four attachments and videos per turn",
            ));
        }
        for video in video_urls {
            video
                .validate()
                .map_err(|e| GuiHostAdapter::host_error("invalid_video", e))?;
        }
    }
    if !attachment_ids.is_empty() || web_search.is_some() {
        if envelope.api_version.minor < 22 {
            return Err(GuiHostAdapter::host_error(
                "unsupported",
                "local attachments and per-run web_search require API 1.22",
            ));
        }
        if !matches!(
            envelope.source,
            pawork_protocol::CommandSource::LocalGui { .. }
        ) {
            return Err(GuiHostAdapter::host_error(
                "unsupported",
                "local attachments and per-run web_search require a local GUI connection",
            ));
        }
    }
    // ADR-063：effort 名 fail-closed 先解析，非法名不启动 Run。
    let parsed_effort = match effort {
        Some(name) => Some(
            pawork_domain::ReasoningEffort::from_wire_name(name).ok_or_else(|| {
                GuiHostAdapter::host_error("invalid_effort", format!("unknown effort {name}"))
            })?,
        ),
        None => None,
    };
    // R-06：同一 Session 同时只允许一个活动 Run。占用早于首个 await，
    // join! 突发的第二个 RunStart 才能确定性拒绝；守卫覆盖此后所有
    // 拒绝路径，spawn 成功后 disarm、由 run 收尾释放。
    let mut session_slot = SessionSlotGuard::acquire(adapter, session_id)?;
    let (history, workspace_id, workspace_roots) = {
        let core = adapter.core.read().await;
        core.get_session(session_id)
            .await
            .map_err(GuiHostAdapter::app_error)?;
        // R-19：计划闸门前移到接受边界，未批准 Plan 同步拒绝，不再
        // 先回 Accepted 再异步 Failed。
        core.ensure_plan_allows_execution(session_id)
            .await
            .map_err(GuiHostAdapter::app_error)?;
        let workspace = core
            .workspace_for_session_or_unbound(session_id)
            .map_err(GuiHostAdapter::app_error)?;
        (
            core.resume_messages_keep_pending(session_id)
                .await
                .map_err(GuiHostAdapter::app_error)?,
            workspace.id.clone(),
            workspace.roots.clone(),
        )
    };
    let current = {
        let core = adapter.core.read().await;
        (
            core.provider_id().as_str().to_string(),
            core.model().as_str().to_string(),
        )
    };
    if let Some((requested_provider, requested_model)) = run_start_requested_provider_switch(
        &current.0,
        &current.1,
        provider.as_ref().map(|id| id.as_str()),
        model.as_ref().map(|id| id.as_str()),
    ) {
        let mut core = adapter.core.write().await;
        core.switch_provider(
            Some(session_id),
            &requested_provider,
            requested_model.as_deref(),
        )
        .await
        .map_err(GuiHostAdapter::app_error)?;
        let confirmed = (
            core.provider_id().as_str().to_string(),
            core.model().as_str().to_string(),
        );
        drop(core);
        if confirmed != current {
            adapter.bus.publish_diagnostic(
                adapter.instance.clone(),
                session_id,
                "model.switched",
                json!({
                    "from": {
                        "provider": current.0,
                        "model": current.1
                    },
                    "to": {
                        "provider": confirmed.0,
                        "model": confirmed.1,
                    }
                }),
            );
        }
    } else if provider.is_none() {
        if let Some(model) = model {
            if model.as_str() != current.1 {
                let mut core = adapter.core.write().await;
                let switched = match core.switch_model(Some(session_id), model.as_str()).await {
                    Ok(()) => Ok(()),
                    Err(crate::AppError::ModelBelongsToProvider { owner, .. }) => {
                        core.switch_provider(Some(session_id), &owner, Some(model.as_str()))
                            .await
                    }
                    Err(error @ crate::AppError::UnknownModel { .. }) => {
                        let owner = core
                            .models_overview()
                            .await
                            .into_iter()
                            .find(|entry| entry.id.as_str() == model.as_str())
                            .map(|entry| entry.provider.as_str().to_string());
                        match owner {
                            Some(owner) if owner != current.0 => {
                                match core
                                    .switch_provider(Some(session_id), &owner, Some(model.as_str()))
                                    .await
                                {
                                    Ok(()) => Ok(()),
                                    Err(crate::AppError::UnknownModel { .. }) => {
                                        match core
                                            .switch_provider(Some(session_id), &owner, None)
                                            .await
                                        {
                                            Ok(()) => {
                                                core.switch_model(Some(session_id), model.as_str())
                                                    .await
                                            }
                                            Err(other) => Err(other),
                                        }
                                    }
                                    Err(other) => Err(other),
                                }
                            }
                            _ => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                };
                switched.map_err(GuiHostAdapter::app_error)?;
                let confirmed = (
                    core.provider_id().as_str().to_string(),
                    core.model().as_str().to_string(),
                );
                drop(core);
                adapter.bus.publish_diagnostic(
                    adapter.instance.clone(),
                    session_id,
                    "model.switched",
                    json!({
                        "from": {
                            "provider": current.0,
                            "model": current.1
                        },
                        "to": {
                            "provider": confirmed.0,
                            "model": confirmed.1,
                        }
                    }),
                );
            }
        }
    }
    // 凭证或供应商代理变化后，同 provider/model 也必须重装配；先完成
    // 上面的显式模型选择，保留旧客户端仅传 model 的解析顺序。
    {
        // ADR-063：显式 effort 随 RunStart 落地；缺省清除陈旧显式值，
        // 由装配层回落模型级默认 / Provider 默认。
        adapter.core.write().await.set_effort(parsed_effort);
        let mut core = adapter.core.write().await;
        *core.subagent_render.lock().unwrap() = Some(Arc::new(GuiBroadcastSink::new(
            adapter.bus.clone(),
            adapter.instance.clone(),
        )));
        let terminal = core.config().terminal.clone().unwrap_or_default();
        let tools: Vec<Arc<dyn pawork_domain::AgentTool>> = vec![
            Arc::new(super::super::terminal_tool::TerminalTool {
                pty: adapter.pty.clone(),
                terminals: adapter.terminals.clone(),
                runs: adapter.runs.clone(),
                bus: adapter.bus.clone(),
                instance: adapter.instance.clone(),
                shell: terminal.shell,
                size: pawork_exec::PtyWindowSize {
                    cols: terminal.columns.unwrap_or(80),
                    rows: terminal.rows.unwrap_or(24),
                    pixel_width: 0,
                    pixel_height: 0,
                },
            }),
            Arc::new(super::super::browser_tool::BrowserTool {
                broker: adapter.browser.clone(),
                runs: adapter.runs.clone(),
            }),
        ];
        core.scheduler = Arc::new(
            core.scheduler
                .with_tools(tools.clone())
                .map_err(|error| GuiHostAdapter::host_error("internal", error.to_string()))?,
        );
        for tool in tools {
            let descriptor = tool.descriptor();
            core.descriptors.retain(|item| item.name != descriptor.name);
            core.tool_defs.retain(|item| item.name != descriptor.name);
            core.tool_defs.push(pawork_domain::ToolDefinition {
                name: descriptor.name.clone(),
                description: descriptor.description.clone(),
                input_schema: descriptor.input_schema.clone(),
            });
            core.descriptors.push(descriptor);
        }
        if core.provider_needs_rebuild() {
            let provider = core.provider_id().as_str().to_string();
            let model = core.model().as_str().to_string();
            core.switch_provider(None, &provider, Some(&model))
                .await
                .map_err(GuiHostAdapter::app_error)?;
        }
    }
    // ADR-055 D4：会话当前生效模型被禁用时结构化 fail-closed——不启动
    // Run、不回退其他模型（显式切换路径由 switch_model/switch_provider
    // 同闸拦截；此处覆盖无切换请求但生效对已禁用的情形）。必须在登记
    // ActiveGuiRun 之前完成：失败路径不能留下幽灵 run。
    {
        let core = adapter.core.read().await;
        let (provider, model) = (
            core.provider_id().as_str().to_string(),
            core.model().as_str().to_string(),
        );
        if !core.config().is_model_enabled(&provider, &model) {
            return Err(GuiHostAdapter::app_error(crate::AppError::ModelDisabled {
                provider,
                model,
            }));
        }
    }
    // 与 CLI run_one_turn 同一语义：user text 原样为首 part，`@token` 命中的
    // file-index 附件作为独立 Text part 追加；无 `@` 或未命中时零行为变化。
    // 解析失败按 fail-closed 上抛，禁止把未展开文本静默发给模型。
    // 必须在登记 ActiveGuiRun 之前完成：失败路径不能留下幽灵 run。
    let mut content = {
        let core = adapter.core.read().await;
        core.expand_at_refs(Some(session_id), user_message)
            .await
            .map_err(GuiHostAdapter::app_error)?
    };
    if !attachment_ids.is_empty() {
        let pawork_protocol::CommandSource::LocalGui { client_id } = &envelope.source else {
            unreachable!()
        };
        content.extend(
            adapter
                .attachments
                .lock()
                .unwrap()
                .take_parts(client_id.as_str(), session_id, attachment_ids)
                .map_err(|message| GuiHostAdapter::host_error("invalid_attachment", message))?,
        );
    }
    content.extend(
        video_urls
            .iter()
            .cloned()
            .map(pawork_domain::ContentPart::Video),
    );
    if user_message.trim().is_empty() {
        content.retain(|part| match part {
            pawork_domain::ContentPart::Text(text) => !text.text.trim().is_empty(),
            _ => true,
        });
    }
    if user_message.trim().is_empty()
        && content.iter().all(|part| match part {
            pawork_domain::ContentPart::Text(text) => text.text.trim().is_empty(),
            _ => false,
        })
    {
        return Err(GuiHostAdapter::host_error(
            "empty_turn",
            "message or attachment is required",
        ));
    }
    let n = adapter.next_gui_run.fetch_add(1, Ordering::Relaxed);
    let run_id = RunId::from(format!("run-gui-{}-{n}", now_timestamp().as_unix_millis()));
    adapter.browser.bind(&run_id, &envelope.source);
    let token = CancellationToken::new();
    adapter.runs.register(
        ActiveGuiRun {
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            workspace_id,
            workspace_roots,
            started_at_ms: now_timestamp().as_unix_millis(),
        },
        token.clone(),
    );
    let core = Arc::clone(&adapter.core);
    let bus = Arc::clone(&adapter.bus);
    let runs = Arc::clone(&adapter.runs);
    let approvals = Arc::clone(&adapter.approvals);
    let browser = Arc::clone(&adapter.browser);
    let instance = adapter.instance.clone();
    let session = session_id.clone();
    let run = run_id.clone();
    let web_search = *web_search;
    let mut messages = history;
    messages.push(Message {
        id: MessageId::from("pending"),
        role: MessageRole::User,
        content,
        metadata: Default::default(),
    });
    adapter.runs.spawn(async move {
        let sink = super::goal::BudgetSink {
            inner: GuiBroadcastSink::new(Arc::clone(&bus), instance.clone()),
            limit: goal_budget,
            cancel: token.clone(),
            usage: std::sync::Mutex::new((0, 0)),
        };
        let outcome = {
            let core = core.read().await;
            core.chat_turn_with_budget(
                run.clone(),
                &session,
                messages,
                &sink,
                token,
                web_search,
                goal_budget,
            )
            .await
        };
        let succeeded = outcome.is_ok();
        if let Err(error) = outcome {
            let core = core.read().await;
            seal_run_without_terminal(&core, &bus, instance.clone(), &session, &run, &error).await;
        }
        approvals.clear_run(&run);
        runs.remove(&run);
        browser.unbind(&run);
        // 会话槽释放在 terminal 登记清理之前：后者是外部 settle 观测点，
        // 观测到清理完成时会话槽必然已可再占用（R-06）。
        runs.release_session(&session);
        bus.clear_terminal_reported(run.as_str());
        // 终态后继续在受 Host 管理的后台任务内命名，不阻塞终态事件。
        if succeeded && goal_budget.is_none() && !runs.is_stopping() {
            tokio::select! {
                biased;
                _ = runs.stopped() => {}
                _ = crate::gui_host::auto_title::auto_title_after_successful_run(
                    core, bus, instance, session,
                ) => {}
            }
        }
    });
    session_slot.disarm();
    Ok(AppResponse::Accepted {
        command_id: envelope.command_id.clone(),
        run_id: Some(run_id),
    })
}
