use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{AgentEventEnvelope, CancellationToken, EventId, RunId, SessionId};
use pawork_engine::{now_timestamp, AgentEventSink, EngineError};
use pawork_protocol::{
    AppEvent, AppEventEnvelope, DiagnosticLevel, EventSource, EventStream, GlobalSequence,
    RunState, API_VERSION,
};
use serde_json::Value;

use crate::{EventHub, HubError};

use super::events::broadcast_event;

/// 宿主合成 wire 事件（无持久化 sequence）的序号起点：从 2^60 递增自取。
/// 真实持久化 sequence 从 1 单调递增、不可能到达该段，因此合成序号既不会
/// 与真实事件在 reducer seen 去重集里碰撞（吞掉真实事件），又能让合成条目
/// 按 insert_entry 有序插入排在既有时间线内容之后——seq-0 旧行为会把合成
/// "Run failed" 摘要插到时间线顶端、压在用户消息乐观回显之上。
pub(in crate::gui_host) const SYNTHETIC_SEQUENCE_BASE: u64 = 1 << 60;

/// 单实例事件总线：内部唯一 EventHub，组信封后 `publish`，容量默认 4096。
pub struct GuiEventBus {
    hub: EventHub,
    revision: AtomicU64,
    next_event: AtomicU64,
    /// 合成事件序号分配器（publish_raw 专用，见 SYNTHETIC_SEQUENCE_BASE）。
    next_synthetic: AtomicU64,
    /// engine 已广播终态的 run 集合：宿主合成终态兜底据此去重，
    /// 避免 fail/cancel 路径在真实终态之后再补发幽灵 RunChanged{Failed}。
    terminal_reported: Mutex<HashSet<String>>,
}

impl GuiEventBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            hub: EventHub::with_capacity(capacity),
            revision: AtomicU64::new(1),
            next_event: AtomicU64::new(1),
            next_synthetic: AtomicU64::new(SYNTHETIC_SEQUENCE_BASE),
            terminal_reported: Mutex::new(HashSet::new()),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AppEventEnvelope> {
        self.hub.subscribe_receiver()
    }

    pub fn hub(&self) -> &EventHub {
        &self.hub
    }

    pub fn replay(
        &self,
        from: GlobalSequence,
        to: Option<GlobalSequence>,
    ) -> Result<Vec<AppEventEnvelope>, HubError> {
        self.hub.replay(from, to)
    }

    pub fn current_sequence(&self) -> u64 {
        self.hub.current().0
    }

    pub(in crate::gui_host) fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::Relaxed)
    }

    fn next_event_id(&self) -> EventId {
        let n = self.next_event.fetch_add(1, Ordering::Relaxed);
        EventId::from(format!("app-evt-{n}"))
    }

    /// engine 终态是否已上过实时流（Completed/Cancelled/Failed/Interrupted）。
    /// 仅 `publish`（GuiBroadcastSink 驱动）登记；`publish_raw` 的合成兜底不自登记。
    pub(in crate::gui_host) fn terminal_reported(&self, run_id: &str) -> bool {
        self.terminal_reported
            .lock()
            .expect("terminal reported set poisoned")
            .contains(run_id)
    }

    /// run 收尾后清除登记，防止集合无界增长。
    pub(in crate::gui_host) fn clear_terminal_reported(&self, run_id: &str) {
        self.terminal_reported
            .lock()
            .expect("terminal reported set poisoned")
            .remove(run_id);
    }

    fn publish(
        &self,
        instance: pawork_domain::CoreInstanceId,
        envelope: &AgentEventEnvelope,
        event: AppEvent,
    ) {
        if let AppEvent::RunChanged { run_id, state } = &event {
            if matches!(
                state,
                RunState::Completed
                    | RunState::Cancelled
                    | RunState::Failed
                    | RunState::Interrupted
            ) {
                self.terminal_reported
                    .lock()
                    .expect("terminal reported set poisoned")
                    .insert(run_id.as_str().to_string());
            }
        }
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Session(envelope.session_id.clone()),
            stream_sequence: envelope.sequence.0,
            timestamp: envelope.timestamp,
            source: EventSource::Core,
            payload: event,
        };
        self.hub.publish(app_envelope);
    }

    pub(in crate::gui_host) fn publish_raw(
        &self,
        instance: pawork_domain::CoreInstanceId,
        session_id: &SessionId,
        event: AppEvent,
    ) {
        // 合成事件不占真实持久化号段：序号从 SYNTHETIC_SEQUENCE_BASE 递增
        // 自取，保证有序插入落在既有时间线内容之后且互相之间保持到达序。
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Session(session_id.clone()),
            stream_sequence: self.next_synthetic.fetch_add(1, Ordering::Relaxed),
            timestamp: now_timestamp(),
            source: EventSource::Core,
            payload: event,
        };
        self.hub.publish(app_envelope);
    }

    pub(in crate::gui_host) fn publish_terminal(
        &self,
        instance: pawork_domain::CoreInstanceId,
        terminal_session_id: &str,
        event: AppEvent,
    ) {
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Terminal(terminal_session_id.to_string()),
            stream_sequence: 0,
            timestamp: now_timestamp(),
            source: EventSource::Core,
            payload: event,
        };
        self.hub.publish(app_envelope);
    }
}

