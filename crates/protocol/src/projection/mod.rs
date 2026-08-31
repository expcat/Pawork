//! Timeline 投影 reducer：host 分页历史与 GUI live 流的单一同源实现。
//!
//! 背景（S12-CR08-08 / R3 波 C）：此前 host `timeline()` 用
//! `project_timeline_item` 把持久化事件映射为 [`TimelineItem`](crate::TimelineItem)，
//! desktop `projection.rs` 再用字符串匹配 + 手工 JSON 解构消费同一批条目，live 臂
//! （[`AppEvent`](crate::AppEvent)）与历史臂语义各自生长，去重 / committed 替换 /
//! tool 回填在两端不一致。本模块把两臂收敛到同一合并核：
//!
//! - [`project_event`]：持久化 `AgentEventEnvelope` → presentation-safe
//!   `TimelineItem`（自 app host 逐字平移，wire 形状不变）；
//! - [`TimelineProjection`]：去重（session sequence）、有序插入
//!   （partition_point）、assistant delta 合并与 committed 替换、tool 身份锚点
//!   （live / 历史统一使用 `run+tool_call_id`）全部单一实现；
//! - resume 三态：Replay 保留基线、SnapshotRequired 清基线、UpToDate 不动。
//! - fork 边界（R6/ADR-040 D5）：[`TimelineEntry::fork_boundary`] 以强类型
//!   标记 run 终止条目（历史 `RunCompleted/RunCancelled/RunFailed` 与 live
//!   `RunState::{Completed,Cancelled,Failed}`），Desktop fork 单点判型；
//!   wire / golden 形状不变。
//!
//! 本模块是纯数据投影，不进 wire 帧：[`TimelineEntry`] 系列不加 serde derive，
//! 序列化只发生在测试渲染层。分页游标元数据（next_sequence/complete）留在
//! 消费方适配层，reducer 只管 entries 语义。

use std::collections::BTreeSet;
use std::ops::Deref;

use pawork_domain::{AgentEvent, AgentEventEnvelope, ApprovalDecision, ContentPart, MessageRole};

use crate::ResumeDisposition;
use crate::app::{AppEvent, AppEventEnvelope, RunState, TimelineItem, TimelineItemKind};

const TOOL_CONTEXT_ID_KEY: &str = "_pawork_tool_call_id";
const TOOL_CONTEXT_DETAIL_KEY: &str = "detail";

