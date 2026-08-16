//! 纯 Rust 状态机投影：把 pawork-client 的 Snapshot / TimelinePage / AppEvent
//! 投影为 Desktop UI 可直接渲染的状态。
//!
//! 本模块不依赖 gpui / tokio / OS API（gui-design 四层约束）。
//! 时间线去重键：live 事件的 stream_sequence 与 TimelinePage item 的 sequence
//! 同为 session 事件 sequence（gui_host publish 把 AgentEvent sequence 写入
//! stream_sequence），因此按 sequence 去重即可覆盖「分页期间 live 事件先到」
//! 的重叠（gui-design §4.1 第 3 条）。

use std::collections::BTreeSet;

use pawork_client::{AppEvent, AppEventEnvelope, EventStream, Snapshot, TimelinePage};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected { instance_id: String },
    Disconnected { reason: String },
    Failed { reason: String },
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::Connecting
    }
}

impl ConnectionState {
    /// 侧栏连接状态文本（禁用原因用文字说明，不只靠颜色区分）。
    pub fn label(&self) -> String {
        match self {
            Self::Connecting => "Connecting…".into(),
            Self::Connected { instance_id } => format!("Connected · {instance_id}"),
            Self::Disconnected { reason } => format!("Disconnected · {reason}"),
            Self::Failed { reason } => format!("Connect failed · {reason}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineEntryKind {
    UserMessage { text: String },
    AssistantMessage { text: String },
    ToolCall { name: String, status: String, detail: Option<String> },
    RunState(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    pub sequence: u64,
    pub event_id: String,
    pub kind: TimelineEntryKind,
    pub timestamp: String,
    pub run_id: Option<String>,
}

/// Assistant 流式合并锚点：同一 run + message 的 delta 追加到同一条目。
#[derive(Clone, Debug, PartialEq, Eq)]
struct AssistantAnchor {
    run_id: Option<String>,
    message_id: Option<String>,
    index: usize,
}

/// Tool 条目锚点：ToolCompleted/ToolOutput 按 run + tool_call_id（live）或
/// run + tool_name（分页历史，TimelineItem 不携带 tool_call_id）回填。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolAnchor {
    run_id: Option<String>,
    tool_call_id: Option<String>,
    name: Option<String>,
    index: usize,
}

/// 从 TimelinePage item 解出的字段值（TimelineItem 类型未从 pawork-client
/// re-export，这里在调用点解构为纯值，保持业务依赖只有 pawork-client）。
struct HistoryItem<'a> {
    sequence: u64,
    event_id: &'a str,
    kind: &'a str,
    run_id: Option<&'a str>,
    text: Option<&'a str>,
    tool_name: Option<&'a str>,
    status: Option<&'a str>,
    detail: Option<&'a str>,
    timestamp: &'a str,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopProjection {
    pub connection: ConnectionState,
    pub sessions: Vec<SessionSummary>,
    pub workspace_id: Option<String>,
    pub active_session_id: Option<String>,
    pub active_run_id: Option<String>,
    pub timeline: Vec<TimelineEntry>,
    /// 已消费的 session sequence（live 与分页共用的去重集）。
    seen: BTreeSet<u64>,
    assistant_anchor: Option<AssistantAnchor>,
    tool_anchors: Vec<ToolAnchor>,
}

impl DesktopProjection {
    /// 从 Snapshot 全量重建（首连 / 重连重取）。
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        let mut projection = Self::default();
        projection.merge_snapshot(snapshot);
        projection
    }

    /// 用 Snapshot 的 session_tree / workspaces 段替换列表，保留连接状态、
    /// 打开的 session 与时间线。
    pub fn merge_snapshot(&mut self, snapshot: &Snapshot) {
        for section in &snapshot.sections {
            let kind = enum_name(serde_json::to_value(&section.kind).ok());
            let data = section.data.clone().unwrap_or(Value::Null);
            match kind.as_str() {
                "session_tree" => {
                    self.sessions = parse_sessions(&data);
                }
                "workspaces" => {
                    self.workspace_id = data
                        .as_array()
                        .and_then(|entries| entries.first())
                        .and_then(|entry| entry.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                _ => {}
            }
        }
    }

    /// 打开（切换）session：清空时间线与去重状态。
    pub fn select_session(&mut self, session_id: &str) {
        self.active_session_id = Some(session_id.to_string());
        self.active_run_id = None;
        self.timeline.clear();
        self.seen.clear();
        self.assistant_anchor = None;
        self.tool_anchors.clear();
    }

    pub fn set_connection(&mut self, state: ConnectionState) {
        self.connection = state;
    }

    /// 合并一页历史时间线（按 sequence 去重，保持 sequence 升序）。
    pub fn apply_timeline_page(&mut self, page: &TimelinePage) {
        for item in &page.items {
            self.merge_history_item(HistoryItem {
                sequence: item.sequence,
                event_id: item.event_id.as_str(),
                kind: &enum_name(serde_json::to_value(&item.kind).ok()),
                run_id: item.run_id.as_deref(),
                text: item.text.as_deref(),
                tool_name: item.tool_name.as_deref(),
                status: item.status.as_deref(),
                detail: item.detail.as_deref(),
                timestamp: item.timestamp.as_str(),
            });
        }
    }

    /// 应用一条 live 事件；返回时间线是否发生变化（用于 UI 自动滚底）。
    pub fn apply_event(&mut self, envelope: &AppEventEnvelope) -> bool {
        let Some(active) = self.active_session_id.as_deref() else {
            return false;
        };
        match &envelope.stream {
            EventStream::Session(session_id) if session_id.as_str() == active => {}
            _ => return false,
        }
        let sequence = envelope.stream_sequence;
        let event_id = envelope.event_id.as_str().to_string();
        let timestamp = envelope.timestamp.0.to_string();
        match &envelope.payload {
            AppEvent::RunChanged { run_id, state } => {
                let state_name = enum_name(serde_json::to_value(state).ok());
                let run_id = Some(run_id.as_str().to_string());
                if matches!(
                    state_name.as_str(),
                    "completed" | "cancelled" | "failed" | "interrupted"
                ) {
                    if self.active_run_id.as_deref() == run_id.as_deref() {
                        self.active_run_id = None;
                    }
                } else {
                    self.active_run_id = run_id.clone();
                }
                if self.seen.insert(sequence) {
                    self.push_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::RunState(format!("run {state_name}")),
                        timestamp,
                        run_id,
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
                let index = self.insert_entry(TimelineEntry {
                    sequence,
                    event_id,
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: "running".into(),
                        detail: None,
                    },
                    timestamp,
                    run_id: run.clone(),
                });
                self.tool_anchors.push(ToolAnchor {
                    run_id: run,
                    tool_call_id: Some(tool_call_id.as_str().to_string()),
                    name: Some(name.clone()),
                    index,
                });
                return true;
            }
            AppEvent::ToolCompleted { run_id, tool_call_id, success } => {
                let status = if *success { "succeeded" } else { "failed" };
                let run = run_id.as_str();
                if self.update_tool_entry(
                    Some(run),
                    Some(tool_call_id.as_str()),
                    None,
                    Some(status),
                    None,
                ) {
                    self.seen.insert(sequence);
                    return true;
                }
                if self.seen.insert(sequence) {
                    self.push_entry(TimelineEntry {
                        sequence,
                        event_id,
                        kind: TimelineEntryKind::ToolCall {
                            name: tool_call_id.as_str().to_string(),
                            status: status.into(),
                            detail: None,
                        },
                        timestamp,
                        run_id: Some(run.to_string()),
                    });
                    return true;
                }
            }
            _ => {}
        }
        false
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
            if anchor.run_id == run
                && anchor.message_id == message
                && matches!(
                    self.timeline.get(anchor.index).map(|entry| &entry.kind),
                    Some(TimelineEntryKind::AssistantMessage { .. })
                )
            {
                if let Some(TimelineEntryKind::AssistantMessage { text }) =
                    self.timeline.get_mut(anchor.index).map(|entry| &mut entry.kind)
                {
                    text.push_str(delta);
                    return true;
                }
            }
        }
        let index = self.insert_entry(TimelineEntry {
            sequence,
            event_id,
            kind: TimelineEntryKind::AssistantMessage {
                text: delta.to_string(),
            },
            timestamp,
            run_id: run.clone(),
        });
        self.assistant_anchor = Some(AssistantAnchor {
            run_id: run,
            message_id: message,
            index,
        });
        true
    }

    /// 合并单条历史条目。历史中的 assistant 形状是「delta 序列 + 末尾 committed
    /// 消息」：delta 逐段合并，committed 到达时以提交文本替换累积文本，保证
    /// 历史回放不双份渲染。
    fn merge_history_item(&mut self, item: HistoryItem<'_>) {
        match item.kind {
            "user_message" => {
                if self.seen.insert(item.sequence) {
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::UserMessage {
                            text: item.text.unwrap_or_default().to_string(),
                        },
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "assistant_delta" => {
                if self.seen.insert(item.sequence) {
                    self.append_assistant_delta(
                        item.sequence,
                        item.event_id.to_string(),
                        item.timestamp.to_string(),
                        item.run_id.unwrap_or_default(),
                        None,
                        item.text.unwrap_or_default(),
                    );
                }
            }
            "assistant_message" => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let run = item.run_id.map(str::to_string);
                let committed = item.text.unwrap_or_default().to_string();
                if let Some(anchor) = &self.assistant_anchor {
                    if anchor.run_id == run
                        && anchor.message_id.is_none()
                        && matches!(
                            self.timeline.get(anchor.index).map(|entry| &entry.kind),
                            Some(TimelineEntryKind::AssistantMessage { .. })
                        )
                    {
                        let index = anchor.index;
                        if let Some(entry) = self.timeline.get_mut(index) {
                            entry.sequence = item.sequence;
                            entry.event_id = item.event_id.to_string();
                            entry.timestamp = item.timestamp.to_string();
                            entry.kind = TimelineEntryKind::AssistantMessage { text: committed };
                        }
                        return;
                    }
                }
                let index = self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.to_string(),
                    kind: TimelineEntryKind::AssistantMessage { text: committed },
                    timestamp: item.timestamp.to_string(),
                    run_id: run.clone(),
                });
                self.assistant_anchor = Some(AssistantAnchor {
                    run_id: run,
                    message_id: None,
                    index,
                });
            }
            "tool_started" => {
                if !self.seen.insert(item.sequence) {
                    return;
                }
                let run = item.run_id.map(str::to_string);
                let name = item.tool_name.unwrap_or("tool").to_string();
                let index = self.insert_entry(TimelineEntry {
                    sequence: item.sequence,
                    event_id: item.event_id.to_string(),
                    kind: TimelineEntryKind::ToolCall {
                        name: name.clone(),
                        status: item.status.unwrap_or("running").to_string(),
                        detail: None,
                    },
                    timestamp: item.timestamp.to_string(),
                    run_id: run.clone(),
                });
                self.tool_anchors.push(ToolAnchor {
                    run_id: run,
                    tool_call_id: None,
                    name: Some(name),
                    index,
                });
            }
            "tool_output" => {
                if self.seen.insert(item.sequence) {
                    self.update_tool_entry(
                        item.run_id,
                        None,
                        item.tool_name,
                        None,
                        Some(item.text.unwrap_or_default()),
                    );
                }
            }
            "tool_completed" => {
                let status = item.status.unwrap_or("succeeded");
                if self.update_tool_entry(item.run_id, None, item.tool_name, Some(status), None) {
                    self.seen.insert(item.sequence);
                } else if self.seen.insert(item.sequence) {
                    let run = item.run_id.map(str::to_string);
                    let name = item.tool_name.unwrap_or("tool").to_string();
                    let index = self.insert_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::ToolCall {
                            name: name.clone(),
                            status: status.to_string(),
                            detail: item.detail.map(str::to_string),
                        },
                        timestamp: item.timestamp.to_string(),
                        run_id: run.clone(),
                    });
                    self.tool_anchors.push(ToolAnchor {
                        run_id: run,
                        tool_call_id: None,
                        name: Some(name),
                        index,
                    });
                }
            }
            "run_started" | "run_completed" | "run_cancelled" => {
                if self.seen.insert(item.sequence) {
                    let state = item.kind.trim_start_matches("run_");
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::RunState(format!("run {state}")),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "run_failed" => {
                if self.seen.insert(item.sequence) {
                    let reason = item.detail.unwrap_or_default();
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::RunState(format!("run failed · {reason}")),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            "diagnostic" => {
                if self.seen.insert(item.sequence) {
                    self.push_entry(TimelineEntry {
                        sequence: item.sequence,
                        event_id: item.event_id.to_string(),
                        kind: TimelineEntryKind::Error(
                            item.detail.unwrap_or_default().to_string(),
                        ),
                        timestamp: item.timestamp.to_string(),
                        run_id: item.run_id.map(str::to_string),
                    });
                }
            }
            _ => {}
        }
    }

    fn push_entry(&mut self, entry: TimelineEntry) {
        self.timeline.push(entry);
    }

    /// 按 sequence 有序插入（页数据可能晚于已到达的 live 事件）。
    fn insert_entry(&mut self, entry: TimelineEntry) -> usize {
        let position = self
            .timeline
            .partition_point(|existing| existing.sequence < entry.sequence);
        self.timeline.insert(position, entry);
        position
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
            Some(anchor.index)
        });
        let Some(index) = found else {
            return false;
        };
        if let Some(TimelineEntryKind::ToolCall { status, detail, .. }) =
            self.timeline.get_mut(index).map(|entry| &mut entry.kind)
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

/// unit enum（SnapshotSectionKind / TimelineItemKind / RunState）的 serde 名。
/// serde 不是本 crate 依赖：调用点先用 serde_json::to_value 序列化（泛型约束
/// 在调用点解析，无需命名 serde trait），这里只取字符串形态。
fn enum_name(json: Option<Value>) -> String {
    json.and_then(|json| json.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn parse_sessions(data: &Value) -> Vec<SessionSummary> {
    let mut sessions = Vec::new();
    if let Some(entries) = data.as_array() {
        for entry in entries {
            let Some(session_id) = entry.get("session_id").and_then(Value::as_str) else {
                continue;
            };
            sessions.push(SessionSummary {
                session_id: session_id.to_string(),
                title: entry
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled")
                    .to_string(),
                updated_at_ms: entry
                    .get("updated_at_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }
    }
    sessions.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    sessions
}

/// 供 controller / probe 复用的 snapshot 解析。
pub fn sessions_in_snapshot(snapshot: &Snapshot) -> Vec<SessionSummary> {
    let mut projection = DesktopProjection::default();
    projection.merge_snapshot(snapshot);
    projection.sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot_with_sessions(entries: Vec<Value>) -> Snapshot {
        serde_json::from_value(json!({
            "instance_id": "instance-1",
            "snapshot_sequence": 0,
            "generated_at": 1,
            "sections": [
                {
                    "kind": "workspaces",
                    "revision": 1,
                    "data": [{ "id": "ws-default", "trusted": true }]
                },
                { "kind": "session_tree", "revision": 2, "data": entries }
            ]
        }))
        .expect("decode Snapshot")
    }

    fn session_entry(id: &str, title: &str, updated: u64) -> Value {
        json!({
            "session_id": id,
            "title": title,
            "created_at_ms": 1,
            "updated_at_ms": updated,
            "active_branch": "main",
            "archived": false
        })
    }

    fn event(sequence: u64, payload: Value) -> AppEventEnvelope {
        serde_json::from_value(json!({
            "api_version": { "major": 1, "minor": 1 },
            "instance_id": "instance-1",
            "event_id": format!("app-{sequence}"),
            "global_sequence": sequence,
            "stream": { "type": "session", "id": "s-1" },
            "stream_sequence": sequence,
            "timestamp": 1_000 + sequence,
            "source": { "type": "core" },
            "payload": payload
        }))
        .expect("decode AppEventEnvelope")
    }

    fn run_changed(sequence: u64, state: &str) -> AppEventEnvelope {
        event(
            sequence,
            json!({ "type": "run_changed", "data": { "run_id": "r-1", "state": state } }),
        )
    }

    fn assistant_delta(sequence: u64, message_id: &str, delta: &str) -> AppEventEnvelope {
        event(
            sequence,
            json!({
                "type": "assistant_delta",
                "data": { "run_id": "r-1", "message_id": message_id, "delta": delta }
            }),
        )
    }

    fn page(items: Vec<Value>, complete: bool) -> TimelinePage {
        serde_json::from_value(json!({
            "items": items,
            "head_sequence": items.len() as u64,
            "complete": complete
        }))
        .expect("decode TimelinePage")
    }

    fn history_item(sequence: u64, kind: &str, extra: Value) -> Value {
        let mut item = json!({
            "sequence": sequence,
            "event_id": format!("hist-{sequence}"),
            "kind": kind,
            "run_id": "r-1",
            "timestamp": "2000"
        });
        if let Some(fields) = extra.as_object() {
            for (key, value) in fields {
                item[key] = value.clone();
            }
        }
        item
    }

    #[test]
    fn snapshot_rebuilds_sessions_and_events_rebuild_timeline() {
        let snapshot = snapshot_with_sessions(vec![
            session_entry("s-old", "Old", 10),
            session_entry("s-new", "New", 20),
        ]);
        let mut projection = DesktopProjection::from_snapshot(&snapshot);
        assert_eq!(projection.workspace_id.as_deref(), Some("ws-default"));
        // 按 updated_at_ms 倒序，最新 session 在最前。
        assert_eq!(projection.sessions[0].session_id, "s-new");
        assert_eq!(projection.sessions.len(), 2);

        projection.set_connection(ConnectionState::Connected {
            instance_id: "instance-1".into(),
        });
        projection.select_session("s-1");

        assert!(projection.apply_event(&run_changed(1, "created")));
        assert!(projection.apply_event(&assistant_delta(2, "m-1", "Hello ")));
        assert!(projection.apply_event(&assistant_delta(3, "m-1", "world")));
        assert!(projection.apply_event(&run_changed(4, "completed")));
        // 终态清空 active_run_id，Composer 恢复可用。
        assert_eq!(projection.active_run_id, None);

        let texts: Vec<String> = projection
            .timeline
            .iter()
            .map(|entry| match &entry.kind {
                TimelineEntryKind::AssistantMessage { text } => format!("assistant:{text}"),
                TimelineEntryKind::RunState(state) => format!("run:{state}"),
                other => format!("other:{other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                "run:run created".to_string(),
                "assistant:Hello world".to_string(),
                "run:run completed".to_string()
            ]
        );
    }

    #[test]
    fn assistant_deltas_merge_until_message_or_run_changes() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");

        projection.apply_event(&assistant_delta(1, "m-1", "a"));
        projection.apply_event(&assistant_delta(2, "m-1", "b"));
        projection.apply_event(&assistant_delta(3, "m-2", "c"));
        assert_eq!(projection.timeline.len(), 2);
        let texts: Vec<&str> = projection
            .timeline
            .iter()
            .map(|entry| match &entry.kind {
                TimelineEntryKind::AssistantMessage { text } => text.as_str(),
                _ => "other",
            })
            .collect();
        assert_eq!(texts, vec!["ab", "c"]);
    }

    #[test]
    fn timeline_pages_dedup_by_sequence_and_merge_committed_text() {
        let mut projection = DesktopProjection::default();
        projection.select_session("s-1");

        let first = page(
            vec![
                history_item(1, "user_message", json!({ "text": "hi" })),
                history_item(2, "assistant_delta", json!({ "text": "He" })),
                history_item(3, "assistant_delta", json!({ "text": "llo" })),
                history_item(4, "assistant_message", json!({ "text": "Hello" })),
            ],
            false,
        );
        projection.apply_timeline_page(&first);
        // 重放同一页：sequence 去重，条目数不变。
        projection.apply_timeline_page(&first);
        assert_eq!(projection.timeline.len(), 2);
        assert!(matches!(
            &projection.timeline[1].kind,
            TimelineEntryKind::AssistantMessage { text } if text == "Hello"
        ));
        // committed 替换后条目携带 committed 的 sequence。
        assert_eq!(projection.timeline[1].sequence, 4);

        let second = page(
            vec![
                history_item(3, "assistant_delta", json!({ "text": "llo" })),
                history_item(
                    5,
                    "tool_started",
                    json!({ "tool_name": "fs_read", "status": "running" }),
                ),
                history_item(6, "tool_output", json!({ "text": "42 bytes" })),
                history_item(
                    7,
                    "tool_completed",
                    json!({ "tool_name": "fs_read", "status": "succeeded" }),
                ),
            ],
            true,
        );
        projection.apply_timeline_page(&second);
        assert_eq!(projection.timeline.len(), 3);
        assert!(matches!(
            &projection.timeline[2].kind,
            TimelineEntryKind::ToolCall { name, status, detail }
                if name == "fs_read" && status == "succeeded" && detail.as_deref() == Some("42 bytes")
        ));

        // 页数据之外先到的 live 事件重放（同 sequence）不再重复。
        assert!(!projection.apply_event(&assistant_delta(2, "m-1", "He")));
    }
}
