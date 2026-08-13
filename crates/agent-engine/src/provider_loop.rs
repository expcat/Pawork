//! Provider Loop（P3-3）—— Agent 循环的主干。
//!
//! 流式提交请求、解析 tool call、执行工具、回填 tool result、继续多轮，直到
//! 模型不再请求工具或达到最大迭代次数。本模块组合状态机（P3-1）、预算控制
//! （P3-6）、消息队列（P3-5）与事件广播（P3-9）。
//!
//! 工具执行与审批通过 trait 注入，既可接 `tool-runtime::ToolScheduler`（P3-4），
//! 也可在测试中用 Mock 注入，保持与调度器解耦。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use agent_domain::{
    CancellationToken, Message, MessageId, MessageMetadata, ModelId, RequestId, RunId, TokenUsage,
};
use agent_events::{AgentEvent, AgentEventEnvelope, EventSequence};
use provider_api::{
    CanonicalModelRequest, ModelProvider, ModelResponseSummary, ProviderError, ProviderEventSink,
    ProviderStreamEvent,
};
use thiserror::Error;

use crate::appender::{AssembledTurn, ToolCallResult};
use crate::broadcast::EventBroadcaster;
use crate::budget::{BudgetController, BudgetDimension, BudgetReport, ExternalQuotaSignal};
use crate::cancel::{CancelHandle, CancelReason};
use crate::queue::MessageQueue;
use crate::retry::{RetryController, RetryDecision, RetryPolicy};
use crate::state::{EventHint, RunState, RunStateMachine, RunTransition, TransitionError};

/// Agent Loop 执行中需要的回调集合（由调用方/宿主注入）。
///
/// 所有持久化、工具执行、审批、ID 生成都经由此 trait，使 Provider Loop 与
/// SQLite/Tool Scheduler/Event Store 解耦，便于单测与替换。
#[async_trait::async_trait]
pub trait LoopContext: Send + Sync {
    /// 执行一批 tool call，返回对应结果（顺序与输入一致）。
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult>;

    /// 请求用户审批一组 tool call；返回每个 call 的审批决策（顺序一致）。
    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        cancel: CancellationToken,
    ) -> Vec<ApprovalOutcome>;

    /// Provider-owned 调用的 transcript continuation hook。
    ///
    /// 实现必须完成 Hosted/Extension 的授权与 dispatch 接管；返回值刻意不含
    /// `ToolResult`。P15-5 将在此接口后接入 ServerToolEvent / transcript envelope。
    async fn dispatch_provider_calls(
        &self,
        calls: Vec<ProviderTranscriptInvocation>,
        _events: LoopEventEmitter,
        _cancel: CancellationToken,
    ) -> Result<(), tool_api::ToolError> {
        let names = calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(tool_api::ToolError {
            kind: tool_api::ToolErrorKind::Internal,
            message: format!("provider transcript hook is not configured for: {names}"),
            retryable: false,
            retry_after_ms: None,
        })
    }

    /// 生成新的 MessageId（保证唯一）。
    fn next_message_id(&self) -> MessageId;

    /// 生成新的 RequestId。
    fn next_request_id(&self) -> RequestId;

    /// 查询工具的执行位点（canonical `ToolKind`，不涉及 Provider 名称）。
    ///
    /// P15-1 路由依据：Core 只本地执行 `ClientFunction`；`ProviderHosted` /
    /// `ProviderExtension` 不本地执行、不生成本地 `ToolResult`。默认视为
    /// `ClientFunction`（旧宿主 / 测试行为不变）；调度器宿主按注册表覆盖。
    fn tool_kind(&self, _name: &str) -> agent_domain::ToolKind {
        agent_domain::ToolKind::ClientFunction
    }

    /// Pre-prompt hook point（P17-1）：每轮请求组装完成后、发送给 Provider 之前
    /// 调用，是 `PromptAssembled` 触发点的**权威回灌位点**。
    ///
    /// 实现可以改写 `request.messages`（PromptTransform 回灌）、注入判定
    /// （PromptEval / AgentEval / McpTool）或拒绝整轮（返回 `Err`）。默认
    /// no-op；不实现该方法的宿主行为不变。
    async fn pre_prompt(
        &self,
        request: &mut CanonicalModelRequest,
        events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Result<(), LoopError> {
        let _ = (request, events, cancel);
        Ok(())
    }

    /// Pre-tool hook point（P17-1）：审批通过后、本地工具执行之前调用，是
    /// `PreToolUse` 触发点的**权威回灌位点**。
    ///
    /// 实现可以从 `invocations` 中移除被拒绝的调用（移除项按审批拒绝语义回填
    /// denied 结果）或返回 `Err` 中止整轮。默认 no-op。
    async fn pre_tool(
        &self,
        invocations: &mut Vec<PendingToolInvocation>,
        events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Result<(), LoopError> {
        let _ = (invocations, events, cancel);
        Ok(())
    }
}

/// 待执行的一次工具调用（解析自本轮 tool call）。
#[derive(Clone, Debug)]
pub struct PendingToolInvocation {
    pub tool_call_id: agent_domain::ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// 已按 canonical kind 分流、必须由 Provider transcript 续接的调用。
#[derive(Clone, Debug)]
pub struct ProviderTranscriptInvocation {
    pub tool_call_id: agent_domain::ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
    pub kind: agent_domain::ToolKind,
}

impl ProviderTranscriptInvocation {
    /// 续接方式不可写，只能由 kind 推导。
    pub const fn continuation_mode(&self) -> agent_domain::ContinuationMode {
        self.kind.continuation_mode()
    }
}

/// 审批结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// 放行该工具。
    Approved,
    /// 拒绝该工具（回填拒绝结果后继续循环）。
    Denied,
}

/// Provider Loop 错误。
#[derive(Debug, Error)]
pub enum LoopError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("budget exhausted: {0:?}")]
    BudgetExceeded(BudgetReport),
    #[error("illegal state transition: {0}")]
    State(#[from] TransitionError),
    #[error("provider tool dispatch error: {0}")]
    ProviderCall(tool_api::ToolError),
    #[error("run cancelled")]
    Cancelled,
    #[error("run failed: {0}")]
    Failed(String),
}

/// 单轮模型调用的产出。
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    pub assistant_message: Message,
    /// 仅含 ClientFunction 的本地结果。
    pub tool_results: Vec<ToolCallResult>,
    /// 已交给 Provider transcript hook 的 Hosted/Extension 调用。
    pub provider_calls: Vec<ProviderTranscriptInvocation>,
    pub summary: ModelResponseSummary,
    /// 该轮结束后的 Run 状态。
    pub state: RunState,
    /// 该轮结束后的预算报告。
    pub budget: BudgetReport,
}

impl TurnOutcome {
    /// 本轮是否请求了工具（循环据此决定是否继续）。
    pub fn requests_tools(&self) -> bool {
        !self.tool_results.is_empty() || !self.provider_calls.is_empty()
    }
}

/// 把单轮 usage 饱和累计到 run 级累计值（任一维度溢出时按 u64::MAX 截断）。
fn saturating_add_usage(acc: &TokenUsage, round: &TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: acc.input_tokens.saturating_add(round.input_tokens),
        output_tokens: acc.output_tokens.saturating_add(round.output_tokens),
        cache_read_tokens: acc
            .cache_read_tokens
            .saturating_add(round.cache_read_tokens),
        cache_write_tokens: acc
            .cache_write_tokens
            .saturating_add(round.cache_write_tokens),
    }
}

/// Provider Loop 配置。
#[derive(Clone, Debug)]
pub struct ProviderLoopConfig {
    pub session_id: agent_domain::SessionId,
    pub run_id: RunId,
    pub provider_id: agent_domain::ProviderId,
    pub model: ModelId,
    /// 工具定义（随每次请求带给 Provider）。
    pub tools: Vec<provider_api::ToolDefinition>,
    /// ProviderHosted 工具声明（随请求带给 Provider；P15-1）。
    pub hosted_tools: Vec<provider_api::HostedToolRequest>,
    /// ProviderExtension 工具声明（随请求带给 Provider；P15-1）。
    pub extensions: Vec<provider_api::ExtensionToolRequest>,
    /// 初始对话历史（不含本轮触发消息）。
    pub initial_messages: Vec<Message>,
    /// 最大循环迭代次数（安全阀，防止模型无限请求工具）。
    pub max_iterations: u64,
    pub budget: crate::budget::BudgetLimits,
    pub retry: RetryPolicy,
    pub thinking: Option<provider_api::ThinkingConfig>,
    /// P15-8 canonical reasoning 请求；显式 effort 优先于 `thinking.level`，
    /// 流入 `CanonicalModelRequest.reasoning` 并驱动 CapabilityNegotiator。
    pub reasoning: Option<provider_api::ReasoningConfig>,
}

/// Provider Loop：执行单次 Agent 循环（可含多轮工具）。
///
/// 调用 [`ProviderLoop::run`] 会流式提交、解析 tool call、审批、执行、回填并
/// 继续下一轮，直到完成、取消或预算耗尽。所有状态转换与消息落库通过
/// [`EventSink`] 回调持久化，并通过 [`EventBroadcaster`] 广播。
pub struct ProviderLoop {
    provider: Arc<dyn ModelProvider>,
    context: Arc<dyn LoopContext>,
    config: ProviderLoopConfig,
    state: RunStateMachine,
    budget: BudgetController,
    broadcaster: EventBroadcaster,
    /// 下一个事件序号（同一 Session 内严格递增）。
    next_sequence: Arc<AtomicU64>,
    /// 已提交的消息历史（每轮追加，供下一轮请求使用）。
    messages: Vec<Message>,
    /// run 级累计 usage（每轮成功后饱和累加；终态 RunCompleted 与返回值使用）。
    run_usage: TokenUsage,
    started_at: Option<Instant>,
    warned_budget_dimensions: BTreeSet<BudgetDimension>,
}