/// 把持久化的 Agent 事件投影为 presentation-safe 的 Timeline 条目。
///
/// 自 app `gui_host::project_timeline_item` 逐字平移（R3 波 C）：host
/// `timeline()` 与本模块历史臂共用同一映射，wire 形状（serde tag/rename）
/// 保持冻结不变。
pub fn project_event(envelope: &AgentEventEnvelope) -> Option<TimelineItem> {
    let (kind, text, tool_name, status, detail) = match &envelope.payload {
        AgentEvent::MessageCommitted { message } => match message.role {
            MessageRole::User => (
                TimelineItemKind::UserMessage,
                Some(join_text(&message.content)),
                None,
                None,
                None,
            ),
            MessageRole::Assistant => (
                TimelineItemKind::AssistantMessage,
                Some(join_text(&message.content)),
                None,
                None,
                None,
            ),
            _ => return None,
        },
        AgentEvent::AssistantTextDelta { delta, .. } => (
            TimelineItemKind::AssistantDelta,
            Some(delta.clone()),
            None,
            None,
            None,
        ),
        AgentEvent::ToolCallStarted { tool_call_id, name } => (
            TimelineItemKind::ToolStarted,
            None,
            Some(name.clone()),
            Some("running".into()),
            Some(tool_timeline_context(tool_call_id.as_str(), None)),
        ),
        AgentEvent::ToolOutputDelta {
            tool_call_id,
            delta,
            ..
        } => (
            TimelineItemKind::ToolOutput,
            Some(delta.clone()),
            None,
            None,
            Some(tool_timeline_context(tool_call_id.as_str(), None)),
        ),
        AgentEvent::ToolExecutionCompleted {
            tool_call_id,
            result,
        } => {
            let display_detail = sandbox_timeline_detail(&result.metadata);
            (
            TimelineItemKind::ToolCompleted,
            Some(join_text(&result.content)),
            result.tool_name.clone(),
                Some(
                    if result.is_error {
                        "failed"
                    } else {
                        "succeeded"
                    }
                    .into(),
                ),
                Some(tool_timeline_context(
                    tool_call_id.as_str(),
                    display_detail.as_deref(),
                )),
            )
        }
        AgentEvent::ToolApprovalRequested { reason, .. } => (
            TimelineItemKind::ApprovalRequested,
            None,
            None,
            Some("pending".into()),
            Some(reason.clone()),
        ),
        AgentEvent::ToolApprovalResponded { decision, .. } => (
            TimelineItemKind::ApprovalResponded,
            None,
            None,
            Some(decision_status(decision)),
            None,
        ),
        AgentEvent::RunStarted { .. } => {
            (TimelineItemKind::RunStarted, None, None, None, None)
        }
        AgentEvent::RunCompleted { .. } => {
            (TimelineItemKind::RunCompleted, None, None, None, None)
        }
        AgentEvent::RunCancelled { .. } => {
            (TimelineItemKind::RunCancelled, None, None, None, None)
        }
        AgentEvent::RunFailed { error, .. } => (
            TimelineItemKind::RunFailed,
            None,
            None,
            Some("failed".into()),
            Some(error.message.clone()),
        ),
        AgentEvent::Diagnostic { code, details } => (
            TimelineItemKind::Diagnostic,
            None,
            None,
            None,
            Some(format!("{code}: {details}")),
        ),
        AgentEvent::CheckpointCreated { checkpoint_id, .. } => (
            TimelineItemKind::Other,
            None,
            None,
            None,
            Some(format!("checkpoint {}", checkpoint_id.as_str())),
        ),
        AgentEvent::CheckpointRolledBack { checkpoint_id } => (
            TimelineItemKind::Other,
            None,
            None,
            None,
            Some(format!("rollback {}", checkpoint_id.as_str())),
        ),
        _ => return None,
    };
    Some(TimelineItem {
        sequence: envelope.sequence.0,
        event_id: envelope.event_id.as_str().to_string(),
        kind,
        run_id: Some(envelope.run_id.as_str().to_string()),
        text,
        tool_name,
        status,
        detail,
        timestamp: envelope.timestamp.as_unix_millis().to_string(),
    })
}

fn sandbox_timeline_detail(metadata: &serde_json::Value) -> Option<String> {
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
    Some(format!("沙箱回退：isolation={isolation} backend={backend}"))
}

/// `TimelineItem` 的 wire 形状在 R3 冻结，不能新增 `tool_call_id` 字段。工具
/// 历史投影仍必须保留稳定身份，否则同一 run 并发工具的 output/completed 会串线。
/// 因此把关联上下文编码进既有 `detail` 字符串；reducer 消费时剥离，仅把可展示
/// 的 `detail` 留给 [`TimelineEntryKind::ToolCall`]。
fn tool_timeline_context(tool_call_id: &str, detail: Option<&str>) -> String {
    let mut context = serde_json::Map::new();
    context.insert(
        TOOL_CONTEXT_ID_KEY.into(),
        serde_json::Value::String(tool_call_id.to_string()),
    );
    if let Some(detail) = detail {
        context.insert(
            TOOL_CONTEXT_DETAIL_KEY.into(),
            serde_json::Value::String(detail.to_string()),
        );
    }
    serde_json::Value::Object(context).to_string()
}

