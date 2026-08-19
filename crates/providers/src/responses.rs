//! OpenAI Responses-compatible transport shared by ChatGPT and xAI adapters.
//!
//! This module owns only wire translation and streaming assembly. Provider choice stays in
//! adapter configuration/model capabilities; Core never branches on a provider name.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use pawork_domain::{
    CanonicalModelRequest, ModelDefinition, ModelResponseSummary, ProviderError,
    ProviderErrorKind, ProviderEventSink, ProviderStreamEvent, ReasoningEffort, ResponseFormat,
    ResolvedCredential, ThinkingLevel, ToolChoice, ToolDefinition,
};
use pawork_domain::{
    CancellationToken, ContentPart, ImageContent, ImageSource, Message, MessageRole, ProviderId,
    StopReason, TokenUsage, ToolCallId,
};
use crate::net::http::{HttpClient, HttpClientConfig};
use crate::net::sse::SseParser;
use crate::{clamp_reasoning_to_thinking, ReasoningProtector};
use serde_json::{json, Map, Value};

use crate::error_table::normalize_vendor_error;
use crate::is_credential_header;
use crate::memory_protector::InMemoryReasoningProtector;
use crate::responses_reasoning::{extract_encrypted_content, to_canonical, to_input};
use crate::usage::{map_stop_reason, normalize_usage};

/// Responses 请求的安全 wire 选项。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResponsesWireOptions {
    /// 显式设置远端存储策略；ChatGPT OAuth 通道默认 `Some(false)`。
    pub store: Option<bool>,
    /// 请求返回 encrypted reasoning continuation，明文随后立即进入 protector。
    pub include_encrypted_reasoning: bool,
}

/// 共享 Responses transport 配置。
#[derive(Clone, Debug)]
pub struct ResponsesTransportConfig {
    pub base_url: String,
    pub provider_id: ProviderId,
    pub http: HttpClientConfig,
    pub request_timeout: Option<Duration>,
    /// 非 secret 的 adapter 固定头；认证头禁止从这里注入。
    pub request_headers: Vec<(String, String)>,
    pub wire: ResponsesWireOptions,
}

impl ResponsesTransportConfig {
    pub fn new(base_url: impl Into<String>, provider_id: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            provider_id: ProviderId::new(provider_id.into()),
            http: HttpClientConfig::default(),
            request_timeout: None,
            request_headers: Vec::new(),
            wire: ResponsesWireOptions::default(),
        }
    }
}

/// 供具体 adapter 组合的 Responses 网络 transport。
pub struct ResponsesTransport {
    config: ResponsesTransportConfig,
    client: HttpClient,
    credential: ResolvedCredential,
    reasoning_protector: Arc<dyn ReasoningProtector>,
}