impl ProviderLoop {
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        context: Arc<dyn LoopContext>,
        config: ProviderLoopConfig,
        start_sequence: u64,
        broadcaster: EventBroadcaster,
    ) -> Self {
        Self::new_with_external_quota(provider, context, config, start_sequence, broadcaster, None)
    }

    /// 创建 Provider Loop，并注入可选的供应商中立外部额度信号。
    ///
    /// 旧 [`ProviderLoop::new`] 保持兼容并以 `None` 委托到此入口；宿主可在后续
    /// 接线时传入 quota-service 归一后的 canonical 信号，而无需让本 crate
    /// 依赖 quota-service。
    pub fn new_with_external_quota(
        provider: Arc<dyn ModelProvider>,
        context: Arc<dyn LoopContext>,
        mut config: ProviderLoopConfig,
        start_sequence: u64,
        broadcaster: EventBroadcaster,
        external_quota: Option<ExternalQuotaSignal>,
    ) -> Self {
        let messages = config.initial_messages.clone();
        // 若 budget 未设迭代上限，用 config.max_iterations 作为安全阀，
        // 避免模型无限请求工具（与「预算控制」统一，不留两套并行死配置）。
        if config.budget.max_iterations.is_none() && config.max_iterations > 0 {
            config.budget.max_iterations = Some(config.max_iterations);
        }
        let mut budget = BudgetController::new(config.budget.clone());
        if let Some(signal) = external_quota {
            budget.set_external_quota(signal);
        }
        Self {
            provider,
            context,
            config,
            state: RunStateMachine::new(),
            budget,
            broadcaster,
            next_sequence: Arc::new(AtomicU64::new(start_sequence.max(1))),
            messages,
            run_usage: TokenUsage::default(),
            started_at: None,
            warned_budget_dimensions: BTreeSet::new(),
        }
    }

    /// 当前 Run 状态。
    pub fn state(&self) -> RunState {
        self.state.state()
    }

    /// 当前消息历史快照。
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// 运行循环直到完成、取消或预算耗尽。
    pub async fn run(
        &mut self,
        queue: Arc<MessageQueue>,
        cancel: CancelHandle,
    ) -> Result<(RunState, ModelResponseSummary), LoopError> {
        self.started_at = Some(Instant::now());
        // Created → PreparingContext → WaitingForProvider
        self.transition(RunTransition::Begin)?;
        self.transition(RunTransition::ContextPrepared)?;

        loop {
            if cancel.is_cancelled() {
                self.transition(RunTransition::Cancel)?;
                cancel.cancel(CancelReason::System);
                self.emit_terminal_payload(AgentEvent::RunCancelled {
                    reason: Some("cancelled before provider request".into()),
                });
                return Err(LoopError::Cancelled);
            }

            self.update_elapsed();
            let report = self.budget.tick_iteration();
            self.emit_budget_warnings(&report);
            if report.must_stop() {
                cancel.cancel(CancelReason::Budget);
                self.transition(RunTransition::Fail)?;
                self.emit_terminal_payload(AgentEvent::RunFailed {
                    error: ProviderError::clone_for_event(&LoopError::BudgetExceeded(
                        report.clone(),
                    )),
                });
                return Err(LoopError::BudgetExceeded(report));
            }

            // 执行一轮：WaitingForProvider → StreamingResponse → (CollectingToolCalls | Completed)
            let outcome = match self.run_turn(&cancel).await {
                Ok(outcome) => outcome,
                Err(LoopError::Cancelled) => {
                    cancel.cancel(CancelReason::System);
                    self.transition(RunTransition::Cancel)?;
                    self.emit_terminal_payload(AgentEvent::RunCancelled {
                        reason: Some("cancelled during provider or tool execution".into()),
                    });
                    return Err(LoopError::Cancelled);
                }
                Err(LoopError::Provider(err))
                    if err.kind == provider_api::ProviderErrorKind::Cancelled =>
                {
                    cancel.cancel(CancelReason::System);
                    self.transition(RunTransition::Cancel)?;
                    self.emit_terminal_payload(AgentEvent::RunCancelled {
                        reason: Some("provider stream cancelled".into()),
                    });
                    return Err(LoopError::Cancelled);
                }
                Err(err) => {
                    let reason = if matches!(&err, LoopError::BudgetExceeded(_)) {
                        CancelReason::Budget
                    } else {
                        CancelReason::System
                    };
                    cancel.cancel(reason);
                    self.transition(RunTransition::Fail)?;
                    self.emit_terminal_payload(AgentEvent::RunFailed {
                        error: ProviderError::clone_for_event(&err),
                    });
                    return Err(err);
                }
            };

            // 每轮成功后把该轮 usage 饱和累计到本 run；终态（RunCompleted 与
            // 返回的 ModelResponseSummary）使用累计值，MessageCommitted 的
            // metadata 仍保留单轮值。
            self.run_usage = saturating_add_usage(&self.run_usage, &outcome.summary.usage);

            // 先判断是否请求工具（借引用），再取走 summary。
            let requests_tools = outcome.requests_tools();
            let mut summary = outcome.summary;

            let queued = queue.drain_one().await;

            if !requests_tools {
                if let Some(queued) = queued {
                    self.transition(RunTransition::QueuedMessageAppended)?;
                    self.messages.push(queued.message.clone());
                    self.emit_message_committed(&queued.message);
                    continue;
                }
                self.transition(RunTransition::Complete)?;
                let usage = self.run_usage.clone();
                summary.usage = usage.clone();
                self.emit_terminal_payload(AgentEvent::RunCompleted {
                    stop_reason: summary.stop_reason.clone(),
                    usage,
                });
                return Ok((self.state.state(), summary));
            }

            if let Some(queued) = queued {
                self.messages.push(queued.message.clone());
                self.emit_message_committed(&queued.message);
            }
            // 已请求工具：回填结果后进入下一轮（run_turn 内部已处理审批/执行/回填）。
        }
    }

    /// 执行单轮：提交 Provider → 收集 → 审批/执行工具 → 回填结果。
    async fn run_turn(&mut self, cancel: &CancelHandle) -> Result<TurnOutcome, LoopError> {
        // WaitingForProvider → StreamingResponse
        self.transition(RunTransition::ProviderStarted)?;

        let mut request = self.build_request();
        let assistant_message_id = self.context.next_message_id();
        self.emit_payload(AgentEvent::ProviderRequestStarted {
            request_id: request.request_id.clone(),
            provider_id: self.config.provider_id.clone(),
            model: self.config.model.as_str().to_string(),
        });

        // P15-8：以证据 × 请求能力协商，发出稳定 `provider_capability_negotiated`
        // Diagnostic（可观测「为何降级」）。协商不触网、不读 Provider 名；缺证据
        // 时仅记录 chosen_transport=ChatCompletions，不放大任何能力。
        self.emit_capability_negotiated(&request);

        // P17-1 权威 pre-prompt 位点：改写/判定结果在请求发出前回灌进 request。
        // 拒绝返回 Err → 本轮终止（run 走既有的 Failed 收敛路径）。
        self.context
            .pre_prompt(&mut request, self.event_emitter(), cancel.token())
            .await?;

        let mut retry = RetryController::new(self.config.retry.clone());
        let (summary, sink) = loop {
            let sink = LoopSink::new(
                self.event_emitter(),
                assistant_message_id.clone(),
                request.request_id.clone(),
            );
            match self
                .provider
                .stream(request.clone(), &sink, cancel.token())
                .await
            {
                Ok(summary) => break (summary, sink),
                Err(err) => match retry.on_error(&err) {
                    RetryDecision::Retry {
                        attempt,
                        backoff,
                        reason,
                    } => {
                        self.emit_payload(AgentEvent::Diagnostic {
                            code: "provider_retry_attempt".into(),
                            details: serde_json::json!({
                                "attempt": attempt,
                                "reason": format!("{reason:?}"),
                                "backoff_ms": backoff.as_millis() as u64,
                                "request_id": request.request_id.as_str(),
                            }),
                        });
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = cancel.token().cancelled() => return Err(LoopError::Cancelled),
                        }
                    }
                    RetryDecision::Stop { .. } => return Err(LoopError::Provider(err)),
                },
            }
        };

        self.budget
            .record_tokens(summary.usage.input_tokens, summary.usage.output_tokens);
        let estimated_cost = model_registry::ModelRegistry::builtin()
            .estimate_cost(self.config.model.as_str(), &summary.usage);
        if let Some(cost) = &estimated_cost {
            self.budget.record_cost(cost.amount_micros);
        }
        self.check_budget()?;

        // 把流式增量累积成一条助手消息。
        let mut turn = AssembledTurn::new(assistant_message_id);
        for event in sink.drain_events() {
            turn.apply(&event);
        }
        turn.summary = Some(summary.clone());

        // 工具轮次立即进入 CollectingToolCalls；无工具轮次由 run 在检查消息队列后
        // 决定完成或继续，避免过早进入不可逆终态。
        if turn.has_tool_calls() {
            self.transition(RunTransition::StreamFinished {
                has_tool_calls: true,
            })?;
        }

        // 构建并提交助手消息。
        let metadata = MessageMetadata {
            usage: Some(summary.usage.clone()),
            stop_reason: Some(summary.stop_reason.clone()),
            provider: Some(self.config.provider_id.clone()),
            model: Some(self.config.model.clone()),
            cost: estimated_cost,
            ..MessageMetadata::default()
        };
        let assistant_message = turn.clone().into_message(metadata);
        self.messages.push(assistant_message.clone());
        self.emit_message_committed(&assistant_message);

        // 没有工具 → 返回（run 会完成）。
        if !turn.has_tool_calls() {
            return Ok(TurnOutcome {
                assistant_message,
                tool_results: Vec::new(),
                provider_calls: Vec::new(),
                summary,
                state: self.state.state(),
                budget: self.budget.check(),
            });
        }

        // 收集待执行 tool call（保持到达顺序）。
        let invocations: Vec<PendingToolInvocation> = turn
            .tool_call_order
            .iter()
            .filter_map(|id| turn.tool_calls.get(id).map(|c| (id, c)))
            .map(|(id, c)| PendingToolInvocation {
                tool_call_id: id.clone(),
                name: c.name.clone(),
                arguments: c.arguments(),
            })
            .collect();

        // P15-1：按 canonical 执行位点分流。Core 仅执行 ClientFunction；其余调用
        // 进入明确 transcript hook，不生成 ToolResult。
        let (invocations, provider_calls) = {
            let mut client = Vec::with_capacity(invocations.len());
            let mut provider = Vec::new();
            for inv in invocations {
                match self.declared_tool_kind(&inv.name) {
                    agent_domain::ToolKind::ClientFunction => client.push(inv),
                    kind => provider.push(ProviderTranscriptInvocation {
                        tool_call_id: inv.tool_call_id,
                        name: inv.name,
                        arguments: inv.arguments,
                        kind,
                    }),
                }
            }
            (client, provider)
        };

        if !provider_calls.is_empty() {
            self.context
                .dispatch_provider_calls(
                    provider_calls.clone(),
                    self.event_emitter(),
                    cancel.token(),
                )
                .await
                .map_err(|err| match err.kind {
                    tool_api::ToolErrorKind::Cancelled => LoopError::Cancelled,
                    _ => LoopError::ProviderCall(err),
                })?;
            // Provider-owned 调用同样计入工具预算（与本地执行一致）。
            for _ in &provider_calls {
                self.budget.record_tool_call();
            }
            self.check_budget()?;
        }

        // 全部为 Provider-owned：单步 CollectingToolCalls → WaitingForProvider，
        // 发出可重放的 ProviderTranscriptContinued 事件；不追加空 Tool 消息，
        // 等待 Provider 原生 transcript 续接（P15-5）。
        if invocations.is_empty() {
            self.transition(RunTransition::ProviderTranscriptContinued)?;
            self.emit_payload(AgentEvent::ProviderTranscriptContinued {
                calls: provider_calls
                    .iter()
                    .map(|call| agent_events::ProviderTranscriptContinuation {
                        tool_call_id: call.tool_call_id.clone(),
                        name: call.name.clone(),
                        kind: call.kind,
                    })
                    .collect(),
            });
            return Ok(TurnOutcome {
                assistant_message,
                tool_results: Vec::new(),
                provider_calls,
                summary,
                state: self.state.state(),
                budget: self.budget.check(),
            });
        }

        // 审批：请求用户决策。
        self.transition(RunTransition::ApprovalRequested)?;
        for inv in &invocations {
            self.emit_payload(AgentEvent::ToolApprovalRequested {
                tool_call_id: inv.tool_call_id.clone(),
                reason: format!("tool `{}` requires approval", inv.name),
            });
        }
        let approvals = self
            .context
            .request_approval(&invocations, cancel.token())
            .await;

        // 仅对 ClientFunction 按原序收集本地结果。
        let mut results: Vec<ToolCallResult> = Vec::with_capacity(invocations.len());
        let mut approved_slots: Vec<usize> = Vec::new();
        for (inv, outcome) in invocations.iter().zip(approvals.iter()) {
            self.emit_payload(AgentEvent::ToolApprovalResponded {
                tool_call_id: inv.tool_call_id.clone(),
                decision: match outcome {
                    ApprovalOutcome::Approved => agent_events::ApprovalDecision::ApprovedOnce,
                    ApprovalOutcome::Denied => agent_events::ApprovalDecision::Denied,
                },
                comment: None,
            });
            match outcome {
                ApprovalOutcome::Approved => {
                    self.budget.record_tool_call();
                    approved_slots.push(results.len());
                    results.push(placeholder_tool_result(inv));
                }
                ApprovalOutcome::Denied => {
                    results.push(denied_tool_result(inv));
                }
            }
        }

        // 审批通过 → 执行工具（按原序），回填到占位位置以保持顺序。
        if !approved_slots.is_empty() {
            self.budget.set_concurrency(approved_slots.len() as u64);
            self.check_budget()?;
            self.transition(RunTransition::ApprovalGranted)?;
            let mut approved: Vec<PendingToolInvocation> = approved_slots
                .iter()
                .map(|&i| invocations[i].clone())
                .collect();
            // P17-1 权威 pre-tool 位点：hook 拒绝的调用从执行列表移除，按
            // 审批拒绝语义回填 denied 结果（不执行、不获得结果）。
            let approved_before_hooks = approved.clone();
            self.context
                .pre_tool(&mut approved, self.event_emitter(), cancel.token())
                .await?;
            let denied_by_hooks: Vec<PendingToolInvocation> = approved_before_hooks
                .into_iter()
                .filter(|inv| !approved.iter().any(|kept| kept.tool_call_id == inv.tool_call_id))
                .collect();
            for inv in &approved {
                self.emit_payload(AgentEvent::ToolExecutionStarted {
                    tool_call_id: inv.tool_call_id.clone(),
                });
            }
            let executed = self
                .context
                .execute_tools(approved, self.event_emitter(), cancel.token())
                .await;
            self.budget.set_concurrency(0);
            if cancel.is_cancelled() {
                return Err(LoopError::Cancelled);
            }
            // hook 拒绝的调用：不执行，按审批拒绝语义回填 denied 结果。
            for denied in &denied_by_hooks {
                self.emit_payload(AgentEvent::ToolExecutionCompleted {
                    tool_call_id: denied.tool_call_id.clone(),
                    result: tool_result_content_view(&denied_tool_result(denied)),
                });
            }
            // 结果按 tool_call_id 回填到原调用槽位（pre_tool 过滤后 executed
            // 与 approved_slots 不再按位置对齐）。
            let mut executed_by_id: std::collections::BTreeMap<
                agent_domain::ToolCallId,
                ToolCallResult,
            > = executed.into_iter().map(|r| (r.tool_call_id.clone(), r)).collect();
            for slot in approved_slots.iter() {
                let inv = &invocations[*slot];
                if let Some(r) = executed_by_id.remove(&inv.tool_call_id) {
                    self.emit_payload(AgentEvent::ToolExecutionCompleted {
                        tool_call_id: r.tool_call_id.clone(),
                        result: tool_result_content_view(&r),
                    });
                    self.budget
                        .record_output(estimate_output_bytes(&r.result.content));
                    self.budget.record_artifact(
                        r.result
                            .artifacts
                            .iter()
                            .map(|artifact| artifact.byte_length)
                            .sum(),
                    );
                    results[*slot] = r;
                }
            }
            self.check_budget()?;
            self.transition(RunTransition::ToolsCompleted)?;
        } else {
            // 全部拒绝时，状态从 WaitingForApproval → AppendingToolResults
            self.transition(RunTransition::ApprovalDenied)?;
        }

        // 回填 tool result 消息。
        let tool_message =
            crate::appender::tool_results_message(self.context.next_message_id(), results.clone());
        self.messages.push(tool_message.clone());
        self.emit_message_committed(&tool_message);

        // AppendingToolResults → WaitingForProvider（下一轮）
        self.transition(RunTransition::ResultsAppended)?;

        Ok(TurnOutcome {
            assistant_message,
            tool_results: results,
            provider_calls,
            summary,
            state: self.state.state(),
            budget: self.budget.check(),
        })
    }

    fn build_request(&self) -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: self.context.next_request_id(),
            model: self.config.model.clone(),
            messages: self.messages.clone(),
            tools: self.config.tools.clone(),
            hosted_tools: self.config.hosted_tools.clone(),
            extensions: self.config.extensions.clone(),
            tool_choice: provider_api::ToolChoice::Auto,
            thinking: self.config.thinking.clone(),
            reasoning: self.config.reasoning.clone(),
            temperature: None,
            max_output_tokens: self.config.budget.max_output_tokens,
            stop_sequences: Vec::new(),
            response_format: provider_api::ResponseFormat::Text,
            prompt_cache: provider_api::PromptCachePreference::Automatic,
            budget: provider_api::RequestBudget::default(),
            provider_options: std::collections::BTreeMap::new(),
            trace_id: None,
        }
    }

    /// P15-8 能力协商诊断：从内置 registry 取证据，按 `reasoning` / hosted tools
    /// / citations 构造 `CapabilityRequirements`，调用 `CapabilityNegotiator`
    /// 后以稳定 `provider_capability_negotiated` Diagnostic 落入观测通道。
    ///
    /// 纯函数式协商，不触网、不读 Provider 名；无证据时仅记录基线 transport，
    /// 不放大任何能力。每轮复用同一记录便于排查「为何降级」。
    fn emit_capability_negotiated(&self, request: &CanonicalModelRequest) {
        use std::collections::BTreeSet;

        // 从 hosted_tools / extensions 收集请求要求的服务端工具标签。
        let mut required_tools: BTreeSet<agent_domain::ToolCapabilityTag> = BTreeSet::new();
        for hosted in &request.hosted_tools {
            required_tools.insert(hosted.kind);
        }
        for ext in &request.extensions {
            for cap in &ext.capabilities {
                required_tools.insert(*cap);
            }
        }

        let requirements = provider_api::CapabilityRequirements {
            transport_pref: Vec::new(),
            required_tools,
            reasoning: request.reasoning.clone(),
            citations: false,
        };

        // 证据来源：内置 registry（与费用估算同一来源）；缺证据时用空证据，
        // 协商器仍能给出 chosen_transport 与 requested/unsupported 快照。
        let registry = model_registry::ModelRegistry::builtin();
        let resolved = match registry.capability_evidence(request.model.as_str()) {
            Some(evidence) => provider_runtime::negotiate::CapabilityNegotiator::negotiate(
                &evidence,
                &requirements,
            ),
            None => {
                let empty = model_registry::CapabilityEvidence {
                    model: request.model.clone(),
                    provider: None,
                    static_declared: None,
                    probe_declared: None,
                    override_declared: None,
                };
                provider_runtime::negotiate::CapabilityNegotiator::negotiate(&empty, &requirements)
            }
        };

        self.emit_payload(AgentEvent::Diagnostic {
            code: "provider_capability_negotiated".into(),
            details: serde_json::json!({
                "model": request.model.as_str(),
                "chosen_transport": format!("{:?}", resolved.chosen_transport),
                "supported": resolved.supported.iter().cloned().collect::<Vec<_>>(),
                "unsupported": resolved.unsupported.iter().cloned().collect::<Vec<_>>(),
                "fallback": resolved
                    .fallback
                    .iter()
                    .map(|(k, v)| (k.clone(), format!("{v:?}")))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            }),
        });
    }

    /// 请求中的 canonical 声明是执行位点的权威来源；registry 仅补充宿主侧
    /// descriptor。若同名声明意外重叠，优先采用约束更严格的 Extension，避免
    /// 把 Provider-owned 调用降成 Core 本地执行。
    fn declared_tool_kind(&self, name: &str) -> agent_domain::ToolKind {
        if self.config.extensions.iter().any(|tool| tool.name == name) {
            agent_domain::ToolKind::ProviderExtension
        } else if self
            .config
            .hosted_tools
            .iter()
            .any(|tool| tool.name == name)
        {
            agent_domain::ToolKind::ProviderHosted
        } else {
            self.context.tool_kind(name)
        }
    }

    fn transition(&mut self, t: RunTransition) -> Result<(RunState, EventHint), TransitionError> {
        let result = self.state.apply(t)?;
        // 按事件 hint 自动补发「每次转换都有事件」契约所要求的事件。
        // ProviderRequestStarted 等携带额外载荷的事件由调用点显式 emit，
        // 这里只补 RunStarted / ContextPrepared（循环此前遗漏的两个）。
        let (_state, hint) = result;
        match hint {
            EventHint::RunStarted => {
                self.emit_payload(AgentEvent::RunStarted {
                    trigger_message_id: self
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m.role == agent_domain::MessageRole::User)
                        .map(|m| m.id.clone())
                        .unwrap_or_else(|| MessageId::from("trigger")),
                });
            }
            EventHint::ContextPrepared => {
                self.emit_payload(AgentEvent::ContextPrepared {
                    message_count: self.messages.len() as u64,
                    estimated_input_tokens: 0,
                });
            }
            EventHint::ProviderRequestStarted
            | EventHint::RunCompleted
            | EventHint::RunCancelled
            | EventHint::RunFailed
            | EventHint::MessageCommitted
            | EventHint::ToolApprovalRequested
            | EventHint::None => {}
        }
        Ok(result)
    }

    fn event_emitter(&self) -> LoopEventEmitter {
        LoopEventEmitter {
            session_id: self.config.session_id.clone(),
            run_id: self.config.run_id.clone(),
            broadcaster: self.broadcaster.clone(),
            next_sequence: self.next_sequence.clone(),
        }
    }

    fn update_elapsed(&mut self) {
        if let Some(started_at) = self.started_at {
            self.budget.set_elapsed(started_at.elapsed());
        }
    }

    fn check_budget(&mut self) -> Result<BudgetReport, LoopError> {
        self.update_elapsed();
        let report = self.budget.check();
        self.emit_budget_warnings(&report);
        if report.must_stop() {
            Err(LoopError::BudgetExceeded(report))
        } else {
            Ok(report)
        }
    }

    fn emit_budget_warnings(&mut self, report: &BudgetReport) {
        for dimension in &report.soft_warnings {
            if self.warned_budget_dimensions.insert(*dimension) {
                let mut details = serde_json::json!({
                    "dimension": dimension.as_str(),
                    "usage": self.budget.usage(),
                });
                if *dimension == BudgetDimension::ProviderQuota {
                    if let Some(note) = self.budget.quota_signal_note() {
                        details
                            .as_object_mut()
                            .expect("budget diagnostic details are an object")
                            .insert("quota_signal_note".into(), serde_json::Value::String(note));
                    }
                }
                self.emit_payload(AgentEvent::Diagnostic {
                    code: "budget_soft_limit".into(),
                    details,
                });
            }
        }
    }

    fn next_envelope(&self, payload: AgentEvent) -> AgentEventEnvelope {
        let sequence = EventSequence::new(self.next_sequence.fetch_add(1, Ordering::SeqCst));
        AgentEventEnvelope::new(
            agent_domain::EventId::from(format!("evt-{}-{}", self.config.run_id, sequence.value())),
            self.config.session_id.clone(),
            self.config.run_id.clone(),
            sequence,
            agent_domain::Timestamp::from_unix_millis(unix_millis_now()),
            payload,
        )
    }

    fn emit_payload(&self, payload: AgentEvent) {
        let envelope = self.next_envelope(payload);
        // 广播忽略无订阅者错误（核心不应因此中断）。
        let _ = self.broadcaster.publish(envelope);
    }

    fn emit_terminal_payload(&self, payload: AgentEvent) {
        self.emit_payload(payload);
    }

    fn emit_message_committed(&self, message: &Message) {
        self.emit_payload(AgentEvent::MessageCommitted {
            message: message.clone(),
        });
    }
}

