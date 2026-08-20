//! Internal MCP SDK isolation. This is the only module that names SDK model types.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use pawork_domain::ToolResult;
use pawork_domain::{
    CancellationToken, ContentPart, ErrorCategory, ErrorContext, ImageContent, ImageSource,
    TextContent,
};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, ContentBlock,
    PingRequest, ServerResult, Tool,
};
use rmcp::service::{Peer, PeerRequestOptions, RoleClient, RunningService, ServiceError};
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::mcp::config::is_loopback_url;
use crate::mcp::transport::HttpTransportConfig;
use crate::mcp::{McpError, McpServerCapabilities, McpToolCall, McpToolInfo};

pub(crate) struct RunningClient {
    inner: RunningService<RoleClient, ()>,
}

#[derive(Clone)]
pub(crate) struct ClientPeer {
    inner: Peer<RoleClient>,
}

impl RunningClient {
    pub(crate) fn peer(&self) -> ClientPeer {
        ClientPeer {
            inner: self.inner.peer().clone(),
        }
    }

    pub(crate) fn is_dead(&self) -> bool {
        self.inner.is_closed() || self.inner.peer().is_transport_closed()
    }

    pub(crate) async fn close_with_timeout(&mut self, timeout: Duration) {
        let _ = self.inner.close_with_timeout(timeout).await;
    }
}

pub(crate) fn tool_to_info(tool: &Tool) -> McpToolInfo {
    let read_only = tool
        .annotations
        .as_ref()
        .and_then(|annotations| annotations.read_only_hint)
        .unwrap_or(false);
    McpToolInfo {
        name: tool.name.to_string(),
        description: tool
            .description
            .as_deref()
            .map(str::to_string)
            .unwrap_or_default(),
        input_schema: tool.schema_as_json_value(),
        read_only,
    }
}

pub(crate) fn call_to_params(call: McpToolCall) -> CallToolRequestParams {
    CallToolRequestParams::new(call.name).with_arguments(call.arguments)
}

pub(crate) fn call_result_to_tool_result(result: &CallToolResult) -> ToolResult {
    let parts: Vec<ContentPart> = result
        .content
        .iter()
        .filter_map(content_block_to_part)
        .collect();
    let metadata = result
        .structured_content
        .as_ref()
        .map(|structured| json!({"mcp": {"structured_content": structured}}))
        .unwrap_or(Value::Null);
    let is_error = result.is_error.unwrap_or(false);
    let error = if is_error {
        let message = parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(ErrorContext {
            category: ErrorCategory::Tool,
            message: if message.is_empty() {
                "MCP server reported a tool error".to_string()
            } else {
                message
            },
            retryable: false,
            retry_after_ms: None,
            diagnostics: Default::default(),
        })
    } else {
        None
    };

    ToolResult {
        content: parts,
        artifacts: Vec::new(),
        metadata,
        truncated: false,
        success: !is_error,
        error,
    }
}

fn content_block_to_part(block: &ContentBlock) -> Option<ContentPart> {
    match block {
        ContentBlock::Text(text) => Some(ContentPart::Text(TextContent {
            text: text.text.clone(),
        })),
        ContentBlock::Image(image) => Some(ContentPart::Image(ImageContent {
            source: ImageSource::Base64(image.data.clone()),
            media_type: image.mime_type.clone(),
            alt_text: None,
        })),
        ContentBlock::Resource(embedded) => {
            let text = embedded.get_text();
            if text.is_empty() {
                None
            } else {
                Some(ContentPart::Text(TextContent { text }))
            }
        }
        _ => None,
    }
}

/// Enforce a hard byte cap. Text that overflows is truncated on a UTF-8 boundary.
pub(crate) fn apply_output_cap(
    parts: Vec<ContentPart>,
    max_output_bytes: u64,
) -> (Vec<ContentPart>, bool, usize) {
    let cap = usize::try_from(max_output_bytes.max(1)).unwrap_or(usize::MAX);
    let mut budget = cap;
    let mut out = Vec::with_capacity(parts.len());
    let mut truncated = false;

    for part in parts {
        if budget == 0 {
            truncated = true;
            continue;
        }
        match part {
            ContentPart::Text(mut text_content) => {
                let text = std::mem::take(&mut text_content.text);
                let len = text.len();
                if len <= budget {
                    budget -= len;
                    out.push(ContentPart::Text(TextContent { text }));
                } else {
                    let cut = char_boundary_down(&text, budget);
                    out.push(ContentPart::Text(TextContent {
                        text: text[..cut].to_string(),
                    }));
                    truncated = true;
                    budget = 0;
                }
            }
            ContentPart::Image(image) => {
                let len = image_byte_size(&image);
                if len <= budget {
                    budget -= len;
                    out.push(ContentPart::Image(image));
                } else {
                    truncated = true;
                    budget = 0;
                }
            }
            other => out.push(other),
        }
    }

    (out, truncated, budget)
}

