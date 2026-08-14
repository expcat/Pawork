//! 生产 `LoopContext`：策略预判 + 审批宿主 + `ToolScheduler`。
//!
//! AskUser 由 [`crate::approval::ApprovalPromptHost`] 决策；Allow 传 `None`
//! 给 scheduler，避免 S2「Allow 后再 resolve」钩子把只读工具也弹出来。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use pawork_api::{
    ToolError, ToolEventSink, ToolExecutionContext, ToolRequest, ToolResult, ToolStreamEvent,
};
use pawork_domain::{
    ApprovalDecision, CancellationToken, ContentPart, ErrorContext, MessageId, RequestId, RunId,
    TextContent, ToolCallId, ToolDescriptor, WorkspaceId,
};
use pawork_engine::{
    now_timestamp, ApprovalGate, LoopContext, LoopEventEmitter, PendingToolInvocation,
    ToolCallResult,
};
use pawork_policy::{ApprovalMode, ApprovalPrompt, PolicyDecision, PolicyEngine, PolicyInput, RiskLevel};
use pawork_tools::ToolScheduler;

use crate::approval::{
    preview_from_input, relative_path_from_input, ApprovalAsk, ApprovalPromptHost,
    PreApprovedResolver,
};

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
                        tool_name: call.name.clone(),
                        tool_call_id: call.tool_call_id.clone(),
                        relative_path: relative_path_from_input(&call.arguments),
                        message: prompt.message,
                        risk: prompt.risk,
                        preview: preview_from_input(&call.arguments),
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
