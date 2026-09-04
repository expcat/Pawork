//! 工具执行与结果回填：写前快照、执行已放行调用、对齐结果并提交 Tool 消息。

use std::collections::BTreeMap;

use pawork_domain::{
    AgentEvent, ApprovalDecision, CancellationToken, ContentPart, ErrorCategory, ErrorContext,
    Message, TextContent, ToolCallId, ToolResult, ToolResultContent,
};

use crate::appender::{tool_results_message, AssembledTurn, ToolCallResult};
use crate::event::{EngineError, EventEmitter, LoopEventEmitter};

use super::{LoopContext, PendingToolInvocation};

pub(super) enum ToolRound {
    Cancelled,
    Committed(Message),
}

pub(super) fn pending_invocations(assembled: &AssembledTurn) -> Vec<PendingToolInvocation> {
    assembled
        .tool_call_order
        .iter()
        .filter_map(|id| {
            assembled
                .tool_calls
                .get(id)
                .map(|call| PendingToolInvocation {
                    tool_call_id: id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments(),
                })
        })
        .collect()
}

/// 快照 → 执行 → 对齐结果 → `ToolExecutionCompleted` → Tool `MessageCommitted`。
/// 执行后若已取消，不提交工具结果（由调用方发 `RunCancelled`）。
pub(super) async fn snapshot_execute_commit(
    loop_ctx: &dyn LoopContext,
    invocations: &[PendingToolInvocation],
    to_run: Vec<PendingToolInvocation>,
    mut decided: BTreeMap<ToolCallId, ApprovalDecision>,
    events: LoopEventEmitter<'_>,
    emitter: &EventEmitter<'_>,
    cancel: CancellationToken,
) -> Result<ToolRound, EngineError> {
    let checkpoints = loop_ctx
        .snapshot_write_tools(&to_run, events.clone(), cancel.clone())
        .await;
    for checkpoint in checkpoints {
        emitter
            .emit(AgentEvent::CheckpointCreated {
                checkpoint_id: checkpoint.checkpoint_id,
                artifacts: checkpoint.artifacts,
            })
            .await?;
    }

    for invocation in &to_run {
        emitter
            .emit(AgentEvent::ToolExecutionStarted {
                tool_call_id: invocation.tool_call_id.clone(),
            })
            .await?;
    }

    let raw = if to_run.is_empty() {
        Vec::new()
    } else {
        loop_ctx.execute_tools(to_run, events, cancel.clone()).await
    };
    if cancel.is_cancelled() {
        return Ok(ToolRound::Cancelled);
    }
    for result in &raw {
        decided.remove(&result.tool_call_id);
    }
    let mut merged = raw;
    merged.extend(decided.into_iter().filter_map(|(id, decision)| {
        if matches!(
            decision,
            ApprovalDecision::Denied | ApprovalDecision::Cancelled
        ) {
            invocations
                .iter()
                .find(|call| call.tool_call_id == id)
                .map(denied_tool_result)
        } else {
            None
        }
    }));
    let results = align_tool_results(invocations, merged);

    for result in &results {
        let content = tool_result_content(result);
        emitter
            .emit(AgentEvent::ToolExecutionCompleted {
                tool_call_id: result.tool_call_id.clone(),
                result: content.clone(),
            })
            .await?;
        if let Some(details) = sandbox_fallback_details(&content.metadata) {
            emitter
                .emit(AgentEvent::Diagnostic {
                    code: "sandbox.fallback".into(),
                    details,
                })
                .await?;
        }
    }

    let tool_message = tool_results_message(loop_ctx.next_message_id(), results);
    emitter
        .emit(AgentEvent::MessageCommitted {
            message: tool_message.clone(),
        })
        .await?;
    Ok(ToolRound::Committed(tool_message))
}

fn denied_tool_result(invocation: &PendingToolInvocation) -> ToolCallResult {
    let mut result = ToolResult::failure(ErrorContext {
        category: ErrorCategory::Authorization,
        message: "tool call denied by user".into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    });
    result.content = vec![ContentPart::Text(TextContent {
        text: "tool call denied by user".into(),
    })];
    ToolCallResult {
        tool_call_id: invocation.tool_call_id.clone(),
        tool_name: invocation.name.clone(),
        arguments: invocation.arguments.clone(),
        result,
    }
}

fn align_tool_results(
    invocations: &[PendingToolInvocation],
    results: Vec<ToolCallResult>,
) -> Vec<ToolCallResult> {
    let mut by_id: BTreeMap<ToolCallId, ToolCallResult> = results
        .into_iter()
        .map(|result| (result.tool_call_id.clone(), result))
        .collect();
    invocations
        .iter()
        .map(|invocation| {
            by_id
                .remove(&invocation.tool_call_id)
                .unwrap_or_else(|| ToolCallResult {
                    tool_call_id: invocation.tool_call_id.clone(),
                    tool_name: invocation.name.clone(),
                    arguments: invocation.arguments.clone(),
                    result: ToolResult::failure(ErrorContext {
                        category: ErrorCategory::NotFound,
                        message: "missing tool result".into(),
                        retryable: false,
                        retry_after_ms: None,
                        diagnostics: Default::default(),
                    }),
                })
        })
        .collect()
}

fn tool_result_content(result: &ToolCallResult) -> ToolResultContent {
    ToolResultContent {
        tool_call_id: result.tool_call_id.clone(),
        tool_name: Some(result.tool_name.clone()),
        content: result.result.content.clone(),
        is_error: result.result.is_error(),
        metadata: result.result.metadata.clone(),
        artifacts: result.result.artifacts.clone(),
    }
}

fn sandbox_fallback_details(metadata: &serde_json::Value) -> Option<serde_json::Value> {
    let sandbox = metadata.get("sandbox")?;
    if !sandbox.get("fallback")?.as_bool()? {
        return None;
    }
    let isolation = sandbox
        .get("isolation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let backend = sandbox
        .get("backend")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let note = sandbox
        .get("note")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let message = if note.is_empty() {
        format!("沙箱回退：isolation={isolation} backend={backend}")
    } else {
        format!("沙箱回退：isolation={isolation} backend={backend}（{note}）")
    };
    Some(serde_json::json!({
        "message": message,
        "isolation": isolation,
        "backend": backend,
        "note": note,
        "fallback": true,
    }))
}