pub(crate) fn apply_tool_result_budget(result: ToolResult, max_output_bytes: u64) -> ToolResult {
    let (content, mut truncated, remaining) = apply_output_cap(result.content, max_output_bytes);
    let metadata = result
        .metadata
        .get("mcp")
        .and_then(|mcp| mcp.get("structured_content"))
        .and_then(|structured| {
            let metadata = json!({"mcp": {"structured_content": structured}});
            let encoded = serde_json::to_vec(&metadata).ok()?;
            if encoded.len() <= remaining {
                Some(metadata)
            } else {
                truncated = true;
                None
            }
        })
        .or_else(|| {
            if result.metadata.is_null() {
                None
            } else if result.metadata.get("mcp").is_none() {
                Some(result.metadata.clone())
            } else {
                None
            }
        })
        .unwrap_or(Value::Null);

    ToolResult {
        content,
        artifacts: result.artifacts,
        metadata,
        truncated: result.truncated || truncated,
        success: result.success,
        error: result.error,
    }
}

fn char_boundary_down(value: &str, mut index: usize) -> usize {
    if index >= value.len() {
        return value.len();
    }
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn image_byte_size(image: &ImageContent) -> usize {
    match &image.source {
        ImageSource::Base64(data) => data.len(),
        _ => 0,
    }
}

pub(crate) async fn serve_stdio(
    read: impl AsyncRead + Unpin + Send + 'static,
    write: impl AsyncWrite + Unpin + Send + 'static,
) -> Result<RunningClient, McpError> {
    let transport = AsyncRwTransport::new_client(read, write);
    ().serve(transport)
        .await
        .map(|inner| RunningClient { inner })
        .map_err(|_| McpError::Transport("stdio handshake failed".into()))
}

pub(crate) async fn serve_http(cfg: &HttpTransportConfig) -> Result<RunningClient, McpError> {
    let rmcp_config = build_http_transport_config(cfg)?;
    let transport = StreamableHttpClientTransport::from_config(rmcp_config);
    ().serve(transport)
        .await
        .map(|inner| RunningClient { inner })
        .map_err(|_| McpError::Transport("http handshake failed".into()))
}

pub(crate) fn validate_http_transport_config(cfg: &HttpTransportConfig) -> Result<(), McpError> {
    build_http_transport_config(cfg).map(|_| ())
}

fn build_http_transport_config(
    cfg: &HttpTransportConfig,
) -> Result<StreamableHttpClientTransportConfig, McpError> {
    use reqwest::header::{HeaderName, HeaderValue};

    let url = cfg.url.trim();
    if url.is_empty() {
        return Err(McpError::Config("http url must not be empty".into()));
    }
    let parsed = url::Url::parse(url)
        .map_err(|error| McpError::Config(format!("invalid http url: {error}")))?;
    if cfg.auth_token.as_deref().is_some_and(str::is_empty) {
        return Err(McpError::Config("auth_token must not be empty".into()));
    }
    let has_authorization_header = cfg
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"));
    if cfg.auth_token.is_some() && has_authorization_header {
        return Err(McpError::Config(
            "auth_token conflicts with a custom Authorization header".into(),
        ));
    }
    if parsed.scheme() == "http"
        && (cfg.auth_token.is_some() || !cfg.headers.is_empty())
        && !is_loopback_url(&parsed)
    {
        return Err(McpError::Config(
            "secret-bearing HTTP authentication requires HTTPS for non-loopback endpoints".into(),
        ));
    }

    let mut rmcp_config = StreamableHttpClientTransportConfig::with_uri(url.to_owned());
    if let Some(token) = &cfg.auth_token {
        rmcp_config = rmcp_config.auth_header(token.clone());
    }
    if !cfg.headers.is_empty() {
        let mut headers = HashMap::with_capacity(cfg.headers.len());
        for (name, value) in &cfg.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| McpError::Config(format!("invalid header name {name:?}: {e}")))?;
            let header_value = HeaderValue::from_str(value)
                .map_err(|e| McpError::Config(format!("invalid header value for {name:?}: {e}")))?;
            if headers.insert(header_name, header_value).is_some() {
                return Err(McpError::Config(format!("duplicate header {name:?}")));
            }
        }
        rmcp_config = rmcp_config.custom_headers(headers);
    }
    Ok(rmcp_config)
}