impl ResponsesTransport {
    pub fn new(
        config: ResponsesTransportConfig,
        credential: ResolvedCredential,
    ) -> Result<Self, ProviderError> {
        if config
            .request_headers
            .iter()
            .chain(config.http.extra_headers.iter())
            .any(|(name, _)| is_credential_header(name))
        {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "Responses HTTP headers cannot override authentication",
            ));
        }
        let mut http = config.http.clone();
        if let Some(timeout) = config.request_timeout {
            http.timeout = Some(timeout);
        }
        Ok(Self {
            config,
            client: HttpClient::new(http)?,
            credential,
            reasoning_protector: Arc::new(InMemoryReasoningProtector::default()),
        })
    }

    pub fn with_reasoning_protector(mut self, protector: Arc<dyn ReasoningProtector>) -> Self {
        self.reasoning_protector = protector;
        self
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.config.provider_id
    }

    pub fn responses_url(&self) -> String {
        format!("{}/responses", self.config.base_url.trim_end_matches('/'))
    }

    pub fn models_url(&self) -> String {
        format!("{}/models", self.config.base_url.trim_end_matches('/'))
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        let mut headers = self.config.request_headers.clone();
        headers.push((
            "Authorization".into(),
            format!("Bearer {}", self.credential.expose_secret()),
        ));
        headers
    }

    /// 读取标准 OpenAI-compatible `{ data: [{ id }] }` 模型目录。
    pub async fn list_standard_models(&self) -> Result<Vec<ModelDefinition>, ProviderError> {
        let value = self.get_json(&self.models_url()).await?;
        Ok(standard_model_definitions(&value))
    }

    /// 使用与 Responses 请求相同的 OAuth/API bearer 与固定头读取 JSON。
    pub async fn get_json(&self, url: &str) -> Result<Value, ProviderError> {
        self.client
            .get_json_with_headers(
                url,
                None,
                &self.request_headers(),
                CancellationToken::new(),
            )
            .await
            .map_err(|error| normalize_vendor_error(self.config.provider_id.as_str(), error))
    }

    pub async fn stream(
        &self,
        request: CanonicalModelRequest,
        sink: &dyn ProviderEventSink,
        cancel: CancellationToken,
    ) -> Result<ModelResponseSummary, ProviderError> {
        if !request.hosted_tools.is_empty() || !request.extensions.is_empty() {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                "this initial Responses adapter does not declare provider-hosted tools",
            ));
        }

        let reasoning_inputs =
            resolve_reasoning_inputs(&request, self.reasoning_protector.as_ref()).await;
        let body = to_responses_body(&request, reasoning_inputs, self.config.wire);
        let mut bytes = self
            .client
            .post_stream_with_headers(
                &self.responses_url(),
                body,
                request.trace_id.as_deref(),
                &self.request_headers(),
                cancel.clone(),
            )
            .await
            .map_err(|error| normalize_vendor_error(self.config.provider_id.as_str(), error))?;

        let mut sse = SseParser::new();
        let mut assembler = ResponsesStreamAssembler::new();
        let mut summary = ModelResponseSummary {
            stop_reason: StopReason::Error,
            usage: TokenUsage::default(),
            response_id: None,
            provider_metadata: Value::Null,
        };
        let mut saw_completion = false;

        while let Some(item) = bytes.next().await {
            if cancel.is_cancelled() {
                return Err(ProviderError::cancelled("Responses stream cancelled"));
            }
            for event in sse.feed(&item?) {
                let data = event?.data;
                if data.trim().is_empty() || data.trim() == "[DONE]" {
                    continue;
                }
                for assembled in assembler.feed(data.trim()) {
                    saw_completion |= self
                        .emit_assembled(assembled, sink, &mut summary)
                        .await?;
                }
            }
        }
        if let Some(event) = sse.finish()? {
            let data = event.data.trim();
            if !data.is_empty() && data != "[DONE]" {
                for assembled in assembler.feed(data) {
                    saw_completion |= self
                        .emit_assembled(assembled, sink, &mut summary)
                        .await?;
                }
            }
        }

        let final_state = assembler.finish();
        summary.response_id = final_state.response_id.or(summary.response_id);
        if final_state.usage != TokenUsage::default() {
            summary.usage = final_state.usage;
        }
        if let Some(stop_reason) = final_state.stop_reason {
            summary.stop_reason = stop_reason;
        }
        saw_completion |= final_state.completed;
        if !saw_completion {
            return Err(ProviderError::new(
                ProviderErrorKind::StreamInterrupted,
                "Responses stream ended without completion event",
            ));
        }
        Ok(summary)
    }

    async fn emit_assembled(
        &self,
        event: ResponsesAssemblyEvent,
        sink: &dyn ProviderEventSink,
        summary: &mut ModelResponseSummary,
    ) -> Result<bool, ProviderError> {
        match event {
            ResponsesAssemblyEvent::Canonical(event) => {
                if let ProviderStreamEvent::Error(error) = &event {
                    let error = normalize_vendor_error(
                        self.config.provider_id.as_str(),
                        error.clone(),
                    );
                    sink.emit(ProviderStreamEvent::Error(error.clone())).await?;
                    return Err(error);
                }
                let completed = matches!(event, ProviderStreamEvent::ResponseCompleted(_));
                match &event {
                    ProviderStreamEvent::UsageUpdated(usage) => summary.usage = usage.clone(),
                    ProviderStreamEvent::ResponseCompleted(stop) => {
                        summary.stop_reason = stop.clone()
                    }
                    ProviderStreamEvent::ResponseStarted { response_id } => {
                        summary.response_id = response_id.clone()
                    }
                    _ => {}
                }
                sink.emit(event).await?;
                Ok(completed)
            }
            ResponsesAssemblyEvent::ReasoningOutputItem { wire } => {
                let Some(encrypted) = extract_encrypted_content(&wire).map_err(|error| {
                    ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        format!("invalid Responses reasoning item: {error}"),
                    )
                })?
                else {
                    return Ok(false);
                };
                let blob_ref = self
                    .reasoning_protector
                    .protect(&encrypted.into_bytes())
                    .await
                    .map_err(|error| {
                        ProviderError::new(
                            ProviderErrorKind::Unknown,
                            format!("reasoning continuation protection failed: {error}"),
                        )
                    })?;
                let item = to_canonical(&wire, blob_ref).map_err(|error| {
                    ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        format!("invalid Responses reasoning item: {error}"),
                    )
                })?;
                sink.emit(ProviderStreamEvent::ReasoningItem(item)).await?;
                Ok(false)
            }
        }
    }
}

