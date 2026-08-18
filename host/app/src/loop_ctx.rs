//! 生产 `LoopContext`：策略预判 + 审批宿主 + `ToolScheduler`。
//!
//! AskUser 由 [`crate::approval::ApprovalPromptHost`] 决策；Allow 传 `None`
//! 给 scheduler，避免 S2「Allow 后再 resolve」钩子把只读工具也弹出来。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use pawork_api::{
    ToolError, ToolEventSink, ToolExecutionContext, ToolRequest, ToolResult, ToolStreamEvent,
};
use pawork_domain::{
    ApprovalDecision, CancellationToken, ContentPart, ErrorContext, EventId, EventSequence,
    MessageId, RequestId, RunId, TextContent, ToolCallId, ToolDescriptor, WorkspaceId,
};
use pawork_blob_store::CheckpointService;
use pawork_engine::{
    now_timestamp, ApprovalGate, AutoCompactionReason, CompactionOutcome, LoopContext,
    LoopEventEmitter, PendingToolInvocation, ToolCallResult, WriteCheckpoint,
};
use pawork_policy::{ApprovalMode, ApprovalPrompt, PolicyDecision, PolicyEngine, PolicyInput, RiskLevel};
use pawork_session::{
    CompactionEngine, CompactionReason as SessionCompactionReason, RetentionInputs,
    RetentionMessage, RetentionToolCall, SessionStore, ToolCallRetentionState,
};
use pawork_tools::ToolScheduler;

use crate::approval::{
    preview_for_tool, relative_path_from_input, ApprovalAsk, ApprovalPromptHost,
    PreApprovedResolver,
};
use crate::checkpoint;

pub(crate) struct SessionLoopCtx<'a> {
    pub scheduler: Arc<ToolScheduler>,
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    pub next_message: &'a AtomicU64,
    pub next_request: &'a AtomicU64,
    pub policy: PolicyEngine,
    pub approval_mode: ApprovalMode,
    pub workspace_trusted: bool,
    pub descriptors: Vec<ToolDescriptor>,
    pub approval_host: Arc<dyn ApprovalPromptHost>,
    /// 压缩回调需要的持久化宿主；测试替身可为 None（engine 退回消息层压缩）。
    pub store: Option<&'a SessionStore>,
    pub session_id: Option<pawork_domain::SessionId>,
    pub token_estimator: Option<Arc<dyn pawork_session::TokenEstimator>>,
    pub checkpoints: Option<CheckpointService>,
    pub workspace_roots: Vec<PathBuf>,
}

struct ForwardingSink<'a> {
    tool_call_id: ToolCallId,
    events: LoopEventEmitter<'a>,
}

#[async_trait]
impl ToolEventSink for ForwardingSink<'_> {
    async fn emit(&self, event: ToolStreamEvent) -> Result<(), ToolError> {
        self.events
            .emit_tool_event(self.tool_call_id.clone(), event)
            .await
            .map_err(|error| ToolError {
                kind: pawork_api::ToolErrorKind::Internal,
                message: error.to_string(),
                retryable: false,
                retry_after_ms: None,
            })
    }
}