pub(crate) fn map_service_error(error: ServiceError) -> McpError {
    match error {
        ServiceError::McpError(data) => McpError::Protocol(data.to_string()),
        ServiceError::TransportSend(inner) => {
            McpError::Disconnected(format!("mcp transport send failed: {inner}"))
        }
        ServiceError::TransportClosed => McpError::Disconnected("mcp transport closed".into()),
        ServiceError::UnexpectedResponse => {
            McpError::Protocol("unexpected response from mcp server".into())
        }
        ServiceError::Cancelled { .. } => McpError::Cancelled,
        ServiceError::Timeout { timeout } => McpError::Timeout(timeout),
        other => McpError::Transport(format!("mcp service error: {other}")),
    }
}

pub(crate) fn should_retry(error: &McpError) -> bool {
    matches!(error, McpError::Disconnected(_))
}

impl ClientPeer {
    pub(crate) fn server_capabilities(&self) -> Result<McpServerCapabilities, McpError> {
        let info = self.inner.peer_info().ok_or_else(|| {
            McpError::Protocol("MCP server omitted initialize handshake information".into())
        })?;
        Ok(McpServerCapabilities {
            tools: info.capabilities.tools.is_some(),
            resources: info.capabilities.resources.is_some(),
            prompts: info.capabilities.prompts.is_some(),
        })
    }

    pub(crate) async fn list_tools(&self) -> Result<Vec<McpToolInfo>, ServiceError> {
        self.inner
            .list_all_tools()
            .await
            .map(|tools| tools.iter().map(tool_to_info).collect())
    }

    pub(crate) async fn ping(&self) -> Result<(), ServiceError> {
        self.inner
            .send_request(ClientRequest::PingRequest(PingRequest::default()))
            .await
            .map(|_| ())
    }

    pub(crate) async fn call_tool(
        &self,
        call: McpToolCall,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ToolResult, McpError> {
        let params = call_to_params(call);
        let handle = self
            .inner
            .send_cancellable_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                PeerRequestOptions::with_timeout(timeout),
            )
            .await
            .map_err(map_service_error)?;

        let mut handle = Some(handle);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if let Some(h) = handle.take() {
                    let _ = h
                        .cancel(Some("client requested cancellation".to_string()))
                        .await;
                }
                Err(McpError::Cancelled)
            }
            result = async {
                let Some(h) = handle.take() else {
                    return Err(McpError::Protocol(
                        "MCP call handle was consumed before response".into(),
                    ));
                };
                h.await_response().await.map_err(map_service_error)
            } => match result? {
                ServerResult::CallToolResult(ct) => Ok(call_result_to_tool_result(&ct)),
                other => Err(McpError::Protocol(match other {
                    ServerResult::InputRequiredResult(_) => {
                        "MCP server requested additional input; not supported".into()
                    }
                    _ => format!("unexpected call_tool response: {other:?}"),
                })),
            },
        }
    }
}