/// canonical request → Responses request body.
pub fn to_responses_body(
    request: &CanonicalModelRequest,
    reasoning_inputs: Vec<Value>,
    wire: ResponsesWireOptions,
) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), Value::String(request.model.to_string()));
    body.insert("stream".into(), Value::Bool(true));

    let mut input = reasoning_inputs;
    let mut instructions = Vec::new();
    for message in &request.messages {
        if message.role == MessageRole::System {
            if let Some(text) = message_text(message) {
                instructions.push(text);
                continue;
            }
        }
        input.extend(message_to_input(message));
    }
    if !instructions.is_empty() {
        body.insert("instructions".into(), Value::String(instructions.join("\n")));
    }
    body.insert("input".into(), Value::Array(input));

    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(request.tools.iter().map(function_tool).collect()),
        );
        body.insert("tool_choice".into(), tool_choice(&request.tool_choice));
        body.insert("parallel_tool_calls".into(), Value::Bool(true));
    }

    let clamped = clamp_reasoning_to_thinking(request.reasoning.as_ref(), request.thinking.as_ref());
    let effort = request
        .reasoning
        .as_ref()
        .map(|reasoning| reasoning.effort)
        .unwrap_or_else(|| match clamped.level {
            ThinkingLevel::Off => ReasoningEffort::None,
            ThinkingLevel::Low => ReasoningEffort::Low,
            ThinkingLevel::Medium => ReasoningEffort::Medium,
            ThinkingLevel::High => ReasoningEffort::High,
        });
    if let Some(effort) = reasoning_effort(effort) {
        body.insert("reasoning".into(), json!({"effort": effort}));
    }
    if wire.include_encrypted_reasoning {
        body.insert(
            "include".into(),
            json!(["reasoning.encrypted_content"]),
        );
    }
    if let Some(store) = wire.store {
        body.insert("store".into(), Value::Bool(store));
    }
    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        body.insert("max_output_tokens".into(), json!(max_output_tokens));
    }
    match &request.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::Json => {
            body.insert("text".into(), json!({"format": {"type": "json_object"}}));
        }
        ResponseFormat::JsonSchema { name, schema } => {
            body.insert(
                "text".into(),
                json!({"format": {"type": "json_schema", "name": name, "schema": schema}}),
            );
        }
    }
    if let Some(Value::String(previous)) = request.provider_options.get("previous_response_id") {
        body.insert("previous_response_id".into(), Value::String(previous.clone()));
    }
    for (key, value) in &request.provider_options {
        if is_reserved_option(key) {
            continue;
        }
        body.insert(key.clone(), value.clone());
    }
    Value::Object(body)
}

