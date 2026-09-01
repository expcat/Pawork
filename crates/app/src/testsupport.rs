//! 测试装配共享件：仅测试编译，供根模块与各服务模块的测试复用。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{AgentEvent, AgentEventEnvelope};
use pawork_domain::{
    CancellationToken, CanonicalModelRequest, ContentPart, Message, MessageId, MessageRole,
    ModelDefinition, ModelId, ModelProvider, ModelResponseSummary, ProviderError, ProviderId,
    ProviderStreamEvent, ResolvedCredential, StopReason, TextContent, TokenUsage,
};
use pawork_engine::{AgentEventSink, EngineError};
use pawork_providers::ModelRegistry;
use pawork_storage::session::SessionStore;
use pawork_workspace::config::{PaworkConfig, ProviderConfig};

use crate::protocol::AdapterProtocol;
use crate::AppCore;

#[derive(Default)]
pub(crate) struct RecordingEvents(pub(crate) Mutex<Vec<AgentEventEnvelope>>);

impl RecordingEvents {
    pub(crate) fn types(&self) -> Vec<&'static str> {
        self.0
            .lock()
            .expect("mutex")
            .iter()
            .map(|envelope| match &envelope.payload {
                AgentEvent::MessageCommitted { message } if message.role == MessageRole::User => {
                    "user"
                }
                AgentEvent::MessageCommitted { .. } => "assistant",
                AgentEvent::RunStarted { .. } => "RunStarted",
                AgentEvent::RunCompleted { .. } => "RunCompleted",
                AgentEvent::AssistantTextDelta { .. } => "delta",
                AgentEvent::ToolCallStarted { .. } => "ToolCallStarted",
                AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
                AgentEvent::ToolApprovalResponded { .. } => "ToolApprovalResponded",
                AgentEvent::ToolExecutionStarted { .. } => "ToolExecutionStarted",
                AgentEvent::ToolExecutionCompleted { .. } => "ToolExecutionCompleted",
                AgentEvent::ToolOutputDelta { .. } => "ToolOutputDelta",
                AgentEvent::CompactionStarted { .. } => "CompactionStarted",
                AgentEvent::CompactionCompleted { .. } => "CompactionCompleted",
                AgentEvent::CheckpointCreated { .. } => "CheckpointCreated",
                AgentEvent::CheckpointRolledBack { .. } => "CheckpointRolledBack",
                _ => "other",
            })
            .collect()
    }
}

#[async_trait]
impl AgentEventSink for RecordingEvents {
    async fn emit(&self, envelope: AgentEventEnvelope) -> Result<(), EngineError> {
        self.0.lock().expect("mutex").push(envelope);
        Ok(())
    }
}

pub(crate) struct ScriptedProvider {
    pub(crate) events: Vec<ProviderStreamEvent>,
    pub(crate) summary: ModelResponseSummary,
    pub(crate) models: Vec<ModelDefinition>,
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    fn id(&self) -> ProviderId {
        ProviderId::from("mock")
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(self.models.clone())
    }

    async fn stream(
        &self,
        _request: CanonicalModelRequest,
        sink: &dyn pawork_domain::ProviderEventSink,
        _cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        for event in &self.events {
            sink.emit(event.clone()).await?;
        }
        Ok(self.summary.clone())
    }
}

pub(crate) fn sample_config(id: &str) -> PaworkConfig {
    PaworkConfig {
        default_provider: Some(id.into()),
        default_model: Some("glm-5.2".into()),
        providers: vec![ProviderConfig {
            id: id.into(),
            base_url: Some("https://example.test/v1".into()),
            ..ProviderConfig::default()
        }],
        ..PaworkConfig::default()
    }
}

pub(crate) fn set_env(key: &str, value: &str) {
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var(key, value);
    }
}

pub(crate) fn remove_env(key: &str) {
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var(key);
    }
}