#[async_trait]
impl LoopContext for SessionLoopCtx<'_> {
    async fn execute_tools(
        &self,
        calls: Vec<PendingToolInvocation>,
        events: LoopEventEmitter<'_>,
        cancel: CancellationToken,
    ) -> Vec<ToolCallResult> {
        let jobs = calls.into_iter().map(|call| {
            let scheduler = self.scheduler.clone();
            let workspace_id = self.workspace_id.clone();
            let run_id = self.run_id.clone();
            let events = events.clone();
            let cancel = cancel.clone();
            let policy = self.policy.clone();
            let approval_mode = self.approval_mode;
            let workspace_trusted = self.workspace_trusted;
            let descriptors = self.descriptors.clone();
            async move {
                execute_one(
                    &scheduler,
                    workspace_id,
                    run_id,
                    call,
                    events,
                    cancel,
                    &policy,
                    approval_mode,
                    workspace_trusted,
                    &descriptors,
                )
                .await
            }
        });
        futures::future::join_all(jobs).await
    }

    async fn request_approval(
        &self,
        calls: &[PendingToolInvocation],
        already_approved_for_run: bool,
        cancel: CancellationToken,
    ) -> Vec<ApprovalGate> {
        let mut batch_approved = already_approved_for_run;
        let mut gates = Vec::with_capacity(calls.len());
        for call in calls {
            let Some(descriptor) = self
                .descriptors
                .iter()
                .find(|item| item.name == call.name)
                .cloned()
            else {
                gates.push(ApprovalGate::NotRequired);
                continue;
            };
            let decision = decide_policy(
                &self.policy,
                &descriptor,
                &call.arguments,
                self.workspace_trusted,
                self.approval_mode,
            );
            match decision {
                PolicyDecision::AskUser { prompt } => {
                    if batch_approved {
                        gates.push(ApprovalGate::Asked(ApprovalDecision::ApprovedForRun));
                        continue;
                    }
                    let ask = ApprovalAsk {
                        run_id: self.run_id.clone(),
                        session_id: self.session_id.clone(),
                        tool_name: call.name.clone(),
                        tool_call_id: call.tool_call_id.clone(),
                        relative_path: relative_path_from_input(&call.arguments),
                        message: prompt.message,
                        risk: prompt.risk,
                        preview: preview_for_tool(
                            &call.name,
                            &call.arguments,
                            &self.workspace_roots,
                        ),
                    };
                    let answered = self.approval_host.decide(&ask, cancel.clone()).await;
                    if matches!(answered, ApprovalDecision::ApprovedForRun) {
                        batch_approved = true;
                    }
                    gates.push(ApprovalGate::Asked(answered));
                }
                PolicyDecision::Allow
                | PolicyDecision::AllowWithConstraints { .. }
                | PolicyDecision::Deny { .. } => gates.push(ApprovalGate::NotRequired),
            }
        }
        gates
    }

    fn next_message_id(&self) -> MessageId {
        let n = self.next_message.fetch_add(1, Ordering::Relaxed);
        MessageId::from(format!("msg-{}-{n}", now_timestamp().as_unix_millis()))
    }

    fn next_request_id(&self) -> RequestId {
        let n = self.next_request.fetch_add(1, Ordering::Relaxed);
        RequestId::from(format!("req-{n}"))
    }

    /// 压缩回调：session 侧 fork recovery branch + 产出压缩快照，回传元数据。
    ///
    /// 无持久化宿主（测试替身）时返回 None，engine 退回纯消息层压缩。
    async fn compact_history(
        &self,
        reason: AutoCompactionReason,
        summary_text: &str,
        _cancel: CancellationToken,
    ) -> Option<CompactionOutcome> {
        let store = self.store?;
        let session_id = self.session_id.clone()?;
        let estimator = self.token_estimator.clone()?;
        let session_reason = match reason {
            AutoCompactionReason::Manual => SessionCompactionReason::Manual,
            AutoCompactionReason::HistorySoftLimit => SessionCompactionReason::HistorySoftLimit,
            AutoCompactionReason::InputBudgetExceeded => {
                SessionCompactionReason::InputBudgetExceeded
            }
        };

        let active_branch = store.get_session(&session_id).await.ok()?.active_branch;
        // 从 active branch 祖先链构建保留策略输入（不是全 session replay）。
        let events = store
            .events_on_lineage(&session_id, &active_branch, 1, usize::MAX)
            .await
            .ok()?;
        let mut inputs = RetentionInputs::default();
        let mut started_tools: std::collections::BTreeMap<pawork_domain::ToolCallId, (EventId, bool)> =
            Default::default();
        for envelope in &events {
            match &envelope.payload {
                pawork_domain::AgentEvent::MessageCommitted { message } => {
                    inputs.messages.push(RetentionMessage {
                        event_id: envelope.event_id.clone(),
                        message: message.clone(),
                    });
                }
                pawork_domain::AgentEvent::ToolCallStarted { tool_call_id, .. } => {
                    started_tools.insert(
                        tool_call_id.clone(),
                        (envelope.event_id.clone(), false),
                    );
                }
                pawork_domain::AgentEvent::ToolExecutionCompleted { tool_call_id, .. } => {
                    if let Some(entry) = started_tools.get_mut(tool_call_id) {
                        entry.1 = true;
                    }
                }
                _ => {}
            }
        }
        for (event_id, completed) in started_tools.into_values() {
            inputs.tool_calls.push(RetentionToolCall {
                event_id,
                state: if completed {
                    ToolCallRetentionState::Completed
                } else {
                    ToolCallRetentionState::Pending
                },
            });
        }

        // 对齐 engine 的消息级保留（crate::RETAINED_MESSAGES 条消息 ≈ 2 轮对话），
        // 让持久化投影与 engine 重建历史在同一边界折叠。
        let engine = CompactionEngine::with_policy(
            store,
            pawork_session::RetentionPolicy {
                retained_turns: (crate::RETAINED_MESSAGES / 2) as u32,
                ..Default::default()
            },
            estimator,
        );
        let result = engine
            .compact(
                &session_id,
                &active_branch,
                session_reason,
                summary_text,
                &inputs,
            )
            .await
            .ok()?;
        let retained_event_ids: std::collections::HashSet<&pawork_domain::EventId> =
            result.decision.retained_event_ids.iter().collect();
        Some(CompactionOutcome {
            source_event_count: result.total_events as u64,
            // 折叠水位 = 被折叠（未保留）消息提交事件的最大 sequence；
            // 保留尾部与摘要（新 sequence）不受影响。无折叠时为 0（projection 不删）。
            compacted_through: events
                .iter()
                .filter(|envelope| {
                    matches!(
                        &envelope.payload,
                        pawork_domain::AgentEvent::MessageCommitted { .. }
                    ) && !retained_event_ids.contains(&envelope.event_id)
                })
                .map(|envelope| envelope.sequence)
                .max()
                .unwrap_or(EventSequence::new(0)),
        })
    }

    async fn snapshot_write_tools(
        &self,
        calls: &[PendingToolInvocation],
        cancel: CancellationToken,
    ) -> Vec<WriteCheckpoint> {
        let Some(checkpoints) = self.checkpoints.as_ref() else {
            return Vec::new();
        };
        if self.workspace_roots.is_empty() {
            return Vec::new();
        }
        checkpoint::snapshot_write_tools(
            checkpoints,
            &self.run_id,
            &self.workspace_roots,
            calls,
            cancel,
        )
        .await
    }
}

