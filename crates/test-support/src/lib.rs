//! 可编程 Mock Provider、Mock Tool 与断言辅助。

pub mod contract;
pub mod plugin_contract;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent_domain::{
    CancellationToken, ModelId, PluginId, ProviderId, RequestId, RunId, StopReason, TokenUsage,
    ToolCallId, WorkspaceId,
};
use async_trait::async_trait;
use plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginError, PluginLifecycleEvent,
    PluginLifecycleEventKind, PluginManifest, PluginPermissions,
};
use provider_api::{
    CanonicalModelRequest, ModelDefinition, ModelProvider, ModelResponseSummary, ProviderError,
    ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, ResolvedCredential,
};
use semver::{Version, VersionReq};
use serde_json::Value;
use tool_api::{
    AgentTool, ToolCapability, ToolDescriptor, ToolError, ToolEventSink, ToolExecutionContext,
    ToolRequest, ToolResult, ToolStreamEvent,
};

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

    /// 发射一条仅含 Protected Blob 安全引用的 reasoning continuation。
    pub fn reasoning_item(mut self, item: agent_domain::ReasoningItem) -> Self {
        self.steps
            .push(MockProviderStep::Event(ProviderStreamEvent::ReasoningItem(
                item,
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

    /// 发射一条归一后的 server tool 事件（P15-5）。
    pub fn server_tool(mut self, event: agent_domain::ServerToolEvent) -> Self {
        self.steps
            .push(MockProviderStep::Event(ProviderStreamEvent::ServerTool(
                event,
            )));
        self
    }

    /// 发射一条 provider transcript 续传信封（provider-neutral）。
    pub fn transcript_envelope(
        mut self,
        envelope: agent_domain::ProviderTranscriptEnvelope,
    ) -> Self {
        self.steps.push(MockProviderStep::Event(
            ProviderStreamEvent::TranscriptEnvelope(envelope),
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

    pub fn events(&self) -> impl Iterator<Item = &ProviderStreamEvent> {
        self.steps.iter().filter_map(|step| match step {
            MockProviderStep::Event(event) => Some(event),
            MockProviderStep::Fail(_) | MockProviderStep::WaitForCancellation => None,
        })
    }
}

#[derive(Clone, Debug)]
enum MockProviderStep {
    Event(ProviderStreamEvent),
    Fail(ProviderError),
    WaitForCancellation,
}

#[derive(Clone, Debug)]
pub struct MockProvider {
    id: ProviderId,
    script: MockScript,
    models: Vec<ModelDefinition>,
    calls: Arc<Mutex<Vec<MockProviderCallRecord>>>,
}

impl MockProvider {
    pub fn new(script: MockScript) -> Self {
        Self {
            id: ProviderId::from("mock"),
            script,
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
        let mut summary = ModelResponseSummary {
            stop_reason: StopReason::Error,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        };

        for step in &self.script.steps {
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

/// Mock Plugin 脚本步骤：按调用顺序逐步消费，脚本耗尽后默认成功。
#[derive(Clone, Debug)]
pub enum MockPluginStep {
    /// 返回成功。
    Ok,
    /// 返回指定插件错误。
    Error(PluginError),
    /// 触发 panic（用于验证 panic 不会终止 Core）。
    Panic(String),
    /// 等待取消令牌（插件自身观察取消）。
    WaitForCancellation,
}

/// 可编程 Mock Plugin：按 manifest 订阅事件，按脚本逐步响应。
#[derive(Clone, Debug)]
pub struct MockPlugin {
    manifest: PluginManifest,
    panic_on_manifest: bool,
    script: Arc<Mutex<VecDeque<MockPluginStep>>>,
    calls: Arc<Mutex<Vec<MockPluginCallRecord>>>,
}

impl MockPlugin {
    pub fn new(manifest: PluginManifest) -> Self {
        Self {
            manifest,
            panic_on_manifest: false,
            script: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_manifest(mut self, manifest: PluginManifest) -> Self {
        self.manifest = manifest;
        self
    }

    /// 让 trait manifest accessor panic，用于验证注册边界的 panic 隔离。
    pub fn with_manifest_panic(mut self) -> Self {
        self.panic_on_manifest = true;
        self
    }

    pub fn with_step(self, step: MockPluginStep) -> Self {
        self.script
            .lock()
            .expect("mock plugin script mutex")
            .push_back(step);
        self
    }

    pub fn with_script(self, steps: impl IntoIterator<Item = MockPluginStep>) -> Self {
        self.script
            .lock()
            .expect("mock plugin script mutex")
            .extend(steps);
        self
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn calls(&self) -> Vec<MockPluginCallRecord> {
        self.calls.lock().expect("mock plugin calls mutex").clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls().len()
    }

    fn next_step(&self) -> MockPluginStep {
        self.script
            .lock()
            .expect("mock plugin script mutex")
            .pop_front()
            .unwrap_or(MockPluginStep::Ok)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockPluginCallRecord {
    pub plugin_id: PluginId,
    pub event: PluginLifecycleEventKind,
    pub context: PluginContext,
    pub cancelled: bool,
}

#[async_trait]
impl Plugin for MockPlugin {
    fn manifest(&self) -> &PluginManifest {
        assert!(
            !self.panic_on_manifest,
            "mock plugin manifest accessor panic"
        );
        &self.manifest
    }

    async fn on_lifecycle_event(
        &self,
        event: PluginLifecycleEvent,
        context: PluginContext,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        let cancelled = cancel.is_cancelled();
        self.calls
            .lock()
            .expect("mock plugin calls mutex")
            .push(MockPluginCallRecord {
                plugin_id: self.manifest.id.clone(),
                event: event.kind(),
                context,
                cancelled,
            });
        if cancelled {
            return Err(PluginError::cancelled("mock plugin cancelled"));
        }

        match self.next_step() {
            MockPluginStep::Ok => Ok(()),
            MockPluginStep::Error(error) => Err(error),
            MockPluginStep::Panic(message) => panic!("{message}"),
            MockPluginStep::WaitForCancellation => {
                cancel.cancelled().await;
                Err(PluginError::cancelled("mock plugin cancelled"))
            }
        }
    }
}

/// 构造带 `LifecycleHook` capability 的 lifecycle hook 测试插件。
pub fn hook_plugin(
    id: impl Into<PluginId>,
    hooks: impl IntoIterator<Item = PluginLifecycleEventKind>,
) -> MockPlugin {
    hook_plugin_with_api(id, "^1", hooks)
}

/// 构造指定宿主 API 兼容范围的 lifecycle hook 测试插件（P10-6）。
pub fn hook_plugin_with_api(
    id: impl Into<PluginId>,
    api_requirement: &str,
    hooks: impl IntoIterator<Item = PluginLifecycleEventKind>,
) -> MockPlugin {
    let id = id.into();
    let manifest = PluginManifest {
        id: id.clone(),
        name: id.as_str().to_owned(),
        version: Version::new(1, 0, 0),
        api_version: VersionReq::parse(api_requirement)
            .expect("api_requirement must be a valid semver requirement"),
        description: None,
        permissions: PluginPermissions::default(),
        capabilities: vec![PluginCapability::LifecycleHook],
        tool_capabilities: Vec::new(),
        tools: Vec::new(),
        commands: Vec::new(),
        lifecycle_hooks: hooks.into_iter().collect(),
    };
    MockPlugin::new(manifest)
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
                description: format!("Mock tool {name}"),
                name,
                input_schema: serde_json::json!({"type": "object"}),
                capability: ToolCapability::ReadOnly,
                kind: tool_api::ToolKind::ClientFunction,
                hosting: tool_api::ToolHosting::Local,
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

    use agent_domain::{
        ContentPart, Message, MessageId, MessageMetadata, MessageRole, TextContent,
    };
    use provider_api::{PromptCachePreference, RequestBudget, ResponseFormat, ToolChoice};

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

    #[tokio::test]
    async fn minimal_text_tool_result_complete_chain_runs_without_network() {
        let script = MockScript::new()
            .response_started("response-1")
            .text("Starting")
            .tool_call("read_file", serde_json::json!({"path": "README.md"}))
            .text("Done")
            .complete();
        let provider = MockProvider::new(script);
        let provider_sink = RecordingProviderSink::default();

        let summary = provider
            .stream(
                request("request-1"),
                &provider_sink,
                CancellationToken::new(),
            )
            .await
            .expect("mock stream");
        let tool = MockTool::new(
            "read_file",
            ToolResult::success(vec![ContentPart::Text(TextContent {
                text: "file body".into(),
            })]),
        );
        let tool_result = tool
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
            .expect("mock tool");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        assert!(!tool_result.is_error());
        assert!(matches!(
            provider_sink.events().as_slice(),
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
        tool.assert_called_with(&[serde_json::json!({"path": "README.md"})]);
        assert_provider_request_order(&provider, &["request-1"]);
    }

    #[tokio::test]
    async fn multiple_tool_calls_keep_partial_json_chunks_in_order() {
        let script = MockScript::new()
            .tool_call_chunks(
                ToolCallId::from("call-a"),
                "first",
                [r#"{"path":"#, r#""a"}"#],
            )
            .tool_call_chunks(ToolCallId::from("call-b"), "second", [r#"{"line":1}"#])
            .complete_with(StopReason::ToolUse);
        let provider = MockProvider::new(script);
        let sink = RecordingProviderSink::default();
        provider
            .stream(request("request-1"), &sink, CancellationToken::new())
            .await
            .expect("mock stream");

        let events = sink.events();
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
    }

    #[tokio::test]
    async fn mock_script_emits_server_tool_events_and_transcript_envelope() {
        use agent_domain::{
            ArtifactId, Citation, CitationSourceKind, ProgramStream, ProviderTranscriptEnvelope,
            ServerToolEvent, TranscriptItem,
        };

        let script = MockScript::new()
            .server_tool(ServerToolEvent::Started {
                tool_call_id: ToolCallId::from("server-tool-1"),
                name: "web_search".into(),
                arguments: Some(serde_json::json!({"query": "pawork"})),
            })
            .server_tool(ServerToolEvent::CitationAdded {
                tool_call_id: ToolCallId::from("server-tool-1"),
                citation: Citation {
                    url: Some("https://example.com".into()),
                    source_kind: CitationSourceKind::WebSearch,
                    ..Citation::empty()
                },
            })
            .server_tool(ServerToolEvent::ProgramOutput {
                tool_call_id: ToolCallId::from("server-tool-1"),
                stream: ProgramStream::Stdout,
                delta: None,
                artifact: Some(ArtifactId::from("artifact-log-1")),
            })
            .server_tool(ServerToolEvent::Completed {
                tool_call_id: ToolCallId::from("server-tool-1"),
                summary: Some("3 results".into()),
                artifacts: Vec::new(),
            })
            .transcript_envelope(ProviderTranscriptEnvelope {
                items: vec![TranscriptItem::Text("final".into())],
                cursor: Some("cursor-1".into()),
                continuation_reference: None,
            })
            .complete();
        let provider = MockProvider::new(script);
        let sink = RecordingProviderSink::default();

        let summary = provider
            .stream(
                request("request-server-tool"),
                &sink,
                CancellationToken::new(),
            )
            .await
            .expect("mock stream");

        assert_eq!(summary.stop_reason, StopReason::Completed);
        let events = sink.events();
        assert_eq!(events.len(), 6);
        assert!(matches!(
            &events[0],
            ProviderStreamEvent::ServerTool(ServerToolEvent::Started { name, .. })
                if name == "web_search"
        ));
        assert!(matches!(
            &events[3],
            ProviderStreamEvent::ServerTool(ServerToolEvent::Completed { .. })
        ));
        assert!(matches!(
            &events[4],
            ProviderStreamEvent::TranscriptEnvelope(_)
        ));
        assert!(matches!(
            &events[5],
            ProviderStreamEvent::ResponseCompleted(StopReason::Completed)
        ));
    }

    #[tokio::test]
    async fn cancellation_reaches_provider_and_tool_and_is_recorded() {
        let provider = MockProvider::new(MockScript::new().text("never emitted").complete());
        let provider_cancel = CancellationToken::new();
        provider_cancel.cancel();
        let provider_error = provider
            .stream(
                request("cancel-provider"),
                &RecordingProviderSink::default(),
                provider_cancel,
            )
            .await
            .expect_err("provider cancellation");

        let tool = MockTool::new("read_file", ToolResult::success(Vec::new()));
        let tool_cancel = CancellationToken::new();
        tool_cancel.cancel();
        let tool_error = tool
            .execute(
                ToolRequest {
                    tool_call_id: ToolCallId::from("call-1"),
                    input: Value::Null,
                },
                context(),
                &RecordingToolSink::default(),
                tool_cancel,
            )
            .await
            .expect_err("tool cancellation");

        assert_eq!(provider_error.kind, ProviderErrorKind::Cancelled);
        assert!(provider.calls()[0].cancelled);
        assert!(tool.calls()[0].cancelled);
        assert_eq!(tool_error.kind, tool_api::ToolErrorKind::Cancelled);
    }

    #[tokio::test]
    async fn mock_plugin_consumes_script_records_calls_and_cancellation() {
        use agent_domain::CoreInstanceId;
        use plugin_api::{PluginErrorKind, PluginLifecycleEvent};

        let context = PluginContext {
            instance_id: CoreInstanceId::from("core"),
            workspace_id: None,
            session_id: None,
            run_id: None,
        };
        let plugin = hook_plugin("p", [PluginLifecycleEventKind::RunStart]).with_script([
            MockPluginStep::Error(PluginError::new(PluginErrorKind::Timeout, "scripted error")),
            MockPluginStep::Panic("scripted panic".into()),
        ]);

        let first = plugin
            .on_lifecycle_event(
                PluginLifecycleEvent::RunStart {
                    run_id: RunId::from("r-1"),
                },
                context.clone(),
                CancellationToken::new(),
            )
            .await
            .expect_err("scripted error");
        assert_eq!(first.kind, PluginErrorKind::Timeout);
        assert_eq!(plugin.call_count(), 1);
        assert_eq!(plugin.calls()[0].event, PluginLifecycleEventKind::RunStart);

        let panic_task = tokio::spawn({
            let plugin = plugin.clone();
            let context = context.clone();
            async move {
                plugin
                    .on_lifecycle_event(
                        PluginLifecycleEvent::RunStart {
                            run_id: RunId::from("r-2"),
                        },
                        context,
                        CancellationToken::new(),
                    )
                    .await
            }
        });
        let join_error = panic_task.await.expect_err("scripted panic");
        assert!(join_error.is_panic());

        let cancel = CancellationToken::new();
        cancel.cancel();
        let cancelled = plugin
            .on_lifecycle_event(
                PluginLifecycleEvent::RunStart {
                    run_id: RunId::from("r-3"),
                },
                context,
                cancel,
            )
            .await
            .expect_err("cancelled");
        assert_eq!(cancelled.kind, PluginErrorKind::Cancelled);
        assert_eq!(plugin.call_count(), 3);
        assert!(plugin.calls()[2].cancelled);
    }
}
