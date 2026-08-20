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
//!   （partition_point）、assistant delta 合并与 committed 替换、tool 双键锚点
//!   （live `run+tool_call_id` / 历史 `run+tool_name`）全部单一实现；
//! - resume 三态：Replay 保留基线、SnapshotRequired 清基线、UpToDate 不动。
//!
//! 本模块是纯数据投影，不进 wire 帧：[`TimelineEntry`] 系列不加 serde derive，
//! 序列化只发生在测试渲染层。分页游标元数据（next_sequence/complete）留在
//! 消费方适配层，reducer 只管 entries 语义。

use std::collections::BTreeSet;
use std::ops::Deref;

use pawork_domain::{AgentEvent, AgentEventEnvelope, ApprovalDecision, ContentPart, MessageRole};

use crate::ResumeDisposition;
use crate::app::{AppEvent, AppEventEnvelope, RunState, TimelineItem, TimelineItemKind};

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
        AgentEvent::ToolCallStarted { name, .. } => (
            TimelineItemKind::ToolStarted,
            None,
            Some(name.clone()),
            Some("running".into()),
            None,
        ),
        AgentEvent::ToolOutputDelta { delta, .. } => (
            TimelineItemKind::ToolOutput,
            Some(delta.clone()),
            None,
            None,
            None,
        ),
        AgentEvent::ToolExecutionCompleted { result, .. } => (
            TimelineItemKind::ToolCompleted,
            Some(join_text(&result.content)),
            result.tool_name.clone(),
            Some(if result.is_error { "failed" } else { "succeeded" }.into()),
            sandbox_timeline_detail(&result.metadata),
        ),
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

/// 渲染态时间线条目（纯数据，非 wire 类型，不进帧）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    pub sequence: u64,
    pub event_id: String,
    pub kind: TimelineEntryKind,
    pub timestamp: String,
    pub run_id: Option<String>,
}

/// Assistant 流式合并锚点：同一 run + message 的 delta 追加到同一条目。
/// 用 event_id / sequence 回查，不存会因中间插入而失效的 index。
#[derive(Clone, Debug, PartialEq, Eq)]
struct AssistantAnchor {
    run_id: Option<String>,
    message_id: Option<String>,
    event_id: String,
    sequence: u64,
}