fn parse_tool_timeline_context(detail: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(raw) = detail else {
        return (None, None);
    };
    let Ok(context) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (None, Some(raw.to_string()));
    };
    let Some(tool_call_id) = context
        .get(TOOL_CONTEXT_ID_KEY)
        .and_then(serde_json::Value::as_str)
    else {
        return (None, Some(raw.to_string()));
    };
    let display_detail = context
        .get(TOOL_CONTEXT_DETAIL_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    (Some(tool_call_id.to_string()), display_detail)
}

fn join_text(parts: &[ContentPart]) -> String {
    let mut text = String::new();
    for part in parts {
        if let ContentPart::Text(content) = part {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&content.text);
        }
    }
    text
}

fn decision_status(decision: &ApprovalDecision) -> String {
    match decision {
        ApprovalDecision::ApprovedOnce => "approve_once".into(),
        ApprovalDecision::ApprovedForRun => "approve_for_run".into(),
        ApprovalDecision::Denied => "deny".into(),
        ApprovalDecision::Cancelled => "cancelled".into(),
    }
}

/// 渲染态条目种类（纯数据，非 wire 类型，不进帧）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineEntryKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ToolCall { name: String, status: String, detail: Option<String> },
    RunState(String),
    Error(String),
}

/// 合法 Desktop fork 边界（纯数据，非 wire 类型）。R6/ADR-040 D5：fork 只许
/// 切在闭合 turn 边界，因此仅历史 `RunCompleted/RunCancelled/RunFailed` 与
/// live `RunState::{Completed,Cancelled,Failed}` 产生该标记；`RunStarted`、
/// `Interrupted`、message / tool / diagnostic 一律不是边界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkBoundary {
    Completed,
    Cancelled,
    Failed,
}

/// 渲染态时间线条目（纯数据，非 wire 类型，不进帧）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    pub sequence: u64,
    pub event_id: String,
    pub kind: TimelineEntryKind,
    /// reducer 单点判型的 fork 边界标记；Desktop 不复制事件词表再判一遍。
    pub fork_boundary: Option<ForkBoundary>,
    pub timestamp: String,
    pub run_id: Option<String>,
}

impl TimelineEntry {
    /// 是否为合法 Desktop fork 边界。connected / active session 由调用方
    /// 另行校验；判定只依赖本标记，禁止对 `kind` 文案做字符串匹配。
    pub fn is_fork_boundary(&self) -> bool {
        self.fork_boundary.is_some()
    }
}

/// Assistant 流式合并锚点：同一 run + message 的 delta 追加到同一条目。
/// 用 event_id / sequence 回查，不存会因中间插入而失效的 index。
#[derive(Clone, Debug, PartialEq, Eq)]
struct AssistantAnchor {
    run_id: Option<String>,
    message_id: Option<String>,
    event_id: String,
    sequence: u64,
    /// 历史 committed 已用权威全文替换该 live message；后到的同 message delta
    /// 只标记 sequence 已消费，不得再次追加或新开条目。
    committed: bool,
}

/// Tool 条目锚点：ToolCompleted/ToolOutput 优先按 run + tool_call_id 回填；
/// 旧历史条目缺少身份时，仅在 run（及可用 name）内唯一候选时兼容回填。
/// 用 event_id / sequence 回查，不存会因中间插入而失效的 index。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolAnchor {
    run_id: Option<String>,
    tool_call_id: Option<String>,
    name: Option<String>,
    event_id: String,
    sequence: u64,
}

struct TimelineIdentity {
    event_id: String,
    sequence: u64,
}

/// Timeline 投影 reducer：历史臂（[`TimelineItem`]）与 live 臂
/// （[`AppEventEnvelope`]）共用去重 / 有序插入 / 锚点合并核。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimelineProjection {
    /// 渲染态条目（按 session sequence 升序）。
    pub entries: Vec<TimelineEntry>,
    /// 已消费的 session sequence（live 与分页共用的去重集）。
    seen: BTreeSet<u64>,
    assistant_anchor: Option<AssistantAnchor>,
    tool_anchors: Vec<ToolAnchor>,
}