fn message_to_input(message: &Message) -> Vec<Value> {
    let mut items = Vec::new();
    let mut content = Vec::new();
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };
    let text_type = if message.role == MessageRole::Assistant {
        "output_text"
    } else {
        "input_text"
    };

    for part in &message.content {
        match part {
            ContentPart::Text(text) => {
                content.push(json!({"type": text_type, "text": text.text}));
            }
            ContentPart::Image(image) => {
                if let Some(image) = input_image(image) {
                    content.push(image);
                }
            }
            ContentPart::ToolCall(call) => {
                let arguments = if call.arguments.is_null() {
                    call.raw_arguments.clone().unwrap_or_default()
                } else {
                    call.arguments.to_string()
                };
                items.push(json!({
                    "type": "function_call",
                    "id": call.id.as_str(),
                    "call_id": call.id.as_str(),
                    "name": call.name,
                    "arguments": arguments,
                }));
            }
            ContentPart::ToolResult(result) => {
                let output = result
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": result.tool_call_id.as_str(),
                    "output": output,
                }));
            }
            ContentPart::Thinking(_) | ContentPart::Reasoning(_) | ContentPart::ArtifactRef(_) => {}
        }
    }
    if !content.is_empty() {
        items.insert(0, json!({"type": "message", "role": role, "content": content}));
    }
    items
}

fn message_text(message: &Message) -> Option<String> {
    let text = message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn input_image(image: &ImageContent) -> Option<Value> {
    let url = match &image.source {
        ImageSource::Url(url) => url.clone(),
        ImageSource::Base64(data) if !data.is_empty() => {
            format!("data:{};base64,{data}", image.media_type)
        }
        ImageSource::Base64(_) | ImageSource::Artifact(_) => return None,
    };
    (!url.is_empty()).then(|| json!({"type": "input_image", "image_url": url}))
}

fn function_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
        "strict": false,
    })
}

fn tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::None => json!("none"),
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Named(name) => json!({"type": "function", "name": name}),
    }
}

fn reasoning_effort(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::None => None,
        ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max => Some("high"),
    }
}

fn is_reserved_option(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "model"
            | "input"
            | "instructions"
            | "stream"
            | "store"
            | "tools"
            | "tool_choice"
            | "parallel_tool_calls"
            | "include"
            | "reasoning"
            | "previous_response_id"
            | "text"
            | "authorization"
            | "proxy-authorization"
            | "api_key"
            | "api-key"
            | "x-api-key"
    )
}

async fn resolve_reasoning_inputs(
    request: &CanonicalModelRequest,
    protector: &dyn ReasoningProtector,
) -> Vec<Value> {
    let mut inputs = Vec::new();
    for message in &request.messages {
        for part in &message.content {
            let ContentPart::Reasoning(item) = part else {
                continue;
            };
            let Ok(payload) = protector.resolve(&item.protected_blob_ref).await else {
                tracing::debug!(reasoning_item_id = %item.id.as_str(), "reasoning continuation unavailable");
                continue;
            };
            let Ok(content) = String::from_utf8(payload) else {
                tracing::debug!(reasoning_item_id = %item.id.as_str(), "reasoning continuation is not utf-8");
                continue;
            };
            match to_input(item, &content) {
                Ok(input) => inputs.push(input),
                Err(error) => tracing::debug!(reasoning_item_id = %item.id.as_str(), %error, "reasoning continuation cannot be rebuilt"),
            }
        }
    }
    inputs
}

fn standard_model_definitions(value: &Value) -> Vec<ModelDefinition> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("id").and_then(Value::as_str))
        .map(|id| ModelDefinition {
            id: pawork_domain::ModelId::new(id),
            display_name: id.to_string(),
            context_window_tokens: 0,
            max_output_tokens: 0,
            capabilities: pawork_domain::ModelCapabilities {
                text: true,
                tool_calls: true,
                ..Default::default()
            },
        })
        .collect()
}

#[derive(Clone, Debug)]
pub enum ResponsesAssemblyEvent {
    Canonical(ProviderStreamEvent),
    ReasoningOutputItem { wire: Value },
}

#[derive(Default)]
pub struct ResponsesStreamAssembler {
    function_calls: BTreeMap<String, String>,
    started: BTreeSet<String>,
    arguments_seen: BTreeSet<String>,
    completed_calls: BTreeSet<String>,
    response_id: Option<String>,
    usage: TokenUsage,
    stop_reason: Option<StopReason>,
    completed: bool,
}