/// Tool 条目锚点：ToolCompleted/ToolOutput 按 run + tool_call_id（live）或
/// run + tool_name（分页历史，TimelineItem 不携带 tool_call_id）回填。
/// 双键策略是既有语义：live 臂拿 wire tool_call_id，历史臂只有 tool_name，
/// 两侧键空间不相交，由同一 update_tool_entry 合并核消费。
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
                if let Some(anchor) = &self.assistant_anchor {
                    if anchor.run_id == item.run_id && anchor.message_id.is_none() {
                        if let Some(index) =
                            self.entry_index_by_identity(&anchor.event_id, anchor.sequence)
                        {
                            if matches!(
                                self.entries.get(index).map(|entry| &entry.kind),
                                Some(TimelineEntryKind::AssistantMessage { .. })
                            ) {
                                if let Some(entry) = self.entries.get_mut(index) {
                                    entry.sequence = item.sequence;
                                    entry.event_id = item.event_id.clone();
                                    entry.timestamp = item.timestamp.clone();
                                    entry.kind =
                                        TimelineEntryKind::AssistantMessage { text: committed };
                                }
                                let anchor = self.assistant_anchor.as_mut().expect("anchor");
                                anchor.event_id = item.event_id.clone();
                                anchor.sequence = item.sequence;
                                return;
                            }
                        }
                    }
                }
                self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.clone(),
                    kind: TimelineEntryKind::AssistantMessage { text: committed },
                    timestamp: item.timestamp.clone(),
                    run_id: item.run_id.clone(),
                });
                if let Some(identity) = self.anchor_after_insert(&item.event_id, item.sequence) {
                    self.assistant_anchor = Some(AssistantAnchor {
                        run_id: item.run_id.clone(),
                        message_id: None,
                        event_id: identity.event_id,
                        sequence: identity.sequence,
                    });
                }
            }
            TimelineItemKind::ToolStarted => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let name = item
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool".into());
                self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.clone(),
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: item
                            .status
                            .clone()
                            .unwrap_or_else(|| "running".into()),
                        detail: None,
                    },
                    timestamp: item.timestamp.clone(),
                    run_id: item.run_id.clone(),
                });
                if let Some(identity) = self.anchor_after_insert(&item.event_id, item.sequence) {
                    self.tool_anchors.push(ToolAnchor {
                        run_id: item.run_id.clone(),
                        tool_call_id: None,
                        name: Some(name),
                        event_id: identity.event_id,
                        sequence: identity.sequence,
                    });
                }
            }
            TimelineItemKind::ToolOutput => {
                if self.seen.insert(item.sequence) {
                    self.update_tool_entry(
                        item.run_id.as_deref(),
                        None,
                        item.tool_name.as_deref(),
                        None,
                        Some(item.text.as_deref().unwrap_or_default()),
                    );
                }
            }
            TimelineItemKind::ToolCompleted => {
                let status = item.status.as_deref().unwrap_or("succeeded");
                if self.update_tool_entry(
                    item.run_id.as_deref(),
                    None,
                    item.tool_name.as_deref(),
                    Some(status),
                    None,
                ) {
                    self.seen.insert(item.sequence);
                } else if self.seen.insert(item.sequence) {
                    let name = item
                        .tool_name
                        .clone()
                        .unwrap_or_else(|| "tool".into());
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::ToolCall {
                            name: name.clone(),
                            status: status.to_string(),
                            detail: item.detail.clone(),
                        },
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                    if let Some(identity) =
                        self.anchor_after_insert(&item.event_id, item.sequence)
                    {
                        self.tool_anchors.push(ToolAnchor {
                            run_id: item.run_id.clone(),
                            tool_call_id: None,
                            name: Some(name),
                            event_id: identity.event_id,
                            sequence: identity.sequence,
                        });
                    }
                }
            }
            TimelineItemKind::RunStarted
            | TimelineItemKind::RunCompleted
            | TimelineItemKind::RunCancelled => {
                if self.seen.insert(item.sequence) {
                    let state = match item.kind {
                        TimelineItemKind::RunStarted => "started",
                        TimelineItemKind::RunCompleted => "completed",
                        TimelineItemKind::RunCancelled => "cancelled",
                        _ => unreachable!("matched arm guarantees run kind"),
                    };
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::RunState(format!("run {state}")),
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
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
                }
            }
            TimelineItemKind::Diagnostic => {
                if self.seen.insert(item.sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.clone(),
                        kind: TimelineEntryKind::Error(
                            item.detail.clone().unwrap_or_default(),
                        ),
                        timestamp: item.timestamp.clone(),
                        run_id: item.run_id.clone(),
                    });
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
                if self.update_tool_entry(
                    Some(run_id.as_str()),
                    Some(tool_call_id.as_str()),
                    None,
                    None,
                    Some(delta),
                ) {
                    self.seen.insert(sequence);
                    return true;
                }
            }
            AppEvent::ToolCompleted { run_id, tool_call_id, success } => {
                let status = if *success { "succeeded" } else { "failed" };
                if self.update_tool_entry(
                    Some(run_id.as_str()),
                    Some(tool_call_id.as_str()),
                    None,
                    Some(status),
                    None,
                ) {
                    self.seen.insert(sequence);
                    return true;
                }
                if self.seen.insert(sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::ToolCall {
                            name: tool_call_id.as_str().to_string(),
                            status: status.into(),
                            detail: None,
                        },
                        timestamp,
                        run_id: Some(run_id.as_str().to_string()),
                    });
                    return true;
                }
            }
            AppEvent::Diagnostic { code, message, .. } => {
                if code == "sandbox.fallback" && self.seen.insert(sequence) {
                    self.insert_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::RunState(sandbox_fallback_label(message)),
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
            timestamp,
            run_id: run.clone(),
        });
        if let Some(identity) = self.anchor_after_insert(&event_id, sequence) {
            self.assistant_anchor = Some(AssistantAnchor {
                run_id: run,
                message_id: message,
                event_id: identity.event_id,
                sequence: identity.sequence,
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

    /// 按 run + tool_call_id（live）或 run + tool_name（历史）回填 tool 条目。
    fn update_tool_entry(
        &mut self,
        run_id: Option<&str>,
        tool_call_id: Option<&str>,
        name: Option<&str>,
        new_status: Option<&str>,
        detail_delta: Option<&str>,
    ) -> bool {
        let run = run_id.map(str::to_string);
        let found = self.tool_anchors.iter().rev().find_map(|anchor| {
            if anchor.run_id != run {
                return None;
            }
            if let Some(expected) = tool_call_id {
                if anchor.tool_call_id.as_deref() != Some(expected) {
                    return None;
                }
            }
            if let Some(expected) = name {
                if anchor.name.as_deref() != Some(expected) {
                    return None;
                }
            }
            Some((anchor.event_id.clone(), anchor.sequence))
        });
        let Some((event_id, sequence)) = found else {
            return false;
        };
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
                    detail.replace(text + delta);
                }
            }
            return true;
        }
        false
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