/// 把 [`ProviderError`] 转成可放入事件的 [`ErrorContext`]（去敏感细节）。
///
/// `ProviderError` 实现了 `Clone`，这里复制一份用于事件化（循环的主错误路径
/// 仍返回原始 owned 错误给调用方）。
trait ProviderErrorExt {
    fn clone_for_event(err: &LoopError) -> agent_domain::ErrorContext;
}

impl ProviderErrorExt for ProviderError {
    fn clone_for_event(err: &LoopError) -> agent_domain::ErrorContext {
        match err {
            LoopError::Provider(e) => agent_domain::ErrorContext::from(e.clone()),
            LoopError::BudgetExceeded(report) => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::ResourceExhausted,
                message: format!("budget exceeded: {:?}", report.hard_exceeded),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            LoopError::State(e) => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Internal,
                message: e.to_string(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            LoopError::ProviderCall(error) => agent_domain::ErrorContext::from(error.clone()),
            LoopError::Cancelled => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Cancelled,
                message: "run cancelled".into(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
            LoopError::Failed(msg) => agent_domain::ErrorContext {
                category: agent_domain::ErrorCategory::Internal,
                message: msg.clone(),
                retryable: false,
                retry_after_ms: None,
                diagnostics: Default::default(),
            },
        }
    }
}

/// 粗估工具输出字节数（用于预算统计）。
fn estimate_output_bytes(content: &[agent_domain::ContentPart]) -> u64 {
    let serialized = serde_json::to_string(content).unwrap_or_default();
    serialized.len() as u64
}

/// 占位结果：保持 results 与 invocations 同序，执行后由真实结果回填。
fn placeholder_tool_result(inv: &PendingToolInvocation) -> ToolCallResult {
    ToolCallResult {
        tool_call_id: inv.tool_call_id.clone(),
        tool_name: inv.name.clone(),
        arguments: inv.arguments.clone(),
        result: tool_api::ToolResult::success(Vec::new()),
    }
}

/// 构造拒绝结果（不执行工具，直接回填错误结果）。
fn denied_tool_result(inv: &PendingToolInvocation) -> ToolCallResult {
    ToolCallResult {
        tool_call_id: inv.tool_call_id.clone(),
        tool_name: inv.name.clone(),
        arguments: inv.arguments.clone(),
        result: tool_api::ToolResult::failure(agent_domain::ErrorContext {
            category: agent_domain::ErrorCategory::Authorization,
            message: "tool call denied by user".into(),
            retryable: false,
            retry_after_ms: None,
            diagnostics: Default::default(),
        }),
    }
}

/// 从 [`ToolCallResult`] 构造用于事件的可序列化视图（借用，不 move）。
fn tool_result_content_view(r: &ToolCallResult) -> agent_domain::ToolResultContent {
    agent_domain::ToolResultContent {
        tool_call_id: r.tool_call_id.clone(),
        tool_name: Some(r.tool_name.clone()),
        content: r.result.content.clone(),
        is_error: r.result.is_error(),
        metadata: r.result.metadata.clone(),
    }
}

fn unix_millis_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 可克隆的 Loop 事件发射器；Provider 与 Tool 的流式 sink 共用同一序列源。
#[derive(Clone)]
pub struct LoopEventEmitter {
    session_id: agent_domain::SessionId,
    run_id: RunId,
    broadcaster: EventBroadcaster,
    next_sequence: Arc<AtomicU64>,
}

impl LoopEventEmitter {
    fn emit(&self, payload: AgentEvent) {
        let sequence = EventSequence::new(self.next_sequence.fetch_add(1, Ordering::SeqCst));
        let envelope = AgentEventEnvelope::new(
            agent_domain::EventId::from(format!("evt-{}-{}", self.run_id, sequence.value())),
            self.session_id.clone(),
            self.run_id.clone(),
            sequence,
            agent_domain::Timestamp::from_unix_millis(unix_millis_now()),
            payload,
        );
        let _ = self.broadcaster.publish(envelope);
    }

    pub fn emit_tool_event(
        &self,
        tool_call_id: agent_domain::ToolCallId,
        event: tool_api::ToolStreamEvent,
    ) {
        match event {
            tool_api::ToolStreamEvent::OutputDelta { channel, delta } => {
                let stream = match channel {
                    tool_api::ToolOutputChannel::Stdout => agent_events::ToolOutputStream::Stdout,
                    tool_api::ToolOutputChannel::Stderr => agent_events::ToolOutputStream::Stderr,
                    tool_api::ToolOutputChannel::Structured => {
                        agent_events::ToolOutputStream::Structured
                    }
                };
                self.emit(AgentEvent::ToolOutputDelta {
                    tool_call_id,
                    stream,
                    delta,
                });
            }
            tool_api::ToolStreamEvent::Progress { .. }
            | tool_api::ToolStreamEvent::ArtifactAvailable(_) => {}
        }
    }
}

/// 内部 sink：缓存 Provider 流式事件供 loop 累积，并同步广播 canonical delta。
struct LoopSink {
    events: std::sync::Mutex<Vec<ProviderStreamEvent>>,
    emitter: LoopEventEmitter,
    message_id: MessageId,
    _request_id: RequestId,
}

impl LoopSink {
    fn new(emitter: LoopEventEmitter, message_id: MessageId, request_id: RequestId) -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
            emitter,
            message_id,
            _request_id: request_id,
        }
    }

    fn drain_events(&self) -> Vec<ProviderStreamEvent> {
        std::mem::take(&mut *self.events.lock().expect("loop sink mutex"))
    }
}

#[async_trait::async_trait]
impl ProviderEventSink for LoopSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        let payload = match &event {
            ProviderStreamEvent::TextDelta(delta) => Some(AgentEvent::AssistantTextDelta {
                message_id: self.message_id.clone(),
                delta: delta.clone(),
            }),
            ProviderStreamEvent::ThinkingDelta(delta) => Some(AgentEvent::AssistantThinkingDelta {
                message_id: self.message_id.clone(),
                delta: delta.clone(),
            }),
            ProviderStreamEvent::ToolCallStarted { id, name } => {
                Some(AgentEvent::ToolCallStarted {
                    tool_call_id: id.clone(),
                    name: name.clone(),
                })
            }
            ProviderStreamEvent::ToolCallArgumentsDelta { id, json } => {
                Some(AgentEvent::ToolCallArgumentsDelta {
                    tool_call_id: id.clone(),
                    json_delta: json.clone(),
                })
            }
            ProviderStreamEvent::UsageUpdated(usage) => Some(AgentEvent::UsageUpdated {
                usage: usage.clone(),
            }),
            // P15-5：Provider 归一后的 server tool 事件与 transcript 信封直接进入
            // canonical 事件流（不参与本地消息组装、不生成 ToolResult）。
            ProviderStreamEvent::ServerTool(event) => Some(AgentEvent::ServerTool(event.clone())),
            ProviderStreamEvent::TranscriptEnvelope(envelope) => {
                Some(AgentEvent::TranscriptEnvelope(envelope.clone()))
            }
            _ => None,
        };
        if let Some(payload) = payload {
            self.emitter.emit(payload);
        }
        self.events.lock().expect("loop sink mutex").push(event);
        Ok(())
    }
}