impl ResponsesStreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, data: &str) -> Vec<ResponsesAssemblyEvent> {
        let value: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(_) => {
                return vec![ResponsesAssemblyEvent::Canonical(
                    ProviderStreamEvent::Error(ProviderError::new(
                        ProviderErrorKind::MalformedResponse,
                        "invalid Responses SSE JSON",
                    )),
                )]
            }
        };
        match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.created" | "response.in_progress" => self.created(&value),
            "response.output_text.delta" => string_delta(&value, ProviderStreamEvent::TextDelta),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                string_delta(&value, ProviderStreamEvent::ThinkingDelta)
            }
            "response.output_item.added" => self.item_added(value.get("item")),
            "response.function_call_arguments.delta" => self.arguments_delta(&value),
            "response.output_item.done" => self.item_done(value.get("item")),
            "response.completed" => self.response_completed(&value),
            "response.incomplete" => self.response_incomplete(&value),
            "response.failed" | "error" => self.response_failed(&value),
            _ => Vec::new(),
        }
    }

    pub fn finish(self) -> ResponsesFinalState {
        ResponsesFinalState {
            response_id: self.response_id,
            usage: self.usage,
            stop_reason: self.stop_reason,
            completed: self.completed,
        }
    }

    fn created(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let id = response(value)
            .and_then(|response| response.get("id"))
            .and_then(Value::as_str)
            .or_else(|| value.get("id").and_then(Value::as_str))
            .map(str::to_owned);
        if id.is_some() {
            self.response_id = id.clone();
        }
        vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ResponseStarted { response_id: id },
        )]
    }

    fn item_added(&mut self, item: Option<&Value>) -> Vec<ResponsesAssemblyEvent> {
        let Some(item) = item else { return Vec::new() };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return Vec::new();
        }
        self.start_function(item)
    }

    fn arguments_delta(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(call_id) = self.function_calls.get(item_id).cloned() else {
            return Vec::new();
        };
        let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
        if delta.is_empty() {
            return Vec::new();
        }
        self.arguments_seen.insert(item_id.to_string());
        vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ToolCallArgumentsDelta {
                id: ToolCallId::new(call_id),
                json: delta.to_string(),
            },
        )]
    }

    fn item_done(&mut self, item: Option<&Value>) -> Vec<ResponsesAssemblyEvent> {
        let Some(item) = item else { return Vec::new() };
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "reasoning" => vec![ResponsesAssemblyEvent::ReasoningOutputItem { wire: item.clone() }],
            "function_call" => {
                let mut events = self.start_function(item);
                let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .or_else(|| self.function_calls.get(item_id).map(String::as_str))
                    .unwrap_or(item_id)
                    .to_string();
                if !self.arguments_seen.contains(item_id) {
                    if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                        if !arguments.is_empty() {
                            events.push(ResponsesAssemblyEvent::Canonical(
                                ProviderStreamEvent::ToolCallArgumentsDelta {
                                    id: ToolCallId::new(call_id.clone()),
                                    json: arguments.to_string(),
                                },
                            ));
                        }
                    }
                }
                if self.completed_calls.insert(call_id.clone()) {
                    events.push(ResponsesAssemblyEvent::Canonical(
                        ProviderStreamEvent::ToolCallCompleted {
                            id: ToolCallId::new(call_id),
                        },
                    ));
                }
                events
            }
            _ => Vec::new(),
        }
    }

    fn start_function(&mut self, item: &Value) -> Vec<ResponsesAssemblyEvent> {
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or(item_id);
        if item_id.is_empty() || call_id.is_empty() {
            return Vec::new();
        }
        self.function_calls
            .insert(item_id.to_string(), call_id.to_string());
        if !self.started.insert(item_id.to_string()) {
            return Vec::new();
        }
        vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ToolCallStarted {
                id: ToolCallId::new(call_id),
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
        )]
    }

    fn response_completed(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let response = response(value).unwrap_or(value);
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            self.response_id = Some(id.to_string());
        }
        self.usage = normalize_usage(response);
        let has_tool_calls = !self.function_calls.is_empty();
        let status = response.get("status").and_then(Value::as_str);
        let stop = if !has_tool_calls && status == Some("completed") {
            StopReason::Completed
        } else {
            map_stop_reason(status, has_tool_calls)
        };
        self.stop_reason = Some(stop.clone());
        self.completed = true;
        let mut events = Vec::new();
        if self.usage != TokenUsage::default() {
            events.push(ResponsesAssemblyEvent::Canonical(
                ProviderStreamEvent::UsageUpdated(self.usage.clone()),
            ));
        }
        events.push(ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ResponseCompleted(stop),
        ));
        events
    }

    fn response_incomplete(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let response = response(value).unwrap_or(value);
        self.usage = normalize_usage(response);
        let reason = response
            .get("incomplete_details")
            .and_then(|details| details.get("reason"))
            .and_then(Value::as_str);
        let stop = map_stop_reason(reason, !self.function_calls.is_empty());
        self.stop_reason = Some(stop.clone());
        self.completed = true;
        vec![ResponsesAssemblyEvent::Canonical(
            ProviderStreamEvent::ResponseCompleted(stop),
        )]
    }

    fn response_failed(&mut self, value: &Value) -> Vec<ResponsesAssemblyEvent> {
        let response = response(value).unwrap_or(value);
        let error = response.get("error").unwrap_or(response);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Responses request failed");
        self.stop_reason = Some(StopReason::Error);
        self.completed = true;
        vec![
            ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::Error(ProviderError::new(
                ProviderErrorKind::Unknown,
                message,
            ))),
            ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::ResponseCompleted(
                StopReason::Error,
            )),
        ]
    }
}

