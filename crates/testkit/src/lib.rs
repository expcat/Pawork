//! 可编程 Mock Provider、Mock Tool 与断言辅助。

pub mod contract;

pub use contract::{
    assert_parallel_tool_calls, assert_single_tool_call, assert_text_stream, count_variant,
};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pawork_domain::{
    AgentTool, CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary,
    ProviderError, ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
    ToolError, ToolEventSink, ToolExecutionContext, ToolRequest, ToolResult, ToolStreamEvent,
};
use pawork_domain::{
    CancellationToken, ModelId, ProviderId, RequestId, RunId, StopReason, TokenUsage, ToolCallId,
    ToolCapability, ToolDescriptor, ToolHosting, ToolKind, WorkspaceId,
};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct MockScript {
    steps: Vec<MockProviderStep>,
    next_tool_call: u64,
}

impl MockScript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn response_started(mut self, response_id: impl Into<String>) -> Self {
        self.steps.push(MockProviderStep::Event(
            ProviderStreamEvent::ResponseStarted {
                response_id: Some(response_id.into()),
            },
        ));
        self
    }

    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.steps
            .push(MockProviderStep::Event(ProviderStreamEvent::TextDelta(
                text.into(),
            )));
        self
    }

    pub fn thinking(mut self, text: impl Into<String>) -> Self {
        self.steps
            .push(MockProviderStep::Event(ProviderStreamEvent::ThinkingDelta(
                text.into(),
            )));
        self
    }

    pub fn tool_call(mut self, name: impl Into<String>, arguments: Value) -> Self {
        let id = ToolCallId::from(format!("mock-tool-call-{}", self.next_tool_call));
        self.next_tool_call += 1;
        let json = serde_json::to_string(&arguments).expect("serde_json::Value always serializes");
        self = self.tool_call_chunks(id, name, [json]);
        self
    }

    pub fn tool_call_chunks<I, S>(
        mut self,
        id: ToolCallId,
        name: impl Into<String>,
        chunks: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.steps.push(MockProviderStep::Event(
            ProviderStreamEvent::ToolCallStarted {
                id: id.clone(),
                name: name.into(),
            },
        ));
        self.steps.extend(chunks.into_iter().map(|json| {
            MockProviderStep::Event(ProviderStreamEvent::ToolCallArgumentsDelta {
                id: id.clone(),
                json: json.into(),
            })
        }));
        self.steps.push(MockProviderStep::Event(
            ProviderStreamEvent::ToolCallCompleted { id },
        ));
        self
    }

    pub fn usage(mut self, usage: TokenUsage) -> Self {
        self.steps
            .push(MockProviderStep::Event(ProviderStreamEvent::UsageUpdated(
                usage,
            )));
        self
    }

    pub fn provider_metadata(mut self, metadata: Value) -> Self {
        self.steps.push(MockProviderStep::Event(
            ProviderStreamEvent::ProviderMetadata(metadata),
        ));
        self
    }

    pub fn complete_with(mut self, stop_reason: StopReason) -> Self {
        self.steps.push(MockProviderStep::Event(
            ProviderStreamEvent::ResponseCompleted(stop_reason),
        ));
        self
    }

    pub fn complete(self) -> Self {
        self.complete_with(StopReason::Completed)
    }

    pub fn fail(mut self, error: ProviderError) -> Self {
        self.steps.push(MockProviderStep::Fail(error));
        self
    }

    pub fn wait_for_cancellation(mut self) -> Self {
        self.steps.push(MockProviderStep::WaitForCancellation);
        self
    }
}

#[derive(Clone, Debug)]
enum MockProviderStep {
    Event(ProviderStreamEvent),
    Fail(ProviderError),
    WaitForCancellation,
}

#[derive(Clone, Debug)]
enum MockProviderSource {
    Replay(MockScript),
    Sequence {
        scripts: Vec<MockScript>,
        next: Arc<AtomicUsize>,
    },
}

#[derive(Clone, Debug)]
pub struct MockProvider {
    id: ProviderId,
    source: MockProviderSource,
    models: Vec<ModelDefinition>,
    calls: Arc<Mutex<Vec<MockProviderCallRecord>>>,
}