/// 将 [`tool_runtime::ToolScheduler`] 适配为 Provider Loop 的工具执行上下文。
pub struct SchedulerLoopContext {
    scheduler: Arc<tool_runtime::ToolScheduler>,
    execution_context: tool_api::ToolExecutionContext,
    approval: Arc<dyn tool_runtime::ApprovalResolver>,
    msg_counter: AtomicU64,
    req_counter: AtomicU64,
}

impl SchedulerLoopContext {
    pub fn new(
        scheduler: Arc<tool_runtime::ToolScheduler>,
        execution_context: tool_api::ToolExecutionContext,
        approval: Arc<dyn tool_runtime::ApprovalResolver>,
    ) -> Self {
        Self {
            scheduler,
            execution_context,
            approval,
            msg_counter: AtomicU64::new(0),
            req_counter: AtomicU64::new(0),
        }
    }

    pub fn execution_context(&self) -> &tool_api::ToolExecutionContext {
        &self.execution_context
    }
}

struct SchedulerToolSink {
    tool_call_id: agent_domain::ToolCallId,
    events: LoopEventEmitter,
}

#[async_trait::async_trait]
impl tool_api::ToolEventSink for SchedulerToolSink {
    async fn emit(&self, event: tool_api::ToolStreamEvent) -> Result<(), tool_api::ToolError> {
        self.events
            .emit_tool_event(self.tool_call_id.clone(), event);
        Ok(())
    }
}

#[async_trait::async_trait]
impl LoopContext for SchedulerLoopContext {
    fn tool_kind(&self, name: &str) -> agent_domain::ToolKind {
        // 未注册工具视为 ClientFunction：交给调度器走既有 NotFound 路径。
        self.scheduler
            .kind_of(name)
            .unwrap_or(agent_domain::ToolKind::ClientFunction)
    }

    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult> {
        let futures = calls.into_iter().map(|call| {
            let scheduler = self.scheduler.clone();
            let context = self.execution_context.clone();
            let approval = self.approval.clone();
            let cancel = cancel.clone();
            let events = events.clone();
            async move {
                let request = tool_api::ToolRequest {
                    tool_call_id: call.tool_call_id.clone(),
                    input: call.arguments.clone(),
                };
                let sink = SchedulerToolSink {
                    tool_call_id: call.tool_call_id.clone(),
                    events,
                };
                let result = scheduler
                    .execute_named(
                        &call.name,
                        request,
                        context,
                        cancel,
                        approval.as_ref(),
                        &sink,
                    )
                    .await
                    .unwrap_or_else(|error| {
                        tool_api::ToolResult::failure(agent_domain::ErrorContext::from(error))
                    });
                ToolCallResult {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.name,
                    arguments: call.arguments,
                    result,
                }
            }
        });
        futures::future::join_all(futures).await
    }