fn decide_policy(
    policy: &PolicyEngine,
    descriptor: &ToolDescriptor,
    input: &serde_json::Value,
    trusted: bool,
    approval_mode: ApprovalMode,
) -> PolicyDecision {
    let mut decision = policy.decide(&PolicyInput {
        capability: descriptor.capability.clone(),
        input: input.clone(),
        trusted,
        allowed_in_untrusted_workspace: descriptor.allowed_in_untrusted_workspace,
        approval_mode,
    });
    if descriptor.requires_approval && !matches!(decision, PolicyDecision::Deny { .. }) {
        decision = PolicyDecision::AskUser {
            prompt: ApprovalPrompt {
                message: format!("tool `{}` requires explicit approval", descriptor.name),
                risk: RiskLevel::Moderate,
            },
        };
    }
    decision
}

async fn execute_one(
    scheduler: &ToolScheduler,
    workspace_id: WorkspaceId,
    run_id: RunId,
    call: PendingToolInvocation,
    events: LoopEventEmitter<'_>,
    cancel: CancellationToken,
    policy: &PolicyEngine,
    approval_mode: ApprovalMode,
    workspace_trusted: bool,
    descriptors: &[ToolDescriptor],
) -> ToolCallResult {
    let request = ToolRequest {
        tool_call_id: call.tool_call_id.clone(),
        input: call.arguments.clone(),
    };
    let context = ToolExecutionContext {
        workspace_id,
        run_id,
        working_directory: None,
    };
    let sink = ForwardingSink {
        tool_call_id: call.tool_call_id.clone(),
        events,
    };
    let preapproved = descriptors
        .iter()
        .find(|item| item.name == call.name)
        .is_some_and(|descriptor| {
            matches!(
                decide_policy(
                    policy,
                    descriptor,
                    &call.arguments,
                    workspace_trusted,
                    approval_mode,
                ),
                PolicyDecision::AskUser { .. }
            )
        });
    let resolver = PreApprovedResolver;
    let approval = if preapproved {
        Some(&resolver as &dyn pawork_tools::ApprovalResolver)
    } else {
        None
    };
    let mut result = match scheduler
        .execute_named(&call.name, request, context, cancel, approval, &sink)
        .await
    {
        Ok(result) => result,
        Err(error) => ToolResult::failure(ErrorContext::from(error)),
    };
    fill_error_content(&mut result);
    ToolCallResult {
        tool_call_id: call.tool_call_id,
        tool_name: call.name,
        arguments: call.arguments,
        result,
    }
}

fn fill_error_content(result: &mut ToolResult) {
    if !result.is_error() || !result.content.is_empty() {
        return;
    }
    if let Some(error) = &result.error {
        result.content = vec![ContentPart::Text(TextContent {
            text: error.message.clone(),
        })];
    }
}
