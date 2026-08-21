use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{AgentEventEnvelope, CancellationToken, EventId, RunId, SessionId};
use pawork_engine::{now_timestamp, AgentEventSink, EngineError};
use pawork_protocol::{
    AppEvent, AppEventEnvelope, DiagnosticLevel, EventSource, EventStream, GlobalSequence,
    API_VERSION,
};
use serde_json::Value;

use crate::{EventHub, HubError};

use super::events::broadcast_event;

/// 单实例事件总线：内部唯一 EventHub，组信封后 `publish`，容量默认 4096。
pub struct GuiEventBus {
    hub: EventHub,
    revision: AtomicU64,
    next_event: AtomicU64,
}

impl GuiEventBus {
    pub fn new(capacity: usize) -> Self {
        Self {
            hub: EventHub::with_capacity(capacity),
            revision: AtomicU64::new(1),
            next_event: AtomicU64::new(1),
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

    fn publish(
        &self,
        instance: pawork_domain::CoreInstanceId,
        envelope: &AgentEventEnvelope,
        event: AppEvent,
    ) {
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
        let app_envelope = AppEventEnvelope {
            api_version: API_VERSION,
            instance_id: instance,
            event_id: self.next_event_id(),
            global_sequence: GlobalSequence(0),
            stream: EventStream::Session(session_id.clone()),
            stream_sequence: 0,
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