    async fn dispatch_provider_calls(
        &self,
        calls: Vec<ProviderTranscriptInvocation>,
        _events: LoopEventEmitter,
        cancel: CancellationToken,
    ) -> Result<(), tool_api::ToolError> {
        for call in calls {
            let ProviderTranscriptInvocation {
                tool_call_id,
                name,
                arguments,
                kind,
            } = call;
            let dispatch = self
                .scheduler
                .authorize_provider_call(
                    &name,
                    tool_api::ToolRequest {
                        tool_call_id,
                        input: arguments,
                    },
                    cancel.clone(),
                    self.approval.as_ref(),
                )
                .await?;
            if dispatch.descriptor().kind != kind {
                return Err(tool_api::ToolError {
                    kind: tool_api::ToolErrorKind::Internal,
                    message: format!(
                        "tool `{}` changed execution kind while dispatching",
                        dispatch.descriptor().name
                    ),
                    retryable: false,
                    retry_after_ms: None,
                });
            }
        }
        Ok(())
    }

    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        _cancel: CancellationToken,
    ) -> Vec<ApprovalOutcome> {
        calls.iter().map(|_| ApprovalOutcome::Approved).collect()
    }

    fn next_message_id(&self) -> MessageId {
        let value = self.msg_counter.fetch_add(1, Ordering::Relaxed);
        MessageId::from(format!("{}-message-{value}", self.execution_context.run_id))
    }

    fn next_request_id(&self) -> RequestId {
        let value = self.req_counter.fetch_add(1, Ordering::Relaxed);
        RequestId::from(format!("{}-request-{value}", self.execution_context.run_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use agent_domain::{ContentPart, TextContent};
    use agent_domain::{StopReason, TokenUsage};
    use test_support::{MockProvider, MockScript, MockTool};
    use tool_api::AgentTool;
    use tool_api::ToolResult;

    #[derive(Clone)]
    struct SequenceProvider {
        phases: Arc<Vec<Arc<MockProvider>>>,
        calls: Arc<AtomicU64>,
        requests: Arc<Mutex<Vec<CanonicalModelRequest>>>,
    }

    impl SequenceProvider {
        fn new(scripts: Vec<MockScript>) -> Self {
            Self {
                phases: Arc::new(
                    scripts
                        .into_iter()
                        .map(|script| Arc::new(MockProvider::new(script)))
                        .collect(),
                ),
                calls: Arc::new(AtomicU64::new(0)),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<CanonicalModelRequest> {
            self.requests.lock().expect("requests").clone()
        }
    }

    #[async_trait::async_trait]
    impl ModelProvider for SequenceProvider {
        fn id(&self) -> agent_domain::ProviderId {
            agent_domain::ProviderId::from("sequence")
        }

        async fn list_models(
            &self,
            _credential: Option<&provider_api::ResolvedCredential>,
        ) -> Result<Vec<provider_api::ModelDefinition>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream(
            &self,
            request: CanonicalModelRequest,
            sink: &dyn ProviderEventSink,
            cancel: CancellationToken,
        ) -> Result<ModelResponseSummary, ProviderError> {
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            let index = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
            let phase = self
                .phases
                .get(index)
                .or_else(|| self.phases.last())
                .expect("sequence provider requires at least one phase");
            phase.stream(request, sink, cancel).await
        }
    }

    /// 测试用 LoopContext：自动审批、直接执行内置 MockTool。
    struct TestContext {
        tools: Mutex<Vec<Arc<MockTool>>>,
        msg_counter: AtomicU64,
        req_counter: AtomicU64,
    }

    impl TestContext {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                tools: Mutex::new(tools.into_iter().map(Arc::new).collect()),
                req_counter: AtomicU64::new(0),
                msg_counter: AtomicU64::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LoopContext for TestContext {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            _events: LoopEventEmitter,
            _cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            let tools = self.tools.lock().expect("tools").clone();
            let mut results = Vec::new();
            for call in calls {
                let tool = tools
                    .iter()
                    .find(|t| t.descriptor().name == call.name)
                    .cloned();
                let result = if let Some(tool) = tool {
                    let req = tool_api::ToolRequest {
                        tool_call_id: call.tool_call_id.clone(),
                        input: call.arguments.clone(),
                    };
                    let ctx = tool_api::ToolExecutionContext {
                        workspace_id: agent_domain::WorkspaceId::from("ws"),
                        run_id: RunId::from("run"),
                        working_directory: None,
                    };
                    let sink = test_support::RecordingToolSink::default();
                    tool.execute(req, ctx, &sink, CancellationToken::new())
                        .await
                        .unwrap_or_else(|e| {
                            ToolResult::failure(agent_domain::ErrorContext::from(e))
                        })
                } else {
                    ToolResult::failure(agent_domain::ErrorContext {
                        category: agent_domain::ErrorCategory::NotFound,
                        message: format!("unknown tool {}", call.name),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    })
                };
                results.push(ToolCallResult {
                    tool_call_id: call.tool_call_id,
                    tool_name: call.name,
                    arguments: call.arguments,
                    result,
                });
            }
            results
        }

        async fn request_approval(
            &self,
            _calls: &[PendingToolInvocation],
            _cancel: CancellationToken,
        ) -> Vec<ApprovalOutcome> {
            // 测试默认全部放行。
            _calls.iter().map(|_| ApprovalOutcome::Approved).collect()
        }

        fn next_message_id(&self) -> MessageId {
            let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
            MessageId::from(format!("msg-{n}"))
        }

        fn next_request_id(&self) -> RequestId {
            let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
            RequestId::from(format!("req-{n}"))
        }
    }

    fn config(messages: Vec<Message>) -> ProviderLoopConfig {
        ProviderLoopConfig {
            session_id: agent_domain::SessionId::from("session-1"),
            run_id: RunId::from("run-1"),
            provider_id: agent_domain::ProviderId::from("mock"),
            model: ModelId::from("mock-model"),
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            initial_messages: messages,
            max_iterations: 10,
            budget: crate::budget::BudgetLimits {
                max_iterations: Some(10),
                ..Default::default()
            },
            retry: RetryPolicy {
                initial_backoff: std::time::Duration::ZERO,
                max_backoff: std::time::Duration::ZERO,
                jitter: 0.0,
                ..RetryPolicy::default()
            },
            thinking: None,
            reasoning: None,
        }
    }

    #[test]
    fn canonical_request_declarations_are_authoritative_for_tool_kind() {
        let mut cfg = config(vec![user_message("classify")]);
        cfg.hosted_tools.push(provider_api::HostedToolRequest {
            name: "shared".into(),
            kind: tool_api::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        cfg.hosted_tools.push(provider_api::HostedToolRequest {
            name: "hosted_only".into(),
            kind: tool_api::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        cfg.extensions.push(provider_api::ExtensionToolRequest {
            name: "shared".into(),
            reference: "connector:test".into(),
            description: String::new(),
            capabilities: Vec::new(),
            requires_approval: true,
        });
        let provider = Arc::new(MockProvider::new(MockScript::new().text("done").complete()));
        let context = Arc::new(TestContext::new(Vec::new()));
        let engine = ProviderLoop::new(provider, context, cfg, 1, EventBroadcaster::new());

        assert_eq!(
            engine.declared_tool_kind("shared"),
            agent_domain::ToolKind::ProviderExtension,
            "overlap must fail closed to the stricter provider-owned site"
        );
        assert_eq!(
            engine.declared_tool_kind("hosted_only"),
            agent_domain::ToolKind::ProviderHosted
        );
        assert_eq!(
            engine.declared_tool_kind("ordinary_client"),
            agent_domain::ToolKind::ClientFunction
        );
    }

    fn run_cancel() -> CancelHandle {
        CancelHandle::new(
            RunId::from("run-1"),
            Arc::new(crate::NoopProcessTreeCleaner),
        )
    }

    fn message_queue() -> Arc<MessageQueue> {
        Arc::new(MessageQueue::new())
    }

    fn user_message(text: &str) -> Message {
        Message {
            id: MessageId::from("user-1"),
            role: agent_domain::MessageRole::User,
            content: vec![ContentPart::Text(TextContent { text: text.into() })],
            metadata: MessageMetadata::default(),
        }
    }

    fn provider_owned_descriptor(
        name: &str,
        kind: agent_domain::ToolKind,
    ) -> tool_api::ToolDescriptor {
        let hosting = match kind {
            agent_domain::ToolKind::ProviderHosted => tool_api::ToolHosting::ProviderHosted {
                hosted_name: name.into(),
                kind: tool_api::ToolCapabilityTag::WebSearch,
            },
            agent_domain::ToolKind::ProviderExtension => tool_api::ToolHosting::ProviderExtension {
                reference: "connector:test".into(),
            },
            agent_domain::ToolKind::ClientFunction => panic!("provider-owned descriptor required"),
        };
        tool_api::ToolDescriptor {
            name: name.into(),
            description: "provider-owned test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
            capability: tool_api::ToolCapability::Network,
            kind,
            hosting,
            capabilities: Vec::new(),
            requires_approval: kind == agent_domain::ToolKind::ProviderExtension,
            read_only: true,
            supports_concurrency: true,
            default_timeout_ms: None,
            max_output_bytes: 1024,
            allowed_in_untrusted_workspace: kind == agent_domain::ToolKind::ProviderHosted,
        }
    }

    fn scheduler_context(
        registry: tool_runtime::ToolRegistry,
        workspace_trusted: bool,
        approval: Arc<dyn tool_runtime::ApprovalResolver>,
    ) -> Arc<dyn LoopContext> {
        Arc::new(SchedulerLoopContext::new(
            Arc::new(tool_runtime::ToolScheduler::new(
                registry,
                tool_runtime::ToolSchedulerConfig {
                    max_concurrent: 4,
                    approval_mode: tool_runtime::ApprovalMode::NeverAsk,
                    workspace_trusted,
                },
            )),
            tool_api::ToolExecutionContext {
                workspace_id: agent_domain::WorkspaceId::from("workspace-routing"),
                run_id: RunId::from("run-1"),
                working_directory: None,
            },
            approval,
        ))
    }

    struct CountingExplicitResolver {
        calls: Arc<AtomicU64>,
    }

    #[async_trait::async_trait]
    impl tool_runtime::ApprovalResolver for CountingExplicitResolver {
        async fn resolve(
            &self,
            requests: &[tool_api::ToolRequest],
        ) -> Vec<tool_runtime::ApprovalOutcome> {
            self.calls
                .fetch_add(requests.len() as u64, Ordering::SeqCst);
            requests
                .iter()
                .map(|_| tool_runtime::ApprovalOutcome::Approved)
                .collect()
        }
    }

    #[tokio::test]
    async fn mock_provider_completes_without_tools() {
        let script = MockScript::new()
            .text("Hello!")
            .usage(TokenUsage {
                input_tokens: 5,
                output_tokens: 3,
                ..Default::default()
            })
            .complete();
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(script));
        let context: Arc<dyn LoopContext> = Arc::new(TestContext::new(Vec::new()));
        let broadcaster = EventBroadcaster::new();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("hi")]),
            1,
            broadcaster,
        );

        let (state, summary) = engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(state, RunState::Completed);
        assert_eq!(summary.stop_reason, StopReason::Completed);
        // 历史：user + assistant
        assert_eq!(engine.messages().len(), 2);
    }

    #[tokio::test]
    async fn all_hosted_calls_continue_via_transcript_without_tool_result() {
        let provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("web_search", serde_json::json!({"query": "pawork"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]);
        let provider_view = provider.clone();
        let mut registry = tool_runtime::ToolRegistry::new();
        registry
            .register_descriptor(provider_owned_descriptor(
                "web_search",
                agent_domain::ToolKind::ProviderHosted,
            ))
            .unwrap();
        assert!(registry.get("web_search").is_none());
        let context =
            scheduler_context(registry, true, Arc::new(tool_runtime::AutoApproveResolver));
        let mut cfg = config(vec![user_message("search")]);
        cfg.hosted_tools.push(provider_api::HostedToolRequest {
            name: "web_search".into(),
            kind: tool_api::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: vec![tool_api::ToolCapabilityTag::WebSearch],
            config: None,
        });
        let mut engine =
            ProviderLoop::new(Arc::new(provider), context, cfg, 1, EventBroadcaster::new());

        let (state, _) = engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(state, RunState::Completed);
        assert_eq!(
            provider_view.requests().len(),
            2,
            "hosted call must continue"
        );
        assert!(engine
            .messages()
            .iter()
            .all(|message| message.role != agent_domain::MessageRole::Tool));
        assert_eq!(provider_view.requests()[0].hosted_tools.len(), 1);
    }

    #[tokio::test]
    async fn mixed_client_and_hosted_calls_only_append_client_tool_result() {
        let provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("echo", serde_json::json!({"text": "hi"}))
                .tool_call("web_search", serde_json::json!({"query": "pawork"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]);
        let provider_view = provider.clone();
        let client = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "hi".into() })]),
        );
        let mut registry = tool_runtime::ToolRegistry::new();
        registry
            .register(Arc::new(client.clone()))
            .expect("client tool registers");
        registry
            .register_descriptor(provider_owned_descriptor(
                "web_search",
                agent_domain::ToolKind::ProviderHosted,
            ))
            .unwrap();
        let context =
            scheduler_context(registry, true, Arc::new(tool_runtime::AutoApproveResolver));
        let mut cfg = config(vec![user_message("mixed")]);
        cfg.tools.push(provider_api::ToolDefinition {
            name: "echo".into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
        });
        cfg.hosted_tools.push(provider_api::HostedToolRequest {
            name: "web_search".into(),
            kind: tool_api::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: vec![tool_api::ToolCapabilityTag::WebSearch],
            config: None,
        });
        let mut engine =
            ProviderLoop::new(Arc::new(provider), context, cfg, 1, EventBroadcaster::new());

        engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(client.calls().len(), 1);
        assert_eq!(provider_view.requests().len(), 2);
        let result_names: Vec<&str> = engine
            .messages()
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|part| match part {
                ContentPart::ToolResult(result) => result.tool_name.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(result_names, vec!["echo"]);
    }

    #[tokio::test]
    async fn reasoning_reference_is_carried_into_the_next_canonical_request() {
        let reasoning = agent_domain::ReasoningItem {
            id: agent_domain::ReasoningItemId::from("reasoning-1"),
            summary: Some("safe summary".into()),
            protected_blob_ref: agent_domain::ProtectedBlobRef::from("protected-1"),
            opaque_metadata: Default::default(),
            continuation_metadata: Default::default(),
        };
        let provider = SequenceProvider::new(vec![
            MockScript::new()
                .reasoning_item(reasoning.clone())
                .tool_call("echo", serde_json::json!({"text": "continue"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]);
        let provider_view = provider.clone();
        let client = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "continue".into(),
            })]),
        );
        let mut registry = tool_runtime::ToolRegistry::new();
        registry.register(Arc::new(client)).expect("register echo");
        let context =
            scheduler_context(registry, true, Arc::new(tool_runtime::AutoApproveResolver));
        let mut cfg = config(vec![user_message("reason across turns")]);
        cfg.tools.push(provider_api::ToolDefinition {
            name: "echo".into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
        });
        let mut engine =
            ProviderLoop::new(Arc::new(provider), context, cfg, 1, EventBroadcaster::new());

        engine.run(message_queue(), run_cancel()).await.unwrap();

        let requests = provider_view.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].messages.iter().any(|message| {
            message
                .content
                .iter()
                .any(|part| part == &ContentPart::Reasoning(reasoning.clone()))
        }));
    }

    #[tokio::test]
    async fn p15_8_capability_negotiated_diagnostic_and_reasoning_flow() {
        // 动态从内置 registry 选一个声明 thinking=true 的模型（不硬编码具体
        // provider 模型名，避免触碰 no_provider_branch 源码扫描）。
        let thinking_model = model_registry::ModelRegistry::builtin()
            .list()
            .into_iter()
            .find(|entry| entry.capabilities.thinking)
            .map(|entry| entry.id.clone())
            .expect("builtin registry 至少有一个 thinking 模型");
        let provider = SequenceProvider::new(vec![MockScript::new().text("ok").complete()]);
        let provider_view = provider.clone();
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("negotiate")]);
        cfg.model = thinking_model.clone();
        cfg.reasoning = Some(provider_api::ReasoningConfig::new(
            provider_api::ReasoningEffort::High,
        ));
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );
        engine.run(message_queue(), run_cancel()).await.unwrap();

        // CanonicalModelRequest 携带 reasoning（流到 Provider）。
        let requests = provider_view.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].reasoning.as_ref().map(|r| r.effort),
            Some(provider_api::ReasoningEffort::High),
            "reasoning 必须流入 CanonicalModelRequest"
        );

        // provider_capability_negotiated Diagnostic 至少出现一次，含 chosen_transport
        // 与 supported/unsupported 字段；thinking 模型 → reasoning 进 supported。
        let mut found = false;
        while let Ok(Some(event)) = sub.try_recv() {
            if let AgentEvent::Diagnostic { code, details } = event.payload {
                if code == "provider_capability_negotiated" {
                    found = true;
                    assert_eq!(details["model"], thinking_model.as_str());
                    assert!(details["chosen_transport"].is_string());
                    let supported = details["supported"].as_array().unwrap();
                    assert!(
                        supported.iter().any(|v| v == "reasoning"),
                        "reasoning 应进 supported: {supported:?}"
                    );
                    let fallback = details["fallback"].as_object().unwrap();
                    assert!(
                        !fallback.contains_key("reasoning"),
                        "不应 reject 已支持的 reasoning: {fallback:?}"
                    );
                }
            }
        }
        assert!(found, "必须发射 provider_capability_negotiated Diagnostic");
    }

    #[tokio::test]
    async fn p15_8_capability_negotiated_marks_unsupported_reasoning_fail_closed() {
        // 动态选一个 thinking=false 的基线模型；请求 reasoning 必须进 unsupported
        // 且 fallback 记录 Reject（fail-closed，不静默丢弃）。
        let baseline_model = model_registry::ModelRegistry::builtin()
            .list()
            .into_iter()
            .find(|entry| !entry.capabilities.thinking)
            .map(|entry| entry.id.clone())
            .expect("builtin registry 至少有一个基线模型");
        let provider = SequenceProvider::new(vec![MockScript::new().text("ok").complete()]);
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("negotiate baseline")]);
        cfg.model = baseline_model.clone();
        cfg.reasoning = Some(provider_api::ReasoningConfig::new(
            provider_api::ReasoningEffort::Medium,
        ));
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );
        engine.run(message_queue(), run_cancel()).await.unwrap();

        let mut found = false;
        while let Ok(Some(event)) = sub.try_recv() {
            if let AgentEvent::Diagnostic { code, details } = event.payload {
                if code == "provider_capability_negotiated" {
                    found = true;
                    let unsupported = details["unsupported"].as_array().unwrap();
                    assert!(
                        unsupported.iter().any(|v| v == "reasoning"),
                        "未声明 reasoning 必须进 unsupported: {unsupported:?}"
                    );
                    assert!(details["fallback"]["reasoning"].is_string());
                }
            }
        }
        assert!(found);
    }

    #[tokio::test]
    async fn extension_call_requires_approval_and_never_appends_tool_result() {
        let provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("remote_mcp", serde_json::json!({"action": "read"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]);
        let provider_view = provider.clone();
        let mut registry = tool_runtime::ToolRegistry::new();
        registry
            .register_descriptor(provider_owned_descriptor(
                "remote_mcp",
                agent_domain::ToolKind::ProviderExtension,
            ))
            .unwrap();
        assert!(registry.get("remote_mcp").is_none());
        let approval_calls = Arc::new(AtomicU64::new(0));
        let context = scheduler_context(
            registry,
            true,
            Arc::new(CountingExplicitResolver {
                calls: approval_calls.clone(),
            }),
        );
        let mut cfg = config(vec![user_message("extension")]);
        cfg.extensions.push(provider_api::ExtensionToolRequest {
            name: "remote_mcp".into(),
            reference: "connector:test".into(),
            description: String::new(),
            capabilities: vec![tool_api::ToolCapabilityTag::ServerSideMcp],
            requires_approval: true,
        });
        let mut engine =
            ProviderLoop::new(Arc::new(provider), context, cfg, 1, EventBroadcaster::new());

        engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(approval_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider_view.requests().len(), 2);
        assert!(engine
            .messages()
            .iter()
            .all(|message| message.role != agent_domain::MessageRole::Tool));
        assert_eq!(provider_view.requests()[0].extensions.len(), 1);
    }

    #[tokio::test]
    async fn extension_deny_fails_closed_without_tool_result() {
        let provider = SequenceProvider::new(vec![MockScript::new()
            .tool_call("remote_mcp", serde_json::json!({"action": "read"}))
            .complete_with(StopReason::ToolUse)]);
        let provider_view = provider.clone();
        let mut registry = tool_runtime::ToolRegistry::new();
        registry
            .register_descriptor(provider_owned_descriptor(
                "remote_mcp",
                agent_domain::ToolKind::ProviderExtension,
            ))
            .unwrap();
        // 未信任 workspace：Extension 无条件拒绝。
        let context =
            scheduler_context(registry, false, Arc::new(tool_runtime::AutoApproveResolver));
        let mut cfg = config(vec![user_message("extension")]);
        cfg.extensions.push(provider_api::ExtensionToolRequest {
            name: "remote_mcp".into(),
            reference: "connector:test".into(),
            description: String::new(),
            capabilities: vec![tool_api::ToolCapabilityTag::ServerSideMcp],
            requires_approval: true,
        });
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(Arc::new(provider), context, cfg, 1, broadcaster);

        let result = engine.run(message_queue(), run_cancel()).await;
        assert!(
            matches!(result, Err(LoopError::ProviderCall(_))),
            "denied extension must fail the run, got {result:?}"
        );
        assert_eq!(engine.state(), RunState::Failed);
        assert_eq!(
            provider_view.requests().len(),
            1,
            "denied extension must not continue to another provider round"
        );
        assert!(engine
            .messages()
            .iter()
            .all(|message| message.role != agent_domain::MessageRole::Tool));
        let mut failed = false;
        while let Ok(Some(event)) = sub.try_recv() {
            failed |= matches!(event.payload, AgentEvent::RunFailed { .. });
        }
        assert!(failed, "fail-closed extension must broadcast RunFailed");
    }

    struct CancelDuringDispatch;

    #[async_trait::async_trait]
    impl LoopContext for CancelDuringDispatch {
        async fn execute_tools(
            &self,
            _calls: Vec<PendingToolInvocation>,
            _events: LoopEventEmitter,
            _cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            Vec::new()
        }

        async fn request_approval(
            &self,
            _calls: &[PendingToolInvocation],
            _cancel: CancellationToken,
        ) -> Vec<ApprovalOutcome> {
            Vec::new()
        }

        async fn dispatch_provider_calls(
            &self,
            _calls: Vec<ProviderTranscriptInvocation>,
            _events: LoopEventEmitter,
            cancel: CancellationToken,
        ) -> Result<(), tool_api::ToolError> {
            // 模拟 dispatch 接管期间取消：hook 必须把取消映射为 Cancelled 错误。
            cancel.cancel();
            Err(tool_api::ToolError::cancelled(
                "cancelled during provider dispatch",
            ))
        }

        fn next_message_id(&self) -> MessageId {
            MessageId::from("m-1")
        }

        fn next_request_id(&self) -> RequestId {
            RequestId::from("r-1")
        }
    }

    #[tokio::test]
    async fn cancel_during_provider_dispatch_maps_to_run_cancelled() {
        let provider = Arc::new(MockProvider::new(
            MockScript::new()
                .tool_call("web_search", serde_json::json!({"query": "pawork"}))
                .complete_with(StopReason::ToolUse),
        ));
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("search")]);
        cfg.hosted_tools.push(provider_api::HostedToolRequest {
            name: "web_search".into(),
            kind: tool_api::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: Vec::new(),
            config: None,
        });
        let mut engine = ProviderLoop::new(
            provider,
            Arc::new(CancelDuringDispatch),
            cfg,
            1,
            broadcaster,
        );

        let result = engine.run(message_queue(), run_cancel()).await;
        assert!(matches!(result, Err(LoopError::Cancelled)));
        assert_eq!(engine.state(), RunState::Cancelled);
        let mut saw_cancelled = false;
        while let Ok(Some(event)) = sub.try_recv() {
            saw_cancelled |= matches!(event.payload, AgentEvent::RunCancelled { .. });
        }
        assert!(saw_cancelled, "dispatch 取消必须映射为 RunCancelled");
    }

    #[tokio::test]
    async fn mock_provider_completes_multi_turn_tool_loop() {
        // 第一轮请求工具，第二轮无工具直接完成。
        // MockProvider 每次 stream 调用重放同一脚本；用两阶段 provider 区分两轮。
        let first = MockScript::new()
            .tool_call("echo", serde_json::json!({"text": "hi"}))
            .usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 100,
                cache_write_tokens: 4,
            })
            .complete_with(StopReason::ToolUse);
        let tool = MockTool::new(
            "echo",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "hi".into() })]),
        );
        let tool = Arc::new(tool);

        // 两阶段 provider：第一次调用产工具，第二次产纯文本。
        #[derive(Clone)]
        struct TwoPhase {
            first: Arc<MockProvider>,
            second: Arc<MockProvider>,
            calls: Arc<std::sync::atomic::AtomicU64>,
        }
        #[async_trait::async_trait]
        impl ModelProvider for TwoPhase {
            fn id(&self) -> agent_domain::ProviderId {
                self.first.id()
            }
            async fn list_models(
                &self,
                cred: Option<&provider_api::ResolvedCredential>,
            ) -> Result<Vec<provider_api::ModelDefinition>, ProviderError> {
                self.first.list_models(cred).await
            }
            async fn stream(
                &self,
                request: CanonicalModelRequest,
                sink: &dyn ProviderEventSink,
                cancel: CancellationToken,
            ) -> Result<ModelResponseSummary, ProviderError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    self.first.stream(request, sink, cancel).await
                } else {
                    self.second.stream(request, sink, cancel).await
                }
            }
        }
        let provider: Arc<dyn ModelProvider> = Arc::new(TwoPhase {
            first: Arc::new(MockProvider::new(first)),
            second: Arc::new(MockProvider::new(
                MockScript::new()
                    .text("done")
                    .usage(TokenUsage {
                        input_tokens: 20,
                        output_tokens: 1,
                        cache_read_tokens: 5,
                        cache_write_tokens: 8,
                    })
                    .complete(),
            )),
            calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        });

        // TestContext 需要在两轮间共享 tool。
        struct SharedToolContext {
            tool: Arc<MockTool>,
            msg_counter: AtomicU64,
            req_counter: AtomicU64,
        }
        #[async_trait::async_trait]
        impl LoopContext for SharedToolContext {
            async fn execute_tools(
                &self,
                calls: Vec<PendingToolInvocation>,
                _events: LoopEventEmitter,
                cancel: CancellationToken,
            ) -> Vec<ToolCallResult> {
                let mut results = Vec::new();
                for call in calls {
                    let req = tool_api::ToolRequest {
                        tool_call_id: call.tool_call_id.clone(),
                        input: call.arguments.clone(),
                    };
                    let ctx = tool_api::ToolExecutionContext {
                        workspace_id: agent_domain::WorkspaceId::from("ws"),
                        run_id: RunId::from("run"),
                        working_directory: None,
                    };
                    let sink = test_support::RecordingToolSink::default();
                    let result = self
                        .tool
                        .execute(req, ctx, &sink, cancel.clone())
                        .await
                        .unwrap_or_else(|e| {
                            ToolResult::failure(agent_domain::ErrorContext::from(e))
                        });
                    results.push(ToolCallResult {
                        tool_call_id: call.tool_call_id,
                        tool_name: call.name,
                        arguments: call.arguments,
                        result,
                    });
                }
                results
            }
            async fn request_approval(
                &self,
                calls: &[PendingToolInvocation],
                _cancel: CancellationToken,
            ) -> Vec<ApprovalOutcome> {
                calls.iter().map(|_| ApprovalOutcome::Approved).collect()
            }
            fn next_message_id(&self) -> MessageId {
                let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
                MessageId::from(format!("msg-{n}"))
            }
            fn next_request_id(&self) -> RequestId {
                let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
                RequestId::from(format!("req-{n}"))
            }
        }
        let context: Arc<dyn LoopContext> = Arc::new(SharedToolContext {
            tool,
            msg_counter: AtomicU64::new(0),
            req_counter: AtomicU64::new(0),
        });

        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("echo")]),
            1,
            broadcaster,
        );
        let (state, summary) = engine.run(message_queue(), run_cancel()).await.unwrap();
        assert_eq!(state, RunState::Completed);
        assert_eq!(summary.stop_reason, StopReason::Completed);
        // 历史：user + assistant(tool call) + tool result + assistant(text) = 4
        assert_eq!(engine.messages().len(), 4);
        // run 级累计 usage：两轮 input 10+20、output 2+1，cache 各维度饱和累计。
        assert_eq!(summary.usage.input_tokens, 30);
        assert_eq!(summary.usage.output_tokens, 3);
        assert_eq!(summary.usage.cache_read_tokens, 105);
        assert_eq!(summary.usage.cache_write_tokens, 12);
        // RunCompleted 广播同样携带 run 级累计 usage，而非最后一轮单轮值。
        let mut completed_usage = None;
        while let Ok(Some(event)) = sub.try_recv() {
            if let AgentEvent::RunCompleted { usage, .. } = event.payload {
                completed_usage = Some(usage);
            }
        }
        let completed_usage = completed_usage.expect("必须广播 RunCompleted");
        assert_eq!(completed_usage.input_tokens, 30);
        assert_eq!(completed_usage.output_tokens, 3);
        assert_eq!(completed_usage.cache_read_tokens, 105);
        assert_eq!(completed_usage.cache_write_tokens, 12);
    }

    #[tokio::test]
    async fn cancelled_run_emits_cancelled_and_returns_error() {
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(MockScript::new().wait_for_cancellation()));
        let context: Arc<dyn LoopContext> = Arc::new(TestContext::new(Vec::new()));
        let cancel = run_cancel();
        cancel.cancel(CancelReason::User);
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("x")]),
            1,
            broadcaster,
        );

        let result = engine.run(message_queue(), cancel).await;
        assert!(matches!(result, Err(LoopError::Cancelled)));
        assert_eq!(engine.state(), RunState::Cancelled);
        let mut saw_cancelled = false;
        while let Ok(Some(event)) = sub.try_recv() {
            saw_cancelled |= matches!(event.payload, AgentEvent::RunCancelled { .. });
        }
        assert!(saw_cancelled, "取消路径必须广播 RunCancelled");
    }

    #[tokio::test]
    async fn streaming_cancel_runs_process_cleanup_and_emits_terminal_event() {
        struct Cleaner(Arc<AtomicU64>);
        impl crate::ProcessTreeCleaner for Cleaner {
            fn cleanup(&self, run_id: &RunId) -> usize {
                assert_eq!(run_id.as_str(), "run-1");
                self.0.fetch_add(1, Ordering::SeqCst);
                1
            }
        }

        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let cleaned = Arc::new(AtomicU64::new(0));
        let cancel = CancelHandle::new(RunId::from("run-1"), Arc::new(Cleaner(cleaned.clone())));
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            trigger.cancel(CancelReason::User);
        });
        let mut engine = ProviderLoop::new(
            Arc::new(MockProvider::new(MockScript::new().wait_for_cancellation())),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("cancel")]),
            1,
            broadcaster,
        );

        assert!(matches!(
            engine.run(message_queue(), cancel).await,
            Err(LoopError::Cancelled)
        ));
        assert_eq!(cleaned.load(Ordering::SeqCst), 1);
        let mut terminal = false;
        while let Ok(Some(event)) = sub.try_recv() {
            terminal |= matches!(event.payload, AgentEvent::RunCancelled { .. });
        }
        assert!(terminal);
    }

    /// 验证混合审批（A 通过、B 拒绝、C 通过）下 tool result 仍按原序排列。
    #[tokio::test]
    async fn mixed_approval_preserves_result_order() {
        // 单轮发三个 tool call，按 B 拒绝、其余通过审批。
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(
            MockScript::new()
                .tool_call("a", serde_json::json!({}))
                .tool_call("b", serde_json::json!({}))
                .tool_call("c", serde_json::json!({}))
                .complete_with(StopReason::ToolUse),
        ));
        let tool_a = MockTool::new(
            "a",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "A".into() })]),
        );
        let tool_c = MockTool::new(
            "c",
            ToolResult::success(vec![ContentPart::Text(TextContent { text: "C".into() })]),
        );

        struct SelectiveApproval {
            deny: &'static str,
            tools: Arc<Mutex<Vec<Arc<MockTool>>>>,
            msg_counter: AtomicU64,
            req_counter: AtomicU64,
        }
        #[async_trait::async_trait]
        impl LoopContext for SelectiveApproval {
            async fn execute_tools(
                &self,
                calls: Vec<PendingToolInvocation>,
                _events: LoopEventEmitter,
                _cancel: CancellationToken,
            ) -> Vec<ToolCallResult> {
                let tools = self.tools.lock().expect("tools").clone();
                let mut out = Vec::new();
                for call in calls {
                    let result =
                        if let Some(t) = tools.iter().find(|t| t.descriptor().name == call.name) {
                            t.execute(
                                tool_api::ToolRequest {
                                    tool_call_id: call.tool_call_id.clone(),
                                    input: call.arguments.clone(),
                                },
                                tool_api::ToolExecutionContext {
                                    workspace_id: agent_domain::WorkspaceId::from("ws"),
                                    run_id: RunId::from("run"),
                                    working_directory: None,
                                },
                                &test_support::RecordingToolSink::default(),
                                CancellationToken::new(),
                            )
                            .await
                            .unwrap_or_else(|e| {
                                ToolResult::failure(agent_domain::ErrorContext::from(e))
                            })
                        } else {
                            ToolResult::failure(agent_domain::ErrorContext {
                                category: agent_domain::ErrorCategory::NotFound,
                                message: format!("unknown tool {}", call.name),
                                retryable: false,
                                retry_after_ms: None,
                                diagnostics: Default::default(),
                            })
                        };
                    out.push(ToolCallResult {
                        tool_call_id: call.tool_call_id,
                        tool_name: call.name,
                        arguments: call.arguments,
                        result,
                    });
                }
                out
            }
            async fn request_approval(
                &self,
                calls: &[PendingToolInvocation],
                _cancel: CancellationToken,
            ) -> Vec<ApprovalOutcome> {
                calls
                    .iter()
                    .map(|c| {
                        if c.name == self.deny {
                            ApprovalOutcome::Denied
                        } else {
                            ApprovalOutcome::Approved
                        }
                    })
                    .collect()
            }
            fn next_message_id(&self) -> MessageId {
                let n = self.msg_counter.fetch_add(1, Ordering::Relaxed);
                MessageId::from(format!("msg-{n}"))
            }
            fn next_request_id(&self) -> RequestId {
                let n = self.req_counter.fetch_add(1, Ordering::Relaxed);
                RequestId::from(format!("req-{n}"))
            }
        }
        let context: Arc<dyn LoopContext> = Arc::new(SelectiveApproval {
            deny: "b",
            tools: Arc::new(Mutex::new(vec![Arc::new(tool_a), Arc::new(tool_c)])),
            msg_counter: AtomicU64::new(0),
            req_counter: AtomicU64::new(0),
        });
        let mut cfg = config(vec![user_message("go")]);
        // 预算=2：第 1 轮执行工具，第 2 轮触发预算停止（确保工具已执行并回填）。
        cfg.budget.max_iterations = Some(2);
        let mut engine = ProviderLoop::new(provider, context, cfg, 1, EventBroadcaster::new());
        // 第一轮：三个工具，B 被拒；预算=1 让循环停下。
        let _ = engine.run(message_queue(), run_cancel()).await;

        // 取回填的 Tool 消息（最后一条），其 content 应含三条 tool result，且按 a,b,c 序。
        let tool_msg = engine
            .messages()
            .iter()
            .rev()
            .find(|m| m.role == agent_domain::MessageRole::Tool)
            .expect("应有 Tool 角色消息");
        let results: Vec<&agent_domain::ContentPart> = tool_msg
            .content
            .iter()
            .filter(|p| matches!(p, agent_domain::ContentPart::ToolResult(_)))
            .collect();
        assert_eq!(results.len(), 3, "应回填三条 tool result");
        // 中间那条（b）应为错误（被拒）。
        if let agent_domain::ContentPart::ToolResult(tr) = results[1] {
            assert!(tr.is_error, "被拒工具 b 的结果应为错误");
        } else {
            panic!("第二条应为 ToolResult");
        }
        // 第一条与第三条应非错误。
        for (idx, part) in results.iter().enumerate() {
            if let agent_domain::ContentPart::ToolResult(tr) = part {
                let expected_error = idx == 1;
                assert_eq!(tr.is_error, expected_error, "第 {idx} 条 is_error 不符预期");
            }
        }
    }

    /// 验证 RunStarted / ContextPrepared 事件被广播（修复「每次转换都有事件」契约）。
    #[tokio::test]
    async fn run_started_and_context_prepared_are_broadcast() {
        let provider: Arc<dyn ModelProvider> =
            Arc::new(MockProvider::new(MockScript::new().text("ok").complete()));
        let context: Arc<dyn LoopContext> = Arc::new(TestContext::new(Vec::new()));
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            context,
            config(vec![user_message("hi")]),
            1,
            broadcaster,
        );
        let _ = engine.run(message_queue(), run_cancel()).await.unwrap();

        let mut saw_run_started = false;
        let mut saw_context_prepared = false;
        for _ in 0..32 {
            match tokio::time::timeout(std::time::Duration::from_millis(50), sub.recv()).await {
                Ok(Ok(env)) => match env.payload {
                    AgentEvent::RunStarted { .. } => saw_run_started = true,
                    AgentEvent::ContextPrepared { .. } => saw_context_prepared = true,
                    _ => {}
                },
                _ => break,
            }
        }
        assert!(saw_run_started, "应广播 RunStarted 事件");
        assert!(saw_context_prepared, "应广播 ContextPrepared 事件");
    }

    #[tokio::test]
    async fn provider_deltas_are_broadcast_while_streaming() {
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(
            MockScript::new().thinking("plan").text("answer").complete(),
        ));
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("hi")]),
            1,
            broadcaster,
        );

        engine.run(message_queue(), run_cancel()).await.unwrap();
        let mut text = false;
        let mut thinking = false;
        while let Ok(Some(event)) = sub.try_recv() {
            match event.payload {
                AgentEvent::AssistantTextDelta { delta, .. } if delta == "answer" => text = true,
                AgentEvent::AssistantThinkingDelta { delta, .. } if delta == "plan" => {
                    thinking = true
                }
                _ => {}
            }
        }
        assert!(text && thinking, "文本与 thinking delta 都应实时广播");
    }

    #[tokio::test]
    async fn server_tool_events_and_transcript_envelope_are_broadcast_in_sequence() {
        use agent_domain::{
            Citation, CitationSourceKind, ProgramStream, ProviderTranscriptEnvelope,
            ServerToolEvent, ToolCallId, TranscriptItem,
        };

        let provider = Arc::new(MockProvider::new(
            MockScript::new()
                .response_started("response-1")
                .server_tool(ServerToolEvent::Started {
                    tool_call_id: ToolCallId::from("server-tool-1"),
                    name: "web_search".into(),
                    arguments: Some(serde_json::json!({"query": "pawork"})),
                })
                .server_tool(ServerToolEvent::CitationAdded {
                    tool_call_id: ToolCallId::from("server-tool-1"),
                    citation: Citation {
                        url: Some("https://example.com".into()),
                        title: Some("Example".into()),
                        source_kind: CitationSourceKind::WebSearch,
                        ..Citation::empty()
                    },
                })
                .server_tool(ServerToolEvent::ProgramOutput {
                    tool_call_id: ToolCallId::from("server-tool-1"),
                    stream: ProgramStream::Stdout,
                    delta: None,
                    artifact: Some(agent_domain::ArtifactId::from("artifact-log-1")),
                })
                .server_tool(ServerToolEvent::Completed {
                    tool_call_id: ToolCallId::from("server-tool-1"),
                    summary: Some("3 results".into()),
                    artifacts: Vec::new(),
                })
                .transcript_envelope(ProviderTranscriptEnvelope {
                    items: vec![TranscriptItem::Text("final".into())],
                    cursor: Some("cursor-1".into()),
                    continuation_reference: None,
                })
                .text("done")
                .complete(),
        ));
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("search")]);
        cfg.hosted_tools.push(provider_api::HostedToolRequest {
            name: "web_search".into(),
            kind: tool_api::ToolCapabilityTag::WebSearch,
            description: String::new(),
            capabilities: vec![tool_api::ToolCapabilityTag::WebSearch],
            config: None,
        });
        let mut engine = ProviderLoop::new(
            provider,
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );

        engine.run(message_queue(), run_cancel()).await.unwrap();
        let mut server_tool_events = Vec::new();
        let mut saw_envelope = false;
        let mut sequences = Vec::new();
        while let Ok(Some(event)) = sub.try_recv() {
            sequences.push(event.sequence.value());
            match event.payload {
                AgentEvent::ServerTool(event) => server_tool_events.push(event),
                AgentEvent::TranscriptEnvelope(_) => saw_envelope = true,
                _ => {}
            }
        }
        assert_eq!(server_tool_events.len(), 4);
        assert!(
            matches!(&server_tool_events[0], ServerToolEvent::Started { name, .. } if name == "web_search"),
            "server tool 生命周期必须以 Started 开头"
        );
        assert!(matches!(
            &server_tool_events[3],
            ServerToolEvent::Completed { summary, .. }
                if summary.as_deref() == Some("3 results")
        ));
        assert!(saw_envelope, "transcript envelope 必须广播");
        assert!(
            sequences
                .windows(2)
                .all(|window| window[1] == window[0] + 1),
            "server tool 事件必须按严格连续的 sequence 广播: {sequences:?}"
        );
        assert!(engine
            .messages()
            .iter()
            .all(|message| message.role != agent_domain::MessageRole::Tool));
    }

    #[tokio::test]
    async fn loop_scheduler_bridge_serializes_capability_and_streams_tool_output() {
        struct SchedulerProbeTool {
            name: &'static str,
            current: Arc<AtomicU64>,
            peak: Arc<AtomicU64>,
            contexts: Arc<Mutex<Vec<tool_api::ToolExecutionContext>>>,
        }

        #[async_trait::async_trait]
        impl AgentTool for SchedulerProbeTool {
            fn descriptor(&self) -> tool_api::ToolDescriptor {
                tool_api::ToolDescriptor {
                    name: self.name.into(),
                    description: "scheduler bridge probe".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    capability: tool_api::ToolCapability::WorkspaceWrite,
                    kind: tool_api::ToolKind::ClientFunction,
                    hosting: tool_api::ToolHosting::Local,
                    capabilities: Vec::new(),
                    requires_approval: false,
                    read_only: false,
                    supports_concurrency: false,
                    default_timeout_ms: Some(1_000),
                    max_output_bytes: 1024,
                    allowed_in_untrusted_workspace: true,
                }
            }

            async fn execute(
                &self,
                _request: tool_api::ToolRequest,
                context: tool_api::ToolExecutionContext,
                sink: &dyn tool_api::ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, tool_api::ToolError> {
                self.contexts.lock().expect("contexts").push(context);
                let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(current, Ordering::SeqCst);
                sink.emit(tool_api::ToolStreamEvent::OutputDelta {
                    channel: tool_api::ToolOutputChannel::Stdout,
                    delta: self.name.into(),
                })
                .await?;
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                self.current.fetch_sub(1, Ordering::SeqCst);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        let current = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));
        let contexts = Arc::new(Mutex::new(Vec::new()));
        let tools: Vec<Arc<dyn AgentTool>> = ["write_a", "write_b"]
            .into_iter()
            .map(|name| {
                Arc::new(SchedulerProbeTool {
                    name,
                    current: current.clone(),
                    peak: peak.clone(),
                    contexts: contexts.clone(),
                }) as Arc<dyn AgentTool>
            })
            .collect();
        let mut registry = tool_runtime::ToolRegistry::new();
        registry.extend(tools).expect("probe tools register");
        let scheduler = Arc::new(tool_runtime::ToolScheduler::new(
            registry,
            tool_runtime::ToolSchedulerConfig {
                max_concurrent: 2,
                approval_mode: tool_runtime::ApprovalMode::NeverAsk,
                workspace_trusted: true,
            },
        ));
        let execution_context = tool_api::ToolExecutionContext {
            workspace_id: agent_domain::WorkspaceId::from("workspace-e2e"),
            run_id: RunId::from("run-e2e"),
            working_directory: Some("repo".into()),
        };
        let context: Arc<dyn LoopContext> = Arc::new(SchedulerLoopContext::new(
            scheduler,
            execution_context.clone(),
            Arc::new(tool_runtime::AutoApproveResolver),
        ));
        let provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("write_a", serde_json::json!({"path": "a"}))
                .tool_call("write_b", serde_json::json!({"path": "b"}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().text("done").complete(),
        ]);
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("write")]);
        cfg.run_id = RunId::from("run-e2e");
        let mut engine = ProviderLoop::new(Arc::new(provider), context, cfg, 1, broadcaster);
        let cancel = CancelHandle::new(
            RunId::from("run-e2e"),
            Arc::new(crate::NoopProcessTreeCleaner),
        );

        engine.run(message_queue(), cancel).await.unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), 1, "WorkspaceWrite 必须串行");
        let seen = contexts.lock().expect("contexts");
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|context| context == &execution_context));
        drop(seen);
        let mut tool_deltas = 0;
        let mut tool_started = 0;
        let mut argument_deltas = 0;
        while let Ok(Some(event)) = sub.try_recv() {
            match event.payload {
                AgentEvent::ToolOutputDelta { .. } => tool_deltas += 1,
                AgentEvent::ToolCallStarted { .. } => tool_started += 1,
                AgentEvent::ToolCallArgumentsDelta { .. } => argument_deltas += 1,
                _ => {}
            }
        }
        assert_eq!(tool_deltas, 2);
        assert_eq!(tool_started, 2);
        assert_eq!(argument_deltas, 2);
    }

    #[tokio::test]
    async fn scheduler_loop_context_uses_explicit_policy_resolver_once() {
        struct PolicyProbe {
            capability: tool_api::ToolCapability,
            calls: Arc<AtomicU64>,
        }

        #[async_trait::async_trait]
        impl AgentTool for PolicyProbe {
            fn descriptor(&self) -> tool_api::ToolDescriptor {
                tool_api::ToolDescriptor {
                    name: "policy_probe".into(),
                    description: "policy bridge probe".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    capability: self.capability.clone(),
                    kind: tool_api::ToolKind::ClientFunction,
                    hosting: tool_api::ToolHosting::Local,
                    capabilities: Vec::new(),
                    requires_approval: false,
                    read_only: false,
                    supports_concurrency: false,
                    default_timeout_ms: None,
                    max_output_bytes: 1024,
                    allowed_in_untrusted_workspace: false,
                }
            }

            async fn execute(
                &self,
                _request: tool_api::ToolRequest,
                _context: tool_api::ToolExecutionContext,
                _sink: &dyn tool_api::ToolEventSink,
                _cancel: CancellationToken,
            ) -> Result<ToolResult, tool_api::ToolError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::success(Vec::new()))
            }
        }

        struct ExplicitResolver {
            outcome: tool_runtime::ApprovalOutcome,
            calls: Arc<AtomicU64>,
        }

        #[async_trait::async_trait]
        impl tool_runtime::ApprovalResolver for ExplicitResolver {
            async fn resolve(
                &self,
                requests: &[tool_api::ToolRequest],
            ) -> Vec<tool_runtime::ApprovalOutcome> {
                self.calls
                    .fetch_add(requests.len() as u64, Ordering::SeqCst);
                requests.iter().map(|_| self.outcome).collect()
            }
        }

        async fn run_case(
            mode: tool_runtime::ApprovalMode,
            capability: tool_api::ToolCapability,
            input: serde_json::Value,
            outcome: tool_runtime::ApprovalOutcome,
        ) -> (u64, u64) {
            let tool_calls = Arc::new(AtomicU64::new(0));
            let approval_calls = Arc::new(AtomicU64::new(0));
            let mut registry = tool_runtime::ToolRegistry::new();
            registry
                .register(Arc::new(PolicyProbe {
                    capability,
                    calls: tool_calls.clone(),
                }))
                .expect("policy probe registers");
            let scheduler = Arc::new(tool_runtime::ToolScheduler::new(
                registry,
                tool_runtime::ToolSchedulerConfig {
                    max_concurrent: 1,
                    approval_mode: mode,
                    workspace_trusted: true,
                },
            ));
            let context: Arc<dyn LoopContext> = Arc::new(SchedulerLoopContext::new(
                scheduler,
                tool_api::ToolExecutionContext {
                    workspace_id: agent_domain::WorkspaceId::from("workspace-policy"),
                    run_id: RunId::from("run-policy"),
                    working_directory: None,
                },
                Arc::new(ExplicitResolver {
                    outcome,
                    calls: approval_calls.clone(),
                }),
            ));
            let provider = SequenceProvider::new(vec![
                MockScript::new()
                    .tool_call("policy_probe", input)
                    .complete_with(StopReason::ToolUse),
                MockScript::new().text("done").complete(),
            ]);
            let mut cfg = config(vec![user_message("policy")]);
            cfg.run_id = RunId::from("run-policy");
            let mut engine =
                ProviderLoop::new(Arc::new(provider), context, cfg, 1, EventBroadcaster::new());
            engine
                .run(
                    message_queue(),
                    CancelHandle::new(
                        RunId::from("run-policy"),
                        Arc::new(crate::NoopProcessTreeCleaner),
                    ),
                )
                .await
                .expect("provider loop");
            (
                tool_calls.load(Ordering::SeqCst),
                approval_calls.load(Ordering::SeqCst),
            )
        }

        assert_eq!(
            run_case(
                tool_runtime::ApprovalMode::AskForWrites,
                tool_api::ToolCapability::WorkspaceWrite,
                serde_json::json!({"path": "a.txt"}),
                tool_runtime::ApprovalOutcome::Denied,
            )
            .await,
            (0, 1),
            "明确拒绝不得执行"
        );
        assert_eq!(
            run_case(
                tool_runtime::ApprovalMode::AskForWrites,
                tool_api::ToolCapability::WorkspaceWrite,
                serde_json::json!({"path": "a.txt"}),
                tool_runtime::ApprovalOutcome::Approved,
            )
            .await,
            (1, 1),
            "明确批准应只提示一次并执行"
        );
        assert_eq!(
            run_case(
                tool_runtime::ApprovalMode::NeverAsk,
                tool_api::ToolCapability::Process,
                serde_json::json!({"command": "rm", "args": ["-rf", "/"]}),
                tool_runtime::ApprovalOutcome::Approved,
            )
            .await,
            (0, 0),
            "灾难命令地板应在 resolver 前直接拒绝"
        );
    }

    #[tokio::test]
    async fn interrupted_stream_retries_with_unchanged_messages() {
        let mut interrupted = ProviderError::new(
            provider_api::ProviderErrorKind::StreamInterrupted,
            "connection reset",
        );
        interrupted.retryable = true;
        interrupted.retry_after_ms = Some(0);
        let provider = SequenceProvider::new(vec![
            MockScript::new().text("partial").fail(interrupted),
            MockScript::new().text("final").complete(),
        ]);
        let provider_view = provider.clone();
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("retry")]),
            1,
            broadcaster,
        );

        engine.run(message_queue(), run_cancel()).await.unwrap();
        let requests = provider_view.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].messages, requests[1].messages);
        assert_eq!(requests[0].request_id, requests[1].request_id);
        assert!(engine.messages()[1]
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::Text(text) if text.text == "final")));
        let mut retry_diagnostic = false;
        while let Ok(Some(event)) = sub.try_recv() {
            retry_diagnostic |= matches!(
                event.payload,
                AgentEvent::Diagnostic { ref code, .. } if code == "provider_retry_attempt"
            );
        }
        assert!(retry_diagnostic, "每次重试必须产生 Diagnostic");
    }

    #[tokio::test]
    async fn queued_message_is_consumed_before_follow_up_turn() {
        let provider = SequenceProvider::new(vec![
            MockScript::new().text("first").complete(),
            MockScript::new().text("second").complete(),
        ]);
        let provider_view = provider.clone();
        let queue = message_queue();
        let queued = Message {
            id: MessageId::from("queued-user"),
            role: agent_domain::MessageRole::User,
            content: vec![ContentPart::Text(TextContent {
                text: "follow up".into(),
            })],
            metadata: MessageMetadata::default(),
        };
        queue.enqueue(queued).await;
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("first")]),
            1,
            EventBroadcaster::new(),
        );

        engine.run(queue, run_cancel()).await.unwrap();
        let requests = provider_view.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1]
            .messages
            .iter()
            .any(|message| message.id.as_str() == "queued-user"));
    }

    #[tokio::test]
    async fn budget_soft_warning_is_emitted_once_and_hard_limit_fails_terminally() {
        let provider = SequenceProvider::new(vec![MockScript::new().text("ok").complete()]);
        let queue = message_queue();
        for index in 0..3 {
            queue
                .enqueue(Message {
                    id: MessageId::from(format!("queued-{index}")),
                    role: agent_domain::MessageRole::User,
                    content: vec![ContentPart::Text(TextContent { text: "x".into() })],
                    metadata: MessageMetadata::default(),
                })
                .await;
        }
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("start")]);
        cfg.budget.max_iterations = Some(5);
        let mut engine = ProviderLoop::new(
            Arc::new(provider),
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );
        engine.run(queue, run_cancel()).await.unwrap();
        let mut warnings = 0;
        while let Ok(Some(event)) = sub.try_recv() {
            if let AgentEvent::Diagnostic { code, details } = event.payload {
                if code == "budget_soft_limit" {
                    warnings += 1;
                    assert_eq!(
                        details.get("dimension").and_then(serde_json::Value::as_str),
                        Some("iterations")
                    );
                    assert!(details.get("usage").is_some());
                    assert!(details.get("quota_signal_note").is_none());
                    assert_eq!(details.as_object().map(serde_json::Map::len), Some(2));
                }
            }
        }
        assert_eq!(warnings, 1, "同一预算维度只警告一次");

        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut cfg = config(vec![user_message("hard")]);
        cfg.budget.max_iterations = Some(1);
        let mut engine = ProviderLoop::new(
            Arc::new(MockProvider::new(MockScript::new().complete())),
            Arc::new(TestContext::new(Vec::new())),
            cfg,
            1,
            broadcaster,
        );
        assert!(matches!(
            engine.run(message_queue(), run_cancel()).await,
            Err(LoopError::BudgetExceeded(_))
        ));
        let mut failed = false;
        while let Ok(Some(event)) = sub.try_recv() {
            failed |= matches!(event.payload, AgentEvent::RunFailed { .. });
        }
        assert!(failed, "预算硬上限必须广播 RunFailed");
    }

    #[tokio::test]
    async fn scraped_and_stale_quota_diagnostics_include_signal_note() {
        for (signal, marker) in [
            (
                ExternalQuotaSignal {
                    remaining_ratio_ppm: 900_000,
                    exhausted: false,
                    stale: false,
                    confidence: crate::budget::QuotaSignalConfidence::Scraped,
                },
                "scraped signal",
            ),
            (
                ExternalQuotaSignal {
                    remaining_ratio_ppm: 900_000,
                    exhausted: false,
                    stale: true,
                    confidence: crate::budget::QuotaSignalConfidence::Exact,
                },
                "stale exact signal",
            ),
        ] {
            let broadcaster = EventBroadcaster::new();
            let mut sub = broadcaster.subscribe();
            let mut engine = ProviderLoop::new_with_external_quota(
                Arc::new(MockProvider::new(MockScript::new().text("ok").complete())),
                Arc::new(TestContext::new(Vec::new())),
                config(vec![user_message("quota diagnostic")]),
                1,
                broadcaster,
                Some(signal),
            );

            engine.run(message_queue(), run_cancel()).await.unwrap();

            let mut quota_note = None;
            while let Ok(Some(event)) = sub.try_recv() {
                if let AgentEvent::Diagnostic { code, details } = event.payload {
                    if code == "budget_soft_limit"
                        && details.get("dimension").and_then(serde_json::Value::as_str)
                            == Some("provider_quota")
                    {
                        assert!(details.get("usage").is_some());
                        assert_eq!(details.as_object().map(serde_json::Map::len), Some(3));
                        quota_note = details
                            .get("quota_signal_note")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned);
                    }
                }
            }

            let quota_note = quota_note.expect("ProviderQuota soft warning must include note");
            assert!(quota_note.contains(marker), "note: {quota_note}");
        }
    }

    #[tokio::test]
    async fn loop_records_cost_duration_concurrency_and_artifact_budgets() {
        let mut cost_cfg = config(vec![user_message("cost")]);
        cost_cfg.model = ModelId::from("gpt-4o");
        cost_cfg.budget.max_cost_micros = Some(1);
        let mut cost_engine = ProviderLoop::new(
            Arc::new(MockProvider::new(
                MockScript::new()
                    .usage(TokenUsage {
                        input_tokens: 1_000_000,
                        ..TokenUsage::default()
                    })
                    .complete(),
            )),
            Arc::new(TestContext::new(Vec::new())),
            cost_cfg,
            1,
            EventBroadcaster::new(),
        );
        let cost_error = cost_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            cost_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::Cost)
        ));

        #[derive(Clone)]
        struct DelayedProvider(Arc<MockProvider>);
        #[async_trait::async_trait]
        impl ModelProvider for DelayedProvider {
            fn id(&self) -> agent_domain::ProviderId {
                self.0.id()
            }
            async fn list_models(
                &self,
                credential: Option<&provider_api::ResolvedCredential>,
            ) -> Result<Vec<provider_api::ModelDefinition>, ProviderError> {
                self.0.list_models(credential).await
            }
            async fn stream(
                &self,
                request: CanonicalModelRequest,
                sink: &dyn ProviderEventSink,
                cancel: CancellationToken,
            ) -> Result<ModelResponseSummary, ProviderError> {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                self.0.stream(request, sink, cancel).await
            }
        }
        let mut duration_cfg = config(vec![user_message("duration")]);
        duration_cfg.budget.max_duration_ms = Some(1);
        let mut duration_engine = ProviderLoop::new(
            Arc::new(DelayedProvider(Arc::new(MockProvider::new(
                MockScript::new().complete(),
            )))),
            Arc::new(TestContext::new(Vec::new())),
            duration_cfg,
            1,
            EventBroadcaster::new(),
        );
        let duration_error = duration_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            duration_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::Duration)
        ));

        let mut concurrency_cfg = config(vec![user_message("concurrency")]);
        concurrency_cfg.budget.max_concurrency = Some(1);
        let mut concurrency_engine = ProviderLoop::new(
            Arc::new(MockProvider::new(
                MockScript::new()
                    .tool_call("unknown", serde_json::json!({}))
                    .complete_with(StopReason::ToolUse),
            )),
            Arc::new(TestContext::new(Vec::new())),
            concurrency_cfg,
            1,
            EventBroadcaster::new(),
        );
        let concurrency_error = concurrency_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            concurrency_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::Concurrency)
        ));

        let mut artifact_result = ToolResult::success(Vec::new());
        artifact_result
            .artifacts
            .push(agent_domain::ArtifactReference {
                id: agent_domain::ArtifactId::from("artifact-1"),
                media_type: "application/octet-stream".into(),
                byte_length: 10,
                content_hash: None,
                label: None,
            });
        let artifact_provider = SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("artifact", serde_json::json!({}))
                .complete_with(StopReason::ToolUse),
            MockScript::new().complete(),
        ]);
        let mut artifact_cfg = config(vec![user_message("artifact")]);
        artifact_cfg.budget.max_artifact_bytes = Some(10);
        let mut artifact_engine = ProviderLoop::new(
            Arc::new(artifact_provider),
            Arc::new(TestContext::new(vec![MockTool::new(
                "artifact",
                artifact_result,
            )])),
            artifact_cfg,
            1,
            EventBroadcaster::new(),
        );
        let artifact_error = artifact_engine
            .run(message_queue(), run_cancel())
            .await
            .unwrap_err();
        assert!(matches!(
            artifact_error,
            LoopError::BudgetExceeded(ref report)
                if report.hard_exceeded.contains(&BudgetDimension::ArtifactBytes)
        ));
    }

    #[tokio::test]
    async fn non_retryable_stream_error_emits_run_failed() {
        let error = ProviderError::new(provider_api::ProviderErrorKind::InvalidRequest, "bad");
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            Arc::new(MockProvider::new(MockScript::new().fail(error))),
            Arc::new(TestContext::new(Vec::new())),
            config(vec![user_message("bad")]),
            1,
            broadcaster,
        );
        assert!(matches!(
            engine.run(message_queue(), run_cancel()).await,
            Err(LoopError::Provider(_))
        ));
        let mut failed = false;
        while let Ok(Some(event)) = sub.try_recv() {
            failed |= matches!(event.payload, AgentEvent::RunFailed { .. });
        }
        assert!(failed);
    }

    // —— P17-1：pre-prompt / pre-tool 权威回灌位点 ——

    /// 记录 pre_prompt 改写与工具执行的测试上下文。
    struct HookTestContext {
        inner: TestContext,
        executed: Mutex<Vec<String>>,
        pre_prompt_prefix: Option<String>,
        pre_tool_deny: Vec<String>,
    }

    impl HookTestContext {
        fn new(tools: Vec<MockTool>) -> Self {
            Self {
                inner: TestContext::new(tools),
                executed: Mutex::new(Vec::new()),
                pre_prompt_prefix: None,
                pre_tool_deny: Vec::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl LoopContext for HookTestContext {
        async fn execute_tools(
            &self,
            calls: Vec<PendingToolInvocation>,
            events: LoopEventEmitter,
            cancel: CancellationToken,
        ) -> Vec<ToolCallResult> {
            self.executed
                .lock()
                .expect("executed")
                .extend(calls.iter().map(|call| call.name.clone()));
            self.inner.execute_tools(calls, events, cancel).await
        }

        async fn request_approval(
            &self,
            calls: &[PendingToolInvocation],
            cancel: CancellationToken,
        ) -> Vec<ApprovalOutcome> {
            self.inner.request_approval(calls, cancel).await
        }

        async fn pre_prompt(
            &self,
            request: &mut CanonicalModelRequest,
            events: LoopEventEmitter,
            cancel: CancellationToken,
        ) -> Result<(), LoopError> {
            if let Some(prefix) = &self.pre_prompt_prefix {
                if let Some(last_user) = request
                    .messages
                    .iter_mut()
                    .rev()
                    .find(|message| message.role == agent_domain::MessageRole::User)
                {
                    let mut text = String::new();
                    for part in &last_user.content {
                        if let ContentPart::Text(t) = part {
                            text.push_str(&t.text);
                        }
                    }
                    last_user.content = vec![ContentPart::Text(TextContent {
                        text: format!("{prefix}{text}"),
                    })];
                }
            }
            let _ = (events, cancel);
            Ok(())
        }

        async fn pre_tool(
            &self,
            invocations: &mut Vec<PendingToolInvocation>,
            events: LoopEventEmitter,
            cancel: CancellationToken,
        ) -> Result<(), LoopError> {
            invocations.retain(|inv| !self.pre_tool_deny.contains(&inv.name));
            let _ = (events, cancel);
            Ok(())
        }

        fn next_message_id(&self) -> MessageId {
            self.inner.next_message_id()
        }

        fn next_request_id(&self) -> RequestId {
            self.inner.next_request_id()
        }
    }

    #[tokio::test]
    async fn pre_prompt_hook_rewrites_prompt_before_provider_stream() {
        let provider = Arc::new(SequenceProvider::new(vec![MockScript::new()
            .text("hello")
            .complete()]));
        let mut hook_context = HookTestContext::new(Vec::new());
        hook_context.pre_prompt_prefix = Some("[HOOK] ".to_string());
        let context = Arc::new(hook_context);
        let mut engine = ProviderLoop::new(
            provider.clone(),
            context,
            config(vec![user_message("original prompt")]),
            1,
            EventBroadcaster::new(),
        );
        engine
            .run(message_queue(), run_cancel())
            .await
            .expect("run completes");

        let requests = provider.requests();
        assert_eq!(requests.len(), 1, "single-turn run sends one request");
        let last_user = requests[0]
            .messages
            .iter()
            .rev()
            .find(|message| message.role == agent_domain::MessageRole::User)
            .expect("user message");
        let mut text = String::new();
        for part in &last_user.content {
            if let ContentPart::Text(t) = part {
                text.push_str(&t.text);
            }
        }
        assert_eq!(
            text, "[HOOK] original prompt",
            "pre-prompt 改写必须在 Provider 收到请求前回灌"
        );
    }

    #[tokio::test]
    async fn pre_tool_hook_denied_tool_is_not_executed_and_gets_denied_result() {
        let provider = Arc::new(SequenceProvider::new(vec![
            MockScript::new()
                .tool_call("blocked-tool", serde_json::json!({}))
                .complete(),
            MockScript::new().text("done").complete(),
        ]));
        let mut hook_context = HookTestContext::new(Vec::new());
        hook_context.pre_tool_deny = vec!["blocked-tool".to_string()];
        let context = Arc::new(hook_context);
        let broadcaster = EventBroadcaster::new();
        let mut sub = broadcaster.subscribe();
        let mut engine = ProviderLoop::new(
            provider,
            context.clone(),
            config(vec![user_message("use the tool")]),
            1,
            broadcaster,
        );
        engine
            .run(message_queue(), run_cancel())
            .await
            .expect("run completes");

        assert!(
            context.executed.lock().expect("executed").is_empty(),
            "被 hook 拒绝的工具不得执行"
        );
        let mut saw_denied_completion = false;
        while let Ok(Some(event)) = sub.try_recv() {
            if let AgentEvent::ToolExecutionCompleted { result, .. } = event.payload {
                saw_denied_completion |= result.is_error;
            }
        }
        assert!(
            saw_denied_completion,
            "被拒绝的工具必须以失败结果回填 ToolExecutionCompleted"
        );
    }
}
