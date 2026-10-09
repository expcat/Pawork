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
/// 执行后若已取消：无任何真实动作事实时维持现状（整批丢弃，由调用方发
/// `RunCancelled`）；存在事实时为整批 invocations 配对结果并沿既有事件链
/// 收口后再返回 `Cancelled`。
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
        if !raw.iter().any(is_dispatch_fact) {
            return Ok(ToolRound::Cancelled);
        }
        // CU-10：输入已经派发的动作不能被取消抹掉。事实 = success == true
        // 或 metadata.input_dispatched 为严格布尔 true（字符串等真值不算）。
        // 只有带事实的批次才为整批 invocations 配对收口：有事实的 raw 原样
        // 保留，Denied 保持 Authorization，其余（含缺项与非事实 raw）显式
        // 回填取消失败。不重试、不再请求 Provider；仍返回 Cancelled，由
        // 调用者发 RunCancelled。
        let results = pair_cancelled_results(invocations, raw, decided);
        commit_tool_results(loop_ctx, emitter, results).await?;
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
    let tool_message = commit_tool_results(loop_ctx, emitter, results).await?;
    Ok(ToolRound::Committed(tool_message))
}

async fn commit_tool_results(
    loop_ctx: &dyn LoopContext,
    emitter: &EventEmitter<'_>,
    results: Vec<ToolCallResult>,
) -> Result<Message, EngineError> {
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
    Ok(tool_message)
}

/// 该结果是否携带真实动作事实：success == true，或
/// metadata.input_dispatched 为严格 JSON 布尔 true。两条是严格 OR，
/// 字符串、数字、null、JSON false 与缺字段都不是事实。
fn is_dispatch_fact(result: &ToolCallResult) -> bool {
    result.result.success
        || result
            .result
            .metadata
            .get("input_dispatched")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

/// 取消后带事实的批次：为整批 invocations 逐条配对。有事实的 raw 原样
/// 保留（即使 decision map 也有该 id）；无事实且 decision 为 Denied 的
/// 沿用拒绝结果；其余（缺 raw、非事实 raw、Cancelled decision）显式回填
/// 取消失败，Text 说明随 content 持久化（ToolResult::failure 的 content
/// 为空，而持久层保存的是 content 不是 error.message）。
fn pair_cancelled_results(
    invocations: &[PendingToolInvocation],
    raw: Vec<ToolCallResult>,
    mut decided: BTreeMap<ToolCallId, ApprovalDecision>,
) -> Vec<ToolCallResult> {
    let mut raw_by_id: BTreeMap<ToolCallId, ToolCallResult> = raw
        .into_iter()
        .map(|result| (result.tool_call_id.clone(), result))
        .collect();
    invocations
        .iter()
        .map(|invocation| {
            if let Some(result) = raw_by_id.remove(&invocation.tool_call_id) {
                if is_dispatch_fact(&result) {
                    decided.remove(&invocation.tool_call_id);
                    return result;
                }
            }
            match decided.remove(&invocation.tool_call_id) {
                Some(ApprovalDecision::Denied) => denied_tool_result(invocation),
                _ => cancelled_tool_result(invocation),
            }
        })
        .collect()
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

/// 取消批次里无事实条目的显式失败：说明写入 content，使
/// `MessageCommitted` 持久化的 Tool 消息携带可读原因。
fn cancelled_tool_result(invocation: &PendingToolInvocation) -> ToolCallResult {
    let mut result = ToolResult::failure(ErrorContext {
        category: ErrorCategory::Cancelled,
        message: "tool call cancelled with no dispatch fact".into(),
        retryable: false,
        retry_after_ms: None,
        diagnostics: Default::default(),
    });
    result.content = vec![ContentPart::Text(TextContent {
        text: "tool call cancelled with no dispatch fact".into(),
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