impl Deref for TimelineProjection {
    type Target = [TimelineEntry];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl TimelineProjection {
    /// 合并单条历史条目（分页臂）。历史中的 assistant 形状是「delta 序列 +
    /// 末尾 committed 消息」：delta 逐段合并，committed 到达时以提交文本替换
    /// 累积文本，保证历史回放不双份渲染。
    pub fn apply_item(&mut self, item: &TimelineItem) {
        match &item.kind {
            TimelineItemKind::UserMessage => {
                if self.seen.insert(item.sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::UserMessage {
                            text: item.text.clone().unwrap_or_default(),
                        },
                        fork_boundary: None,
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::AssistantDelta => {
                if self.seen.insert(item.sequence) {
                    self.append_assistant_delta(
                        item.sequence,
                        item.event_id.clone(),
                        item.timestamp.clone(),
                        item.run_id.as_deref().unwrap_or_default(),
                        None,
                        item.text.as_deref().unwrap_or_default(),
                    );
                }
            }
            TimelineItemKind::AssistantMessage => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let committed = item.text.clone().unwrap_or_default();
                let matching_anchor = self.assistant_anchor.clone().filter(|anchor| {
                    anchor.run_id == item.run_id && anchor.sequence < item.sequence
                });
                if let Some(anchor) = matching_anchor {
                        if let Some(index) =
                            self.entry_index_by_identity(&anchor.event_id, anchor.sequence)
                        {
                            if matches!(
                                self.entries.get(index).map(|entry| &entry.kind),
                                Some(TimelineEntryKind::AssistantMessage { .. })
                            ) {
                            // committed 采用自己的 sequence；必须移除后重新按序
                            // 插入，不能原位改 sequence，否则中间到达的 tool/run
                            // 条目会让 entries 失序。
                            let replacement = TimelineEntry {
                                sequence: item.sequence,
                                event_id: item.event_id.clone(),
                                kind: TimelineEntryKind::AssistantMessage { text: committed },
                                fork_boundary: None,
                                timestamp: item.timestamp.clone(),
                                run_id: item.run_id.clone(),
                            };
                            self.entries.remove(index);
                            self.insert_entry(replacement);
                            // live anchor 携带 message_id：保留为 committed tombstone，
                            // 吞掉已包含在权威全文中的迟到同-message delta；纯历史
                            // anchor 无 message_id，直接结束，避免下一轮复用。
                            self.assistant_anchor =
                                anchor.message_id.map(|message_id| AssistantAnchor {
                                    run_id: item.run_id.clone(),
                                    message_id: Some(message_id),
                                    event_id: item.event_id.clone(),
                                    sequence: item.sequence,
                                    committed: true,
                                });
                                return;
                            }
                        }
                    }
                self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.clone(),
                    kind: TimelineEntryKind::AssistantMessage { text: committed },
                    fork_boundary: None,
                    timestamp: item.timestamp.clone(),
                    run_id: item.run_id.clone(),
                });
                // 较新的 live anchor 可能先于较旧历史页到达；旧 committed 不得
                // 清掉它。只有同 run 且早于当前 committed 的失效锚点才收口。
                if self.assistant_anchor.as_ref().is_some_and(|anchor| {
                    anchor.run_id == item.run_id && anchor.sequence < item.sequence
                }) {
                    self.assistant_anchor = None;
                }
            }
            TimelineItemKind::ToolStarted => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let (tool_call_id, display_detail) =
                    parse_tool_timeline_context(item.detail.as_deref());
                let name = item.tool_name.clone().unwrap_or_else(|| "tool".into());
                self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.clone(),
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: item.status.clone().unwrap_or_else(|| "running".into()),
                        detail: display_detail,
                    },
                    fork_boundary: None,
                    timestamp: item.timestamp.clone(),
                    run_id: item.run_id.clone(),
                });
                if let Some(identity) = self.anchor_after_insert(&item.event_id, item.sequence) {
                    self.tool_anchors.push(ToolAnchor {
                        run_id: item.run_id.clone(),
                        tool_call_id,
                        name: Some(name),
                        event_id: identity.event_id,
                        sequence: identity.sequence,
                    });
                }
            }
            TimelineItemKind::ToolOutput => {
                if self.seen.insert(item.sequence) {
                    let (tool_call_id, _) = parse_tool_timeline_context(item.detail.as_deref());
                    self.update_tool_entry(
                        item.run_id.as_deref(),
                        tool_call_id.as_deref(),
                        item.tool_name.as_deref(),
                        None,
                        Some(item.text.as_deref().unwrap_or_default()),
                    );
                }
            }
            TimelineItemKind::ToolCompleted => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let status = item.status.as_deref().unwrap_or("succeeded");
                let (tool_call_id, display_detail) =
                    parse_tool_timeline_context(item.detail.as_deref());
                if self.update_tool_entry(
                    item.run_id.as_deref(),
                    tool_call_id.as_deref(),
                    item.tool_name.as_deref(),
                    Some(status),
                    display_detail.as_deref(),
                ) {
                    return;
                } else {
                    let name = item.tool_name.clone().unwrap_or_else(|| "tool".into());
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::ToolCall {
                            name: name.clone(),
                            status: status.to_string(),
                            detail: display_detail,
                        },
                        fork_boundary: None,
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::RunStarted
            | TimelineItemKind::RunCompleted
            | TimelineItemKind::RunCancelled => {
                if self.seen.insert(item.sequence) {
                    let (state, fork_boundary) = match item.kind {
                        TimelineItemKind::RunStarted => ("started", None),
                        TimelineItemKind::RunCompleted => {
                            ("completed", Some(ForkBoundary::Completed))
                        }
                        TimelineItemKind::RunCancelled => {
                            ("cancelled", Some(ForkBoundary::Cancelled))
                        }
                        _ => unreachable!("matched arm guarantees run kind"),
                    };
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::RunState(format!("run {state}")),
                        fork_boundary,
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::RunFailed => {
                if self.seen.insert(item.sequence) {
                    let reason = item.detail.as_deref().unwrap_or_default();
                    let label = if reason.is_empty() {
                        "run failed".to_string()
                    } else {
                        format!("run failed · {reason}")
                    };
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::RunState(label),
                        fork_boundary: Some(ForkBoundary::Failed),
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::ApprovalRequested => {
                if self.seen.insert(item.sequence) {
                    let tool = item.tool_name.as_deref().unwrap_or("tool");
                    let reason = item
                        .text
                        .as_deref()
                        .or(item.detail.as_deref())
                        .unwrap_or_default();
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::RunState(if reason.is_empty() {
                            format!("approval requested · {tool}")
                        } else {
                            format!("approval requested · {tool} · {reason}")
                        }),
                        fork_boundary: None,
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::ApprovalResponded => {
                if self.seen.insert(item.sequence) {
                    let decision = item
                        .status
                        .as_deref()
                        .or(item.detail.as_deref())
                        .or(item.text.as_deref())
                        .unwrap_or("responded");
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::RunState(format!("approval {decision}")),
                        fork_boundary: None,
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::Diagnostic => {
                // 持久化 Diagnostic 没有 level；与 live 臂保持一致，只把
                // sandbox.fallback 作为运行提示展示。其它信息诊断（例如
                // resources.injected）不是用户错误，不能在重放后变成 Error。
                if let Some(message) =
                    historical_sandbox_fallback_message(item.detail.as_deref())
                {
                    if self.seen.insert(item.sequence) {
                        self.insert_entry(TimelineEntry {
                            sequence: item.sequence,
                            event_id: item.event_id.clone(),
                            kind: TimelineEntryKind::RunState(sandbox_fallback_label(message)),
                            fork_boundary: None,
                            timestamp: item.timestamp.clone(),
                            run_id: item.run_id.clone(),
                        });
                    }
                }
            }
            TimelineItemKind::Other => {}
        }
    }

    /// 应用一条 live 事件（wire 臂）；返回 entries 是否变化（用于 UI 自动滚底）。
    ///
    /// 只处理时间线语义；审批卡 / 模型切换 / run 跟踪等 UI 态由消费方适配层
    /// 自行处理。run 态文案与历史臂统一（见 [`run_state_label`]）。
    pub fn apply_event(&mut self, envelope: &AppEventEnvelope) -> bool {
        let sequence = envelope.stream_sequence;
        let event_id = envelope.event_id.as_str().to_string();
        let timestamp = envelope.timestamp.as_unix_millis().to_string();
        match &envelope.payload {
            AppEvent::RunChanged { run_id, state } => {
                if self.seen.insert(sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::RunState(format!(
                            "run {}",
                            run_state_label(state)
                        )),
                        fork_boundary: live_fork_boundary(state),
                        timestamp,
                        run_id: Some(run_id.as_str().to_string()),
                    });
                    return true;
                }
            }
            AppEvent::AssistantDelta { run_id, message_id, delta } => {
                if !self.seen.insert(sequence) {
                    return false;
                }
                return self.append_assistant_delta(
                    sequence,
                    event_id,
                    timestamp,
                    run_id.as_str(),
                    Some(message_id.as_str()),
                    delta,
                );
            }
            AppEvent::ToolStarted { run_id, tool_call_id, name } => {
                if !self.seen.insert(sequence) {
                    // 历史页可能先以同 sequence 建立条目；live 重放虽然不应
                    // 重复渲染，仍需补上历史 wire 未显式暴露的 tool_call_id，
                    // 让后续 live output/completed 精确命中同一锚点。
                    self.enrich_tool_anchor(sequence, run_id.as_str(), tool_call_id.as_str(), name);
                    return false;
                }
                let run = Some(run_id.as_str().to_string());
                self.insert_entry(TimelineEntry {
                    sequence,
                    event_id: event_id.clone(),
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: "running".into(),
                        detail: None,
                    },
                    fork_boundary: None,
                    timestamp,
                    run_id: run.clone(),
                });
                if let Some(anchor) = self.anchor_after_insert(&event_id, sequence) {
                    self.tool_anchors.push(ToolAnchor {
                        run_id: run,
                        tool_call_id: Some(tool_call_id.as_str().to_string()),
                        name: Some(name.clone()),
                        event_id: anchor.event_id,
                        sequence: anchor.sequence,
                    });
                }
                return true;
            }
            AppEvent::ToolOutput {
                run_id,
                tool_call_id,
                delta,
                ..
            } => {
                if !self.seen.insert(sequence) {
                    return false;
                }
                if self.update_tool_entry(
                    Some(run_id.as_str()),
                    Some(tool_call_id.as_str()),
                    None,
                    None,
                    Some(delta),
                ) {
                    return true;
                }
            }
            AppEvent::ToolCompleted {
                run_id,
                tool_call_id,
                success,
            } => {
                if !self.seen.insert(sequence) {
                    return false;
                }
                let status = if *success { "succeeded" } else { "failed" };
                if self.update_tool_entry(
                    Some(run_id.as_str()),
                    Some(tool_call_id.as_str()),
                    None,
                    Some(status),
                    None,
                ) {
                    return true;
                }
                    self.insert_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::ToolCall {
                            name: tool_call_id.as_str().to_string(),
                            status: status.into(),
                            detail: None,
                        },
                        fork_boundary: None,
                        timestamp,
                        run_id: Some(run_id.as_str().to_string()),
                    });
                    return true;
                }
            AppEvent::Diagnostic { code, message, .. } => {
                if code == "sandbox.fallback" && self.seen.insert(sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::RunState(sandbox_fallback_label(message)),
                        fork_boundary: None,
                        timestamp,
                        run_id: None,
                    });
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    /// resume 三态的基线语义：Replay 保留基线、SnapshotRequired 清基线、
    /// UpToDate 不动。消费 [`crate::compute_resume_disposition`] 的输出。
    pub fn apply_resume_disposition(&mut self, disposition: &ResumeDisposition) {
        match disposition {
            ResumeDisposition::Replay { .. } | ResumeDisposition::UpToDate { .. } => {}
            ResumeDisposition::SnapshotRequired { .. } => self.reset_baseline(),
        }
    }

    /// 清空时间线基线（切换 session / SnapshotRequired 重建前调用）。
    pub fn reset_baseline(&mut self) {
        self.entries.clear();
        self.seen.clear();
        self.assistant_anchor = None;
        self.tool_anchors.clear();
    }

    /// 追加 assistant delta：命中锚点则合并，否则新开条目。
    fn append_assistant_delta(
        &mut self,
        sequence: u64,
        event_id: String,
        timestamp: String,
        run_id: &str,
        message_id: Option<&str>,
        delta: &str,
    ) -> bool {
        let run = Some(run_id.to_string());
        let message = message_id.map(str::to_string);
        if let Some(anchor) = &self.assistant_anchor {
            if anchor.run_id == run && anchor.message_id == message {
                if anchor.committed {
                    return false;
                }
                if let Some(index) = self.entry_index_by_identity(&anchor.event_id, anchor.sequence)
                {
                    if let Some(TimelineEntryKind::AssistantMessage { text }) =
                        self.entries.get_mut(index).map(|entry| &mut entry.kind)
                    {
                        text.push_str(delta);
                        return true;
                    }
                }
            }
        }
        self.insert_entry(TimelineEntry {
            sequence,
            event_id: event_id.clone(),
            kind: TimelineEntryKind::AssistantMessage {
                text: delta.to_string(),
            },
            fork_boundary: None,
            timestamp,
            run_id: run.clone(),
        });
        if let Some(identity) = self.anchor_after_insert(&event_id, sequence) {
            self.assistant_anchor = Some(AssistantAnchor {
                run_id: run,
                message_id: message,
                event_id: identity.event_id,
                sequence: identity.sequence,
                committed: false,
            });
        }
        true
    }

    /// 按 sequence 有序插入（页数据可能晚于已到达的 live 事件）。
    fn insert_entry(&mut self, entry: TimelineEntry) {
        let position = self
            .entries
            .partition_point(|existing| existing.sequence < entry.sequence);
        self.entries.insert(position, entry);
    }

    /// reducer 内部按 identity 回查:先精确匹配 event_id(wire 与 Fork 路径的唯一
    /// 锚点),未命中才退回按 sequence 的首个命中兜底(此处不校验唯一性,仅依赖
    /// 库级 UNIQUE(session_id, sequence) 提供的实际不碰撞)。sequence 回退只服务
    /// 内部防御性查找,不改变「锚点只用 event_id」的对外语义(见 plan/R6 §2.4)。
    fn entry_index_by_identity(&self, event_id: &str, sequence: u64) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.event_id == event_id)
            .or_else(|| {
                self.entries
                    .iter()
                    .position(|entry| entry.sequence == sequence)
            })
    }

    /// `insert_entry` 之后按 identity 回查，避免使用插入时的瞬时 index。
    fn anchor_after_insert(&self, event_id: &str, sequence: u64) -> Option<TimelineIdentity> {
        let index = self.entry_index_by_identity(event_id, sequence)?;
        let entry = self.entries.get(index)?;
        Some(TimelineIdentity {
            event_id: entry.event_id.clone(),
            sequence: entry.sequence,
        })
    }

    fn enrich_tool_anchor(&mut self, sequence: u64, run_id: &str, tool_call_id: &str, name: &str) {
        let run = Some(run_id.to_string());
        if let Some(anchor) = self
            .tool_anchors
            .iter_mut()
            .find(|anchor| anchor.sequence == sequence && anchor.run_id == run)
        {
            anchor.tool_call_id = Some(tool_call_id.to_string());
            anchor.name = Some(name.to_string());
        }
    }

    /// 按 run + tool_call_id 精确回填；兼容旧历史条目时只接受唯一候选，
    /// 绝不把无身份 output 随意写进同 run 最近的并发工具。
    fn update_tool_entry(
        &mut self,
        run_id: Option<&str>,
        tool_call_id: Option<&str>,
        name: Option<&str>,
        new_status: Option<&str>,
        detail_delta: Option<&str>,
    ) -> bool {
        let run = run_id.map(str::to_string);
        let candidates = self
            .tool_anchors
            .iter()
            .enumerate()
            .filter(|(_, anchor)| anchor.run_id == run)
            .filter(|(_, anchor)| match tool_call_id {
                Some(expected) => anchor.tool_call_id.as_deref() == Some(expected),
                None => match name {
                    Some(expected) => anchor.name.as_deref() == Some(expected),
                    None => true,
                },
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let anchor_index = match candidates.as_slice() {
            [only] => *only,
            _ => return false,
        };
        let anchor = &self.tool_anchors[anchor_index];
        let event_id = anchor.event_id.clone();
        let sequence = anchor.sequence;
        let Some(index) = self.entry_index_by_identity(&event_id, sequence) else {
            return false;
        };
        if let Some(TimelineEntryKind::ToolCall { status, detail, .. }) =
            self.entries.get_mut(index).map(|entry| &mut entry.kind)
        {
            if let Some(next) = new_status {
                status.clear();
                status.push_str(next);
            }
            if let Some(delta) = detail_delta {
                if !delta.is_empty() {
                    let text = detail.take().unwrap_or_default();
                    let separator = if new_status.is_some() && !text.is_empty() {
                        "\n"
                    } else {
                        ""
                    };
                    detail.replace(text + separator + delta);
                }
            }
            if new_status.is_some() {
                self.tool_anchors.remove(anchor_index);
            }
            return true;
        }
        false
    }
}

/// live `RunChanged` 的 fork 边界映射（与历史臂 run 终态同集）：
/// `Interrupted` 在 UI 上同样终结 run 跟踪，但不属于 storage fork
/// 白名单（ADR-040 D5），不产生边界标记。
fn live_fork_boundary(state: &RunState) -> Option<ForkBoundary> {
    match state {
        RunState::Completed => Some(ForkBoundary::Completed),
        RunState::Cancelled => Some(ForkBoundary::Cancelled),
        RunState::Failed => Some(ForkBoundary::Failed),
        _ => None,
    }
}

/// run 态条目文案（live 与历史统一，CR08-08 根治点之一）：
/// `RunState::Created` 历史上曾按 wire 态渲染为 "run created"、分页臂渲染为
/// "run started"，重载后文案闪烁。统一按事件语义取 "started"；其余态保持
/// wire snake_case 名。
fn run_state_label(state: &RunState) -> &'static str {
    match state {
        RunState::Created => "started",
        RunState::PreparingContext => "preparing_context",
        RunState::WaitingForProvider => "waiting_for_provider",
        RunState::StreamingResponse => "streaming_response",
        RunState::CollectingToolCalls => "collecting_tool_calls",
        RunState::WaitingForApproval => "waiting_for_approval",
        RunState::ExecutingTools => "executing_tools",
        RunState::AppendingToolResults => "appending_tool_results",
        RunState::Completed => "completed",
        RunState::Cancelled => "cancelled",
        RunState::Failed => "failed",
        RunState::Interrupted => "interrupted",
    }
}

fn sandbox_fallback_label(message: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(message) {
        if let Some(text) = value.get("message").and_then(serde_json::Value::as_str) {
            return text.to_string();
        }
    }
    if message.is_empty() {
        "沙箱回退：隔离已降级".into()
    } else {
        message.to_string()
    }
}

fn historical_sandbox_fallback_message(detail: Option<&str>) -> Option<&str> {
    detail?.strip_prefix("sandbox.fallback: ")
}