impl GuiEventBus {
    pub fn publish_diagnostic(
        &self,
        instance: pawork_domain::CoreInstanceId,
        session_id: &SessionId,
        code: &str,
        details: Value,
    ) {
        self.publish_raw(
            instance,
            session_id,
            AppEvent::Diagnostic {
                level: DiagnosticLevel::Info,
                code: code.to_string(),
                message: details.to_string(),
            },
        );
    }

    /// Bus-level Lagged degrade: hub publish of a Diagnostic frame.
    pub fn publish_event_stream_lagged(
        &self,
        instance: pawork_domain::CoreInstanceId,
        missed: Option<u64>,
        client_id: Option<&str>,
    ) -> (pawork_protocol::AppEventEnvelope, usize) {
        self.hub.publish_lagged_degrade_envelope(instance, missed, client_id)
    }
}

/// GUI 侧渲染 sink：把 persist 之后的事件映射为 App 事件广播出去。
pub struct GuiBroadcastSink {
    bus: Arc<GuiEventBus>,
    instance: pawork_domain::CoreInstanceId,
}

impl GuiBroadcastSink {
    pub fn new(bus: Arc<GuiEventBus>, instance: pawork_domain::CoreInstanceId) -> Self {
        Self { bus, instance }
    }
}

#[async_trait]
impl AgentEventSink for GuiBroadcastSink {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        if let Some(event) = broadcast_event(&envelope) {
            self.bus.publish(self.instance.clone(), &envelope, event);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ActiveGuiRun {
    pub run_id: RunId,
    pub session_id: SessionId,
    pub started_at_ms: u64,
}

/// 活动 Run 注册表：GUI RunStart 登记，RunCancel 找令牌，完成后摘除。
#[derive(Default)]
pub struct GuiRunRegistry {
    runs: Mutex<HashMap<String, (ActiveGuiRun, CancellationToken)>>,
}

impl GuiRunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub(in crate::gui_host) fn register(&self, run: ActiveGuiRun, token: CancellationToken) {
        self.runs
            .lock()
            .expect("gui run registry poisoned")
            .insert(run.run_id.as_str().to_string(), (run, token));
    }

    pub(in crate::gui_host) fn remove(&self, run_id: &RunId) {
        self.runs
            .lock()
            .expect("gui run registry poisoned")
            .remove(run_id.as_str());
    }

    pub fn cancel(&self, run_id: &RunId) -> bool {
        let mut runs = self.runs.lock().expect("gui run registry poisoned");
        match runs.remove(run_id.as_str()) {
            Some((_, token)) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    pub fn active(&self) -> Vec<ActiveGuiRun> {
        let runs = self.runs.lock().expect("gui run registry poisoned");
        let mut list: Vec<_> = runs.values().map(|(run, _)| run.clone()).collect();
        list.sort_by(|a, b| a.started_at_ms.cmp(&b.started_at_ms));
        list
    }

    pub fn contains(&self, run_id: &RunId) -> bool {
        self.runs
            .lock()
            .expect("gui run registry poisoned")
            .contains_key(run_id.as_str())
    }
}
