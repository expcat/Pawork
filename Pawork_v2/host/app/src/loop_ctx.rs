//! 生产 `LoopContext`：把 engine 的待执行调用交给 `ToolScheduler`。
//!
//! 审批传 `None`（S2）；`working_directory` 为 `None`。失败结果把错误文案
//! 写入 `content`，方便模型解释、CLI 渲染。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use pawork_api::{
    ToolError, ToolEventSink, ToolExecutionContext, ToolRequest, ToolResult, ToolStreamEvent,
};
use pawork_domain::{
    CancellationToken, ContentPart, ErrorContext, MessageId, RequestId, RunId, TextContent,
    ToolCallId, WorkspaceId,
};
use pawork_engine::{
    now_timestamp, LoopContext, LoopEventEmitter, PendingToolInvocation, ToolCallResult,
};
use pawork_tools::ToolScheduler;

pub(crate) struct SessionLoopCtx<'a> {
    pub scheduler: Arc<ToolScheduler>,
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    pub next_message: &'a AtomicU64,
    pub next_request: &'a AtomicU64,
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
            async move { execute_one(&scheduler, workspace_id, run_id, call, events, cancel).await }
        });
        futures::future::join_all(jobs).await
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

async fn execute_one(
    scheduler: &ToolScheduler,
    workspace_id: WorkspaceId,
    run_id: RunId,
    call: PendingToolInvocation,
    events: LoopEventEmitter<'_>,
    cancel: CancellationToken,
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
    let mut result = match scheduler
        .execute_named(
            &call.name,
            request,
            context,
            cancel,
            None,
            &sink,
        )
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