pub(crate) fn user_hello() -> Message {
    Message {
        id: MessageId::from("message-1"),
        role: MessageRole::User,
        content: vec![ContentPart::Text(TextContent {
            text: "hello".into(),
        })],
        metadata: Default::default(),
    }
}

pub(crate) async fn mock_core(events: Vec<ProviderStreamEvent>) -> (AppCore, tempfile::TempDir) {
    mock_core_with_usage(events, TokenUsage::default()).await
}

pub(crate) async fn mock_core_with_usage(
    events: Vec<ProviderStreamEvent>,
    usage: TokenUsage,
) -> (AppCore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.db");
    let (store, _) = SessionStore::open(&path).await.expect("store");
    let summary = ModelResponseSummary {
        stop_reason: StopReason::Completed,
        usage,
        response_id: Some("resp-1".into()),
        provider_metadata: Default::default(),
    };
    let core = AppCore::from_parts(
        Arc::new(ScriptedProvider {
            events,
            summary,
            models: vec![ModelDefinition {
                id: ModelId::from("glm-5.2"),
                display_name: "glm-5.2".into(),
                context_window_tokens: 0,
                max_output_tokens: 0,
                capabilities: pawork_domain::ModelCapabilities::default(),
            }],
        }),
        None,
        ModelId::from("glm-5.2"),
        ProviderId::from("mock"),
        Some(store),
    );
    (core, dir)
}

pub(crate) fn core_with_registry(registry: ModelRegistry, model: &str) -> AppCore {
    AppCore::from_parts_with_protocol(
        Arc::new(ScriptedProvider {
            events: Vec::new(),
            summary: ModelResponseSummary {
                stop_reason: StopReason::Completed,
                usage: TokenUsage::default(),
                response_id: Some("resp-1".into()),
                provider_metadata: Default::default(),
            },
            models: Vec::new(),
        }),
        None,
        ModelId::from(model),
        ProviderId::from("mock"),
        AdapterProtocol::ChatCompletions,
        None,
        registry,
    )
}

/// Captures structured tracing fields for degrade emission tests.
#[derive(Clone, Debug, Default)]
pub(crate) struct CapturedTrace {
    pub level: String,
    pub message: String,
    pub fields: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Default)]
pub(crate) struct RecordingSubscriber {
    events: Arc<Mutex<Vec<CapturedTrace>>>,
}

impl RecordingSubscriber {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn events(&self) -> Vec<CapturedTrace> {
        self.events.lock().expect("events").clone()
    }
}