fn string_delta(
    value: &Value,
    event: impl FnOnce(String) -> ProviderStreamEvent,
) -> Vec<ResponsesAssemblyEvent> {
    let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
    if delta.is_empty() {
        Vec::new()
    } else {
        vec![ResponsesAssemblyEvent::Canonical(event(delta.to_string()))]
    }
}

fn response(value: &Value) -> Option<&Value> {
    value.get("response").filter(|response| response.is_object())
}

#[derive(Clone, Debug)]
pub struct ResponsesFinalState {
    pub response_id: Option<String>,
    pub usage: TokenUsage,
    pub stop_reason: Option<StopReason>,
    pub completed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembler_streams_text_function_usage_and_completion() {
        let mut assembler = ResponsesStreamAssembler::new();
        assert!(matches!(
            assembler.feed(r#"{"type":"response.output_text.delta","delta":"hi"}"#)[0],
            ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::TextDelta(_))
        ));
        assembler.feed(r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read"}}"#);
        assembler.feed(r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{}"}"#);
        let done = assembler.feed(r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{}"}}"#);
        assert!(done.iter().any(|event| matches!(event, ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::ToolCallCompleted { .. }))));
        let completed = assembler.feed(r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":3,"output_tokens":2}}}"#);
        assert!(completed.iter().any(|event| matches!(event, ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::UsageUpdated(_)))));
        assert!(completed.iter().any(|event| matches!(event, ResponsesAssemblyEvent::Canonical(ProviderStreamEvent::ResponseCompleted(StopReason::ToolUse)))));
    }

    #[test]
    fn fixed_headers_cannot_override_auth() {
        let mut config = ResponsesTransportConfig::new("https://example.com", "test");
        config.request_headers.push(("Authorization".into(), "attacker".into()));
        let credential = ResolvedCredential::new(pawork_domain::CredentialKind::OAuthBearer, "token");
        let error = ResponsesTransport::new(config, credential).err().unwrap();
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);

        let mut config = ResponsesTransportConfig::new("https://example.com", "test");
        config
            .http
            .extra_headers
            .push(("x-api-key".into(), "attacker".into()));
        let credential = ResolvedCredential::new(pawork_domain::CredentialKind::ApiKey, "token");
        let error = ResponsesTransport::new(config, credential).err().unwrap();
        assert_eq!(error.kind, ProviderErrorKind::InvalidRequest);
    }
}