impl MockProvider {
    pub fn new(script: MockScript) -> Self {
        Self {
            id: ProviderId::from("mock"),
            source: MockProviderSource::Replay(script),
            models: Vec::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn sequence(scripts: Vec<MockScript>) -> Self {
        Self {
            id: ProviderId::from("mock"),
            source: MockProviderSource::Sequence {
                scripts,
                next: Arc::new(AtomicUsize::new(0)),
            },
            models: Vec::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_id(mut self, id: ProviderId) -> Self {
        self.id = id;
        self
    }

    pub fn with_models(mut self, models: Vec<ModelDefinition>) -> Self {
        self.models = models;
        self
    }

    pub fn calls(&self) -> Vec<MockProviderCallRecord> {
        self.calls
            .lock()
            .expect("mock provider calls mutex")
            .clone()
    }

    fn take_script(&self) -> Result<MockScript, ProviderError> {
        match &self.source {
            MockProviderSource::Replay(script) => Ok(script.clone()),
            MockProviderSource::Sequence { scripts, next } => {
                let index = next.fetch_add(1, Ordering::SeqCst);
                scripts.get(index).cloned().ok_or_else(|| {
                    ProviderError::new(
                        ProviderErrorKind::StreamInterrupted,
                        "mock script sequence exhausted",
                    )
                })
            }
        }
    }

    fn record(&self, request: &CanonicalModelRequest) -> usize {
        let mut calls = self.calls.lock().expect("mock provider calls mutex");
        calls.push(MockProviderCallRecord {
            request_id: request.request_id.clone(),
            model: request.model.clone(),
            event_count: 0,
            cancelled: false,
            completed: false,
        });
        calls.len() - 1
    }

    fn update_call(&self, index: usize, update: impl FnOnce(&mut MockProviderCallRecord)) {
        let mut calls = self.calls.lock().expect("mock provider calls mutex");
        update(&mut calls[index]);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockProviderCallRecord {
    pub request_id: RequestId,
    pub model: ModelId,
    pub event_count: usize,
    pub cancelled: bool,
    pub completed: bool,
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    async fn list_models(
        &self,
        _credential: Option<&ResolvedCredential>,
    ) -> Result<Vec<ModelDefinition>, ProviderError> {
        Ok(self.models.clone())
    }

    async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        let call_index = self.record(&request);
        let script = self.take_script()?;
        let mut summary = ModelResponseSummary {
            stop_reason: StopReason::Error,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        };

        for step in &script.steps {
            if cancel.is_cancelled() {
                self.update_call(call_index, |call| call.cancelled = true);
                return Err(ProviderError::cancelled("mock provider cancelled"));
            }

            match step {
                MockProviderStep::Event(event) => {
                    match event {
                        ProviderStreamEvent::ResponseStarted { response_id } => {
                            summary.response_id.clone_from(response_id);
                        }
                        ProviderStreamEvent::UsageUpdated(usage) => summary.usage = usage.clone(),
                        ProviderStreamEvent::ResponseCompleted(stop_reason) => {
                            summary.stop_reason = stop_reason.clone();
                            self.update_call(call_index, |call| call.completed = true);
                        }
                        ProviderStreamEvent::ProviderMetadata(metadata) => {
                            summary.provider_metadata = metadata.clone();
                        }
                        ProviderStreamEvent::TextDelta(_)
                        | ProviderStreamEvent::ThinkingDelta(_)
                        | ProviderStreamEvent::ReasoningItem(_)
                        | ProviderStreamEvent::ToolCallStarted { .. }
                        | ProviderStreamEvent::ToolCallArgumentsDelta { .. }
                        | ProviderStreamEvent::ToolCallCompleted { .. }
                        | ProviderStreamEvent::ServerTool(_)
                        | ProviderStreamEvent::TranscriptEnvelope(_)
                        | ProviderStreamEvent::Error(_) => {}
                    }
                    sink.emit(event.clone()).await?;
                    self.update_call(call_index, |call| call.event_count += 1);
                }
                MockProviderStep::Fail(error) => return Err(error.clone()),
                MockProviderStep::WaitForCancellation => {
                    cancel.cancelled().await;
                    self.update_call(call_index, |call| call.cancelled = true);
                    return Err(ProviderError::cancelled("mock provider cancelled"));
                }
            }
        }

        if !self.calls()[call_index].completed {
            return Err(ProviderError::new(
                ProviderErrorKind::StreamInterrupted,
                "mock script ended without ResponseCompleted",
            ));
        }
        Ok(summary)
    }
}

#[derive(Clone, Debug)]
pub struct MockTool {
    descriptor: ToolDescriptor,
    outcome: Result<ToolResult, ToolError>,
    calls: Arc<Mutex<Vec<MockToolCallRecord>>>,
}

impl MockTool {
    pub fn new(name: impl Into<String>, result: ToolResult) -> Self {
        let name = name.into();
        Self {
            descriptor: ToolDescriptor {
                name: name.clone(),
                description: format!("Mock tool {name}"),
                input_schema: serde_json::json!({"type": "object"}),
                capability: ToolCapability::ReadOnly,
                kind: ToolKind::ClientFunction,
                hosting: ToolHosting::Local,
                capabilities: Vec::new(),
                requires_approval: false,
                read_only: true,
                supports_concurrency: true,
                default_timeout_ms: Some(1_000),
                max_output_bytes: 64 * 1024,
                allowed_in_untrusted_workspace: true,
            },
            outcome: Ok(result),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn failing(name: impl Into<String>, error: ToolError) -> Self {
        let mut tool = Self::new(name, ToolResult::success(Vec::new()));
        tool.outcome = Err(error);
        tool
    }

    pub fn with_descriptor(mut self, descriptor: ToolDescriptor) -> Self {
        self.descriptor = descriptor;
        self
    }

    pub fn calls(&self) -> Vec<MockToolCallRecord> {
        self.calls.lock().expect("mock tool calls mutex").clone()
    }

    pub fn assert_called_with(&self, expected: &[Value]) {
        let actual: Vec<_> = self.calls().into_iter().map(|call| call.input).collect();
        assert_eq!(actual, expected, "mock tool input sequence differs");
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MockToolCallRecord {
    pub tool_call_id: ToolCallId,
    pub input: Value,
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    pub cancelled: bool,
}

#[async_trait]
impl AgentTool for MockTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    async fn execute(
        &self,
        request: ToolRequest,
        context: ToolExecutionContext,
        _sink: &dyn ToolEventSink,
        cancel: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let cancelled = cancel.is_cancelled();
        self.calls
            .lock()
            .expect("mock tool calls mutex")
            .push(MockToolCallRecord {
                tool_call_id: request.tool_call_id,
                input: request.input,
                workspace_id: context.workspace_id,
                run_id: context.run_id,
                cancelled,
            });

        if cancelled {
            return Err(ToolError::cancelled("mock tool cancelled"));
        }
        self.outcome.clone()
    }
}

#[derive(Clone, Debug, Default)]
pub struct RecordingProviderSink(Arc<Mutex<Vec<ProviderStreamEvent>>>);

impl RecordingProviderSink {
    pub fn events(&self) -> Vec<ProviderStreamEvent> {
        self.0.lock().expect("provider sink mutex").clone()
    }
}

#[async_trait]
impl ProviderEventSink for RecordingProviderSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        self.0.lock().expect("provider sink mutex").push(event);
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct RecordingToolSink(Arc<Mutex<Vec<ToolStreamEvent>>>);

impl RecordingToolSink {
    pub fn events(&self) -> Vec<ToolStreamEvent> {
        self.0.lock().expect("tool sink mutex").clone()
    }
}

#[async_trait]
impl ToolEventSink for RecordingToolSink {
    async fn emit(&self, event: ToolStreamEvent) -> Result<(), ToolError> {
        self.0.lock().expect("tool sink mutex").push(event);
        Ok(())
    }
}

pub fn assert_provider_request_order(provider: &MockProvider, expected: &[&str]) {
    let actual: Vec<_> = provider
        .calls()
        .iter()
        .map(|call| call.request_id.as_str().to_owned())
        .collect();
    assert_eq!(actual, expected, "mock provider request sequence differs");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pawork_domain::{
        PromptCachePreference, RequestBudget, ResponseFormat, ToolChoice, ToolErrorKind,
    };
    use pawork_domain::{
        ContentPart, Message, MessageId, MessageMetadata, MessageRole, TextContent,
    };

    use super::*;

    fn request(id: &str) -> CanonicalModelRequest {
        CanonicalModelRequest {
            request_id: RequestId::from(id),
            model: ModelId::from("mock-model"),
            messages: vec![Message {
                id: MessageId::from("message-1"),
                role: MessageRole::User,
                content: vec![ContentPart::Text(TextContent { text: "go".into() })],
                metadata: MessageMetadata::default(),
            }],
            tools: Vec::new(),
            hosted_tools: Vec::new(),
            extensions: Vec::new(),
            tool_choice: ToolChoice::Auto,
            thinking: None,
            reasoning: None,
            temperature: None,
            max_output_tokens: None,
            stop_sequences: Vec::new(),
            response_format: ResponseFormat::Text,
            prompt_cache: PromptCachePreference::Automatic,
            budget: RequestBudget::default(),
            provider_options: BTreeMap::new(),
            trace_id: None,
        }
    }

    fn context() -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_id: WorkspaceId::from("workspace-1"),
            run_id: RunId::from("run-1"),
            working_directory: None,
        }
    }

    async fn stream(
        provider: &MockProvider,
        request_id: &str,
    ) -> (
        Result<ModelResponseSummary, ProviderError>,
        Vec<ProviderStreamEvent>,
    ) {
        let sink = RecordingProviderSink::default();
        let result = provider
            .stream(request(request_id), &sink, CancellationToken::new())
            .await;
        (result, sink.events())
    }

    #[tokio::test]
    async fn single_script_text_tool_call_and_complete() {
        let script = MockScript::new()
            .response_started("response-1")
            .text("Starting")
            .tool_call("read_file", serde_json::json!({"path": "README.md"}))
            .text("Done")
            .complete();
        let provider = MockProvider::new(script);
        let (summary, events) = stream(&provider, "request-1").await;
        let summary = summary.expect("mock stream");

        assert_eq!(provider.id().as_str(), "mock");
        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert!(matches!(
            events.as_slice(),
            [
                ProviderStreamEvent::ResponseStarted { .. },
                ProviderStreamEvent::TextDelta(_),
                ProviderStreamEvent::ToolCallStarted { .. },
                ProviderStreamEvent::ToolCallArgumentsDelta { .. },
                ProviderStreamEvent::ToolCallCompleted { .. },
                ProviderStreamEvent::TextDelta(_),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed)
            ]
        ));
        assert_single_tool_call(&events);
        assert_text_stream(&events);
        assert_provider_request_order(&provider, &["request-1"]);
        assert!(provider.calls()[0].completed);

        let (replay, _) = stream(&provider, "request-2").await;
        assert_eq!(
            replay.expect("replay same script").stop_reason,
            StopReason::Completed
        );
        assert_provider_request_order(&provider, &["request-1", "request-2"]);
    }

    #[tokio::test]
    async fn sequence_plays_two_scripts_then_exhausts() {
        let provider = MockProvider::sequence(vec![
            MockScript::new().text("first").complete(),
            MockScript::new()
                .text("second")
                .complete_with(StopReason::ToolUse),
        ]);

        let (first, first_events) = stream(&provider, "request-1").await;
        let (second, second_events) = stream(&provider, "request-2").await;
        let (third, third_events) = stream(&provider, "request-3").await;

        assert_eq!(first.expect("first script").stop_reason, StopReason::Completed);
        assert_eq!(
            second.expect("second script").stop_reason,
            StopReason::ToolUse
        );
        assert!(matches!(
            first_events.as_slice(),
            [
                ProviderStreamEvent::TextDelta(text),
                ProviderStreamEvent::ResponseCompleted(StopReason::Completed)
            ] if text == "first"
        ));
        assert!(matches!(
            second_events.as_slice(),
            [
                ProviderStreamEvent::TextDelta(text),
                ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse)
            ] if text == "second"
        ));

        let error = third.expect_err("sequence exhausted");
        assert_eq!(error.kind, ProviderErrorKind::StreamInterrupted);
        assert!(
            error.message.contains("mock script sequence exhausted"),
            "unexpected exhausted message: {}",
            error.message
        );
        assert!(third_events.is_empty());
        assert_provider_request_order(&provider, &["request-1", "request-2", "request-3"]);
        assert!(!provider.calls()[2].completed);
    }

    #[tokio::test]
    async fn tool_call_chunks_keep_partial_json_in_order() {
        let script = MockScript::new()
            .tool_call_chunks(
                ToolCallId::from("call-a"),
                "first",
                [r#"{"path":"#, r#""a"}"#],
            )
            .tool_call_chunks(ToolCallId::from("call-b"), "second", [r#"{"line":1}"#])
            .complete_with(StopReason::ToolUse);
        let provider = MockProvider::new(script);
        let (_, events) = stream(&provider, "request-1").await;

        let chunks: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ProviderStreamEvent::ToolCallArgumentsDelta { id, json } => {
                    Some((id.as_str(), json.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            chunks,
            vec![
                ("call-a", r#"{"path":"#),
                ("call-a", r#""a"}"#),
                ("call-b", r#"{"line":1}"#)
            ]
        );
        assert_parallel_tool_calls(&events);
        assert_eq!(
            count_variant(&events, |event| {
                matches!(event, ProviderStreamEvent::ToolCallStarted { .. })
            }),
            2
        );
    }

    #[tokio::test]
    async fn mock_tool_success_failure_and_cancellation() {
        let success = MockTool::new(
            "read_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "file body".into(),
            })]),
        );
        assert_eq!(success.descriptor().name, "read_file");
        assert_eq!(
            success.descriptor().input_schema,
            serde_json::json!({"type": "object"})
        );
        assert!(success.descriptor().read_only);
        assert!(success.descriptor().supports_concurrency);
        assert_eq!(success.descriptor().kind, ToolKind::ClientFunction);
        assert_eq!(success.descriptor().hosting, ToolHosting::Local);
        assert!(!success.descriptor().requires_approval);
        assert_eq!(success.descriptor().default_timeout_ms, Some(1_000));
        assert_eq!(success.descriptor().max_output_bytes, 64 * 1024);
        assert!(success.descriptor().allowed_in_untrusted_workspace);
        assert_eq!(success.descriptor().capability, ToolCapability::ReadOnly);

        let ok = success
            .execute(
                ToolRequest {
                    tool_call_id: ToolCallId::from("mock-tool-call-0"),
                    input: serde_json::json!({"path": "README.md"}),
                },
                context(),
                &RecordingToolSink::default(),
                CancellationToken::new(),
            )
            .await
            .expect("mock tool success");
        assert!(!ok.is_error());
        success.assert_called_with(&[serde_json::json!({"path": "README.md"})]);

        let fail_error = ToolError {
            kind: ToolErrorKind::ExecutionFailed,
            message: "boom".into(),
            retryable: false,
            retry_after_ms: None,
        };
        let failing = MockTool::failing("broken", fail_error.clone());
        let failed = failing
            .execute(
                ToolRequest {
                    tool_call_id: ToolCallId::from("call-fail"),
                    input: serde_json::json!({"x": 1}),
                },
                context(),
                &RecordingToolSink::default(),
                CancellationToken::new(),
            )
            .await
            .expect_err("mock tool failure");
        assert_eq!(failed, fail_error);
        failing.assert_called_with(&[serde_json::json!({"x": 1})]);

        let cancellable = MockTool::new("read_file", ToolResult::success(Vec::new()));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let cancelled = cancellable
            .execute(
                ToolRequest {
                    tool_call_id: ToolCallId::from("call-1"),
                    input: Value::Null,
                },
                context(),
                &RecordingToolSink::default(),
                cancel,
            )
            .await
            .expect_err("tool cancellation");
        assert_eq!(cancelled.kind, ToolErrorKind::Cancelled);
        assert!(cancellable.calls()[0].cancelled);
    }

    #[tokio::test]
    async fn provider_cancellation_is_recorded() {
        let provider = MockProvider::new(MockScript::new().text("never emitted").complete());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = provider
            .stream(
                request("cancel-provider"),
                &RecordingProviderSink::default(),
                cancel,
            )
            .await
            .expect_err("provider cancellation");

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert!(provider.calls()[0].cancelled);
        assert!(!provider.calls()[0].completed);
    }

    #[tokio::test]
    async fn fail_returns_scripted_error_immediately() {
        let error = ProviderError::new(ProviderErrorKind::Timeout, "scripted timeout");
        let provider = MockProvider::new(
            MockScript::new()
                .text("partial")
                .fail(error.clone())
                .complete(),
        );
        let (result, events) = stream(&provider, "request-fail").await;
        assert_eq!(result.expect_err("scripted fail"), error);
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::TextDelta(text)] if text == "partial"
        ));
        assert!(!provider.calls()[0].completed);
    }
}