impl tracing::Subscriber for RecordingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let mut captured = CapturedTrace {
            level: event.metadata().level().to_string(),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
        };
        if captured.message.is_empty() {
            if let Some(message) = captured.fields.get("message") {
                captured.message = message.clone();
            }
        }
        self.events.lock().expect("events").push(captured);
    }

    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    fields: std::collections::BTreeMap<String, String>,
}

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            self.message = Some(rendered.trim_matches('"').to_string());
        } else {
            self.fields.insert(
                field.name().to_string(),
                rendered.trim_matches('"').to_string(),
            );
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

/// 带「interest 缓存钉住」的 degrade 事件捕获器。
///
/// 背景（tracing-core 0.1.36）：每个 callsite 在全局注册表里只缓存一份 Interest。
/// 当注册表 has_just_one 为 true 时，一个 callsite 若在**没有 scoped default** 的线程上
/// 首次注册，interest 会按「JustOne → get_default → NONE」路径算出并缓存
/// Interest::never()——此后任何线程在该 callsite 上的 emit 都被宏门
/// !interest.is_never() 静默跳过，直到某次 Dispatch::new 的 Write 重建将其治愈。
/// 与无 subscriber 测试共享 callsite 的断言测试因此会间歇丢事件（R8 审计的
/// command_record_failure_is_counted_not_swallowed flake 即此机制）。
///
/// install() 对同一 subscriber 做两次 Dispatch::new（两个独立 Arc → 两个 registrar）：
/// 第二次注册把注册表推到 ≥2，将 has_just_one 置为 false 并以 Write 重建治愈既有
/// never 缓存；pin 存活期间，新 callsite 注册一律走 Read(vec) 路径对存活分发器计算
/// interest，投毒窗口被完全封闭。捕获窗口结束后调用 dismiss() 释放。
pub(crate) struct RecordingCapture {
    subscriber: RecordingSubscriber,
    dispatch: Option<tracing::Dispatch>,
    pin: Option<tracing::Dispatch>,
    guard: Option<tracing::dispatcher::DefaultGuard>,
}

impl RecordingCapture {
    pub(crate) fn install() -> Self {
        let subscriber = RecordingSubscriber::new();
        let dispatch = tracing::Dispatch::new(subscriber.clone());
        // 第二次注册（独立 Arc）是 has_just_one=false 且触发重建治愈 never 缓存的最小条件；
        // 它同时作为 pin：存活期间注册表始终 ≥2 个 registrar。
        let pin = tracing::Dispatch::new(subscriber.clone());
        let guard = tracing::dispatcher::set_default(&dispatch);
        Self {
            subscriber,
            dispatch: Some(dispatch),
            pin: Some(pin),
            guard: Some(guard),
        }
    }

    pub(crate) fn events(&self) -> Vec<CapturedTrace> {
        self.subscriber.events()
    }

    /// 结束捕获：先解除 scoped default，再释放两个已注册 Dispatch。
    pub(crate) fn dismiss(mut self) {
        self.guard.take();
        self.dispatch.take();
        self.pin.take();
    }
}

#[cfg(test)]
mod capture_regression_tests {
    use super::*;

    /// 探针 callsite A：跨线程共享的静态 callsite，首次命中的线程完成注册。
    fn probe_a(tag: &str) {
        tracing::warn!(code = "r8.capture_probe_a", tag = %tag, "probe a");
    }

    /// 探针 callsite B：用于在捕获窗口内再次尝试投毒。
    fn probe_b(tag: &str) {
        tracing::warn!(code = "r8.capture_probe_b", tag = %tag, "probe b");
    }

    #[test]
    fn recording_capture_heals_and_shields_interest_cache_poisoning() {
        // 1) 孤立线程上创建 Dispatch 后立即释放：注册表回到「最后一次观察 ≤1」稳态。
        std::thread::spawn(|| {
            let _dead = tracing::Dispatch::new(RecordingSubscriber::new());
        })
        .join()
        .expect("seed thread");

        // 2) 无 scoped default 的线程首次命中 probe_a：
        //    「JustOne → get_default → NONE」路径缓存 Interest::never()（投毒）。
        std::thread::spawn(|| probe_a("poison"))
            .join()
            .expect("poison thread");

        // 3) install()：第二次注册治愈 never 缓存，并令 has_just_one 保持 false。
        let capture = RecordingCapture::install();

        // 4) pin 存活期间，另一无 default 线程首次命中 probe_b：不应再被投毒。
        std::thread::spawn(|| probe_b("late"))
            .join()
            .expect("late thread");

        // 5) 窗口内的 emit 必须全部被捕获。
        probe_a("inside-window");
        probe_b("inside-window");
        let events = capture.events();
        let has_tag = |code: &str, tag: &str| {
            events.iter().any(|event| {
                event.fields.get("code").map(String::as_str) == Some(code)
                    && event.fields.get("tag").map(String::as_str) == Some(tag)
            })
        };
        assert!(
            has_tag("r8.capture_probe_a", "inside-window"),
            "probe_a emit inside capture window must be captured: {events:?}"
        );
        assert!(
            has_tag("r8.capture_probe_b", "inside-window"),
            "probe_b must not stay poisoned by late registration: {events:?}"
        );
        capture.dismiss();
    }
}