pub(crate) async fn timed<F, T>(timeout: Duration, fut: F) -> Result<T, McpError>
where
    F: Future<Output = Result<T, ServiceError>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(inner) => inner.map_err(map_service_error),
        Err(_) => Err(McpError::Timeout(timeout)),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use std::future::Future;

    use rmcp::model::{
        CallToolRequestParams, CallToolResult, ContentBlock, JsonObject, ListToolsResult,
        ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::ServerHandler;
    use rmcp::{ErrorData, RoleServer, ServiceExt as _};

    use super::RunningClient;
    use crate::mcp::McpError;

    #[derive(Clone, Copy)]
    pub(crate) enum ServerBehavior {
        Echo,
        Slow { delay: Duration },
    }

    #[derive(Clone)]
    struct EchoServer {
        behavior: ServerBehavior,
    }

    impl ServerHandler for EchoServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send {
            let tool = Tool::new(
                "echo",
                "echo the tool name back",
                Arc::new(JsonObject::new()),
            );
            std::future::ready(Ok(ListToolsResult::with_all_items(vec![tool])))
        }

        fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> impl Future<Output = Result<rmcp::model::CallToolResponse, ErrorData>> + Send {
            let behavior = self.behavior;
            async move {
                match behavior {
                    ServerBehavior::Echo => {
                        let text = format!("echo: {}", request.name);
                        Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
                    }
                    ServerBehavior::Slow { delay } => {
                        tokio::select! {
                            _ = tokio::time::sleep(delay) => {
                                Ok(CallToolResult::success(vec![ContentBlock::text("slow-done")]).into())
                            }
                            _ = context.ct.cancelled() => {
                                Ok(CallToolResult::success(vec![ContentBlock::text("cancelled")]).into())
                            }
                        }
                    }
                }
            }
        }
    }

    pub(crate) struct InProcessConnector {
        server: EchoServer,
        fail_until: u32,
        connect_delay: Duration,
        calls: Arc<AtomicU32>,
    }

    impl InProcessConnector {
        pub(crate) fn echo() -> Self {
            Self {
                server: EchoServer {
                    behavior: ServerBehavior::Echo,
                },
                fail_until: 0,
                connect_delay: Duration::ZERO,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }

        pub(crate) fn slow(delay: Duration) -> Self {
            Self {
                server: EchoServer {
                    behavior: ServerBehavior::Slow { delay },
                },
                fail_until: 0,
                connect_delay: Duration::ZERO,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }

        pub(crate) fn failing(fail_until: u32, behavior: ServerBehavior) -> Self {
            Self {
                server: EchoServer { behavior },
                fail_until,
                connect_delay: Duration::ZERO,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }

        pub(crate) fn delayed(delay: Duration) -> Self {
            Self {
                server: EchoServer {
                    behavior: ServerBehavior::Echo,
                },
                fail_until: 0,
                connect_delay: delay,
                calls: Arc::new(AtomicU32::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::mcp::transport::McpConnector for InProcessConnector {
        fn transport_name(&self) -> &'static str {
            "test-in-process"
        }

        async fn connect(&self) -> Result<RunningClient, McpError> {
            if !self.connect_delay.is_zero() {
                tokio::time::sleep(self.connect_delay).await;
            }
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_until {
                return Err(McpError::Transport(format!(
                    "injected connect failure #{n}"
                )));
            }
            let (server_io, client_io) = tokio::io::duplex(8192);
            let server = self.server.clone();
            tokio::spawn(async move {
                let running = match server.serve(server_io).await {
                    Ok(r) => r,
                    Err(error) => {
                        tracing::warn!(%error, "test server failed to initialize");
                        return;
                    }
                };
                let _ = running.waiting().await;
            });
            ().serve(client_io)
                .await
                .map(|inner| RunningClient { inner })
                .map_err(|e| McpError::Transport(format!("test client handshake failed: {e}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::transport::HttpTransportConfig;

    #[test]
    fn http_config_rejects_invalid_inputs() {
        assert!(matches!(
            build_http_transport_config(&HttpTransportConfig::new("")),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            build_http_transport_config(&HttpTransportConfig::new("not a url")),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            build_http_transport_config(
                &HttpTransportConfig::new("https://h/mcp").with_auth_token("")
            ),
            Err(McpError::Config(_))
        ));
        assert!(matches!(
            build_http_transport_config(
                &HttpTransportConfig::new("http://remote.example/mcp").with_auth_token("secret")
            ),
            Err(McpError::Config(_))
        ));
        let mut headers = HashMap::new();
        headers.insert("authorization".into(), "Bearer duplicate".into());
        assert!(matches!(
            build_http_transport_config(
                &HttpTransportConfig::new("https://remote.example/mcp")
                    .with_auth_token("secret")
                    .with_headers(headers)
            ),
            Err(McpError::Config(_))
        ));
    }

    #[test]
    fn http_config_applies_auth_and_headers() {
        use reqwest::header::HeaderName;

        let mut headers = HashMap::new();
        headers.insert("X-Trace-Id".to_string(), "abc".to_string());
        let cfg = HttpTransportConfig::new("https://host/mcp")
            .with_auth_token("tok")
            .with_headers(headers);

        let rmcp_config = build_http_transport_config(&cfg).expect("valid config");
        assert_eq!(rmcp_config.auth_header.as_deref(), Some("tok"));
        assert_eq!(
            rmcp_config
                .custom_headers
                .get(&HeaderName::from_static("x-trace-id"))
                .map(|v| v.to_str().unwrap()),
            Some("abc")
        );
        assert_eq!(rmcp_config.uri.as_ref(), "https://host/mcp");
    }

    #[test]
    fn tool_round_trip_preserves_read_only_hint() {
        let mut tool = Tool::new(
            "search",
            "find things",
            json!({"type":"object"}).as_object().unwrap().clone(),
        );
        tool = tool.with_annotations(rmcp::model::ToolAnnotations::new().read_only(true));
        let info = tool_to_info(&tool);
        assert_eq!(info.name, "search");
        assert_eq!(info.description, "find things");
        assert!(info.read_only);
        let params = call_to_params(McpToolCall {
            name: info.name,
            arguments: serde_json::Map::new(),
        });
        assert_eq!(params.name, "search");
    }

    #[test]
    fn call_result_marks_utf8_truncation() {
        let result = CallToolResult::success(vec![ContentBlock::text("hello世界")]);
        let converted = call_result_to_tool_result(&result);
        let (parts, truncated, _) = apply_output_cap(converted.content, 6);
        assert!(truncated);
        let ContentPart::Text(text) = &parts[0] else {
            panic!("expected text");
        };
        assert!(text.text.is_char_boundary(text.text.len()));
        assert!(text.text.starts_with("hello"));
    }
}
