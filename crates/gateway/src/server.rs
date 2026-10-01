//! Loopback-only OpenAI-compatible HTTP/1 gateway. No Agent session or tool execution.
use crate::{GatewayBackend, GatewayChatRequest, GatewayError, GatewayTokenStore};
use async_trait::async_trait;
use futures::stream;
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, Limited, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use pawork_domain::{
    CancellationToken, ModelPurpose, ProviderError, ProviderEventSink, ProviderStreamEvent,
    StopReason, TokenUsage,
};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinSet,
};

type Body = UnsyncBoxBody<Bytes, Infallible>;
type Jobs = Arc<Mutex<JoinSet<()>>>;
const BODY_LIMIT: usize = 2 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub async fn serve_gateway<B: GatewayBackend>(
    core: Arc<B>,
    listener: TcpListener,
    tokens: GatewayTokenStore,
    cancel: CancellationToken,
) -> io::Result<()> {
    let address = listener.local_addr()?;
    if address.ip() != std::net::Ipv4Addr::LOCALHOST {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "gateway requires 127.0.0.1",
        ));
    }
    let jobs: Jobs = Arc::new(Mutex::new(JoinSet::new()));
    let mut connections = JoinSet::new();
    let mut cleanup = tokio::time::interval(Duration::from_secs(1));
    let slots = Arc::new(tokio::sync::Semaphore::new(64));
    let result = loop {
        tokio::select! {
            _=cancel.cancelled()=>break Ok(()),
            accepted=listener.accept()=>{
                let (socket,_)=match accepted { Ok(value)=>value,Err(error)=>break Err(error) };
                let Ok(permit)=slots.clone().try_acquire_owned() else {drop(socket);continue};
                let core=core.clone();let tokens=tokens.clone();let jobs=jobs.clone();let stop=cancel.clone();
                connections.spawn(async move {
                    let _permit=permit;
                    let connection_cancel=CancellationToken::new();
                    let _guard=CancelOnDrop(connection_cancel.clone());
                    let handler=hyper::service::service_fn(move |request| handle(request,core.clone(),tokens.clone(),jobs.clone(),connection_cancel.clone(),address.port()));
                    let mut builder=hyper::server::conn::http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(15)).max_buf_size(32*1024);
                    let connection=builder.serve_connection(TokioIo::new(socket),handler);
                    tokio::pin!(connection);
                    tokio::select! { _=stop.cancelled()=>{},_=&mut connection=>{} }
                });
            },
            _=cleanup.tick()=>{
                while connections.try_join_next().is_some() {}
                let mut jobs=jobs.lock().expect("gateway jobs");
                while jobs.try_join_next().is_some() {}
            }
        }
    };
    cancel.cancel();
    while connections.join_next().await.is_some() {}
    // Connection guards cancel upstream work; wait for accounting and lease release.
    let mut remaining = std::mem::take(&mut *jobs.lock().expect("gateway jobs"));
    while remaining.join_next().await.is_some() {}
    result
}

fn json_response(status: u16, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store")
        .body(Full::new(Bytes::from(value.to_string())).boxed_unsync())
        .expect("static response")
}
fn error_value(error: &GatewayError) -> Value {
    json!({"error":{"message":error.message,"type":if error.status<500 {"invalid_request_error"} else {"server_error"},"code":error.code,"param":null}})
}
fn error_response(error: GatewayError) -> Response<Body> {
    json_response(error.status, error_value(&error))
}

async fn handle<B: GatewayBackend>(
    request: Request<Incoming>,
    core: Arc<B>,
    tokens: GatewayTokenStore,
    jobs: Jobs,
    connection_cancel: CancellationToken,
    port: u16,
) -> Result<Response<Body>, Infallible> {
    let response = handle_inner(request, core, tokens, jobs, connection_cancel, port)
        .await
        .unwrap_or_else(error_response);
    Ok(response)
}
async fn handle_inner<B: GatewayBackend>(
    request: Request<Incoming>,
    core: Arc<B>,
    tokens: GatewayTokenStore,
    jobs: Jobs,
    connection_cancel: CancellationToken,
    port: u16,
) -> Result<Response<Body>, GatewayError> {
    let host = request
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err(GatewayError::new(
            403,
            "invalid_host",
            "Loopback Host header required.",
        ));
    }
    if let Some(origin) = request.headers().get("origin") {
        if origin.to_str().ok() != Some(format!("http://{host}").as_str()) {
            return Err(GatewayError::new(
                403,
                "invalid_origin",
                "Cross-origin access is not supported.",
            ));
        }
    }
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_string();
    let client = tokio::task::spawn_blocking(move || tokens.authenticate(&bearer))
        .await
        .map_err(|_| {
            GatewayError::new(
                503,
                "authentication_unavailable",
                "Gateway authentication unavailable.",
            )
        })?
        .map_err(|_| {
            GatewayError::new(
                503,
                "authentication_unavailable",
                "Gateway authentication unavailable.",
            )
        })?
        .ok_or_else(|| {
            GatewayError::new(401, "invalid_api_key", "Invalid or revoked gateway token.")
        })?;
    let path = request.uri().path();
    let query = request.uri().query().unwrap_or_default();
    if path == "/v1/models" {
        if request.method() != Method::GET {
            return Err(GatewayError::new(405, "method_not_allowed", "Use GET."));
        }
        // ADR-064（Gateway v1.1）：?purpose=<name> 可重复，取交集；
        // 未知参数 / 未知用途值 fail-closed。缺省 = v1 口径（仅 text 模型）。
        let mut purposes = Vec::new();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').ok_or_else(GatewayError::invalid)?;
            if key != "purpose" {
                return Err(GatewayError::invalid());
            }
            let purpose = ModelPurpose::from_wire_name(value).ok_or_else(GatewayError::invalid)?;
            if !purposes.contains(&purpose) {
                purposes.push(purpose);
            }
        }
        let models = tokio::time::timeout(Duration::from_secs(15), core.gateway_models(&purposes))
            .await
            .map_err(|_| GatewayError::timeout())??;
        return Ok(json_response(200, json!({"object":"list","data":models})));
    }
    if path != "/v1/chat/completions" {
        return Err(GatewayError::new(404, "not_found", "Unknown API route."));
    }
    if !query.is_empty() {
        return Err(GatewayError::invalid());
    }
    if request.method() != Method::POST {
        return Err(GatewayError::new(405, "method_not_allowed", "Use POST."));
    }
    if !request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        })
    {
        return Err(GatewayError::new(
            415,
            "unsupported_media_type",
            "Use application/json.",
        ));
    }
    let body = tokio::time::timeout(
        Duration::from_secs(15),
        Limited::new(request.into_body(), BODY_LIMIT).collect(),
    )
    .await
    .map_err(|_| GatewayError::new(408, "request_timeout", "Request body timed out."))?
    .map_err(|_| {
        GatewayError::new(
            413,
            "request_too_large",
            "Request body is invalid or too large.",
        )
    })?
    .to_bytes();
    let input: GatewayChatRequest =
        serde_json::from_slice(&body).map_err(|_| GatewayError::invalid())?;
    let streaming = input.stream;
    let include_usage = input
        .stream_options
        .as_ref()
        .is_some_and(|v| v.include_usage);
    let model = input.model.clone();
    let prepared = tokio::time::timeout(
        Duration::from_secs(30),
        core.prepare_gateway_completion(input),
    )
    .await
    .map_err(|_| GatewayError::timeout())??;
    let cancel = CancellationToken::new();
    let guard = CancelOnDrop(cancel.clone());
    let (sender, receiver) = mpsc::channel::<Bytes>(32);
    let (done, finish) = oneshot::channel();
    let created = crate::unix_millis() / 1000;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| {
        GatewayError::new(
            500,
            "entropy_unavailable",
            "Cannot create response identity.",
        )
    })?;
    let id = format!(
        "chatcmpl-{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let sink = Arc::new(OutputSink {
        sender: streaming.then_some(sender.clone()),
        text: Mutex::new(String::new()),
        images: Mutex::new(Vec::new()),
        bytes: std::sync::atomic::AtomicUsize::new(0),
        id: id.clone(),
        model: model.clone(),
        created,
        include_usage,
    });
    jobs.lock().expect("gateway jobs").spawn(async move {
        let execution=core.run_gateway_completion(&client,prepared,sink.clone(),cancel.clone());
        tokio::pin!(execution);
        let result=tokio::select! {
            value=&mut execution=>value,
            _=connection_cancel.cancelled()=>{ cancel.cancel(); execution.await }
        };
        if streaming {
            let frames=match result {
                Ok(summary)=>{
                    let mut values=vec![chunk(&id,&model,created,json!({}),Some(finish_reason(&summary.stop_reason)),include_usage)];
                    if include_usage {values.push(json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[],"usage":usage_json(&summary.usage)}));}
                    values
                },
                Err(error)=>vec![error_value(&error)],
            };
            for value in frames {
                tokio::select! {
                    _=connection_cancel.cancelled()=>break,
                    result=tokio::time::timeout(Duration::from_secs(5),sender.send(sse(value)))=>{
                        if !matches!(result,Ok(Ok(()))){break}
                    }
                }
            }
            let _=tokio::time::timeout(Duration::from_secs(5),sender.send(Bytes::from_static(b"data: [DONE]\n\n"))).await;
        } else {
            // ADR-064：图像生成模型透传 image part（message.images 数组，
            // additive 字段；文本模型恒为空数组，v1 客户端可忽略）。
            let images = sink.images.lock().expect("output").clone();
            let value=result.map(|summary|json!({"id":id,"object":"chat.completion","created":created,"model":model,"choices":[{"index":0,"message":{"role":"assistant","content":sink.text.lock().expect("output").clone(),"images":images.iter().map(|url|json!({"url":url})).collect::<Vec<_>>()},"finish_reason":finish_reason(&summary.stop_reason)}],"usage":usage_json(&summary.usage)}));
            let _=done.send(value);
        }
    });
    if streaming {
        let stream = stream::unfold((receiver, guard), |(mut receiver, guard)| async move {
            receiver
                .recv()
                .await
                .map(|bytes| (Ok::<_, Infallible>(Frame::data(bytes)), (receiver, guard)))
        });
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(StreamBody::new(stream).boxed_unsync())
            .expect("SSE response"))
    } else {
        let result = finish
            .await
            .map_err(|_| GatewayError::new(500, "request_failed", "Model request failed."))?;
        drop(guard);
        Ok(json_response(200, result?))
    }
}
fn sse(value: Value) -> Bytes {
    Bytes::from(format!("data: {value}\n\n"))
}
fn chunk(
    id: &str,
    model: &str,
    created: u64,
    delta: Value,
    finish: Option<&str>,
    include_usage: bool,
) -> Value {
    let mut value = json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
    if include_usage {
        value["usage"] = Value::Null;
    }
    value
}
fn finish_reason(reason: &StopReason) -> &'static str {
    match reason {
        StopReason::MaxTokens => "length",
        StopReason::ContentFiltered => "content_filter",
        StopReason::ToolUse => "tool_calls",
        _ => "stop",
    }
}
fn usage_json(usage: &TokenUsage) -> Value {
    json!({"prompt_tokens":usage.input_tokens,"completion_tokens":usage.output_tokens,"total_tokens":usage.input_tokens.saturating_add(usage.output_tokens),"prompt_tokens_details":{"cached_tokens":usage.cache_read_tokens}})
}
struct OutputSink {
    sender: Option<mpsc::Sender<Bytes>>,
    text: Mutex<String>,
    images: Mutex<Vec<String>>,
    bytes: std::sync::atomic::AtomicUsize,
    id: String,
    model: String,
    created: u64,
    include_usage: bool,
}
#[async_trait]
impl ProviderEventSink for OutputSink {
    async fn emit(&self, event: ProviderStreamEvent) -> Result<(), ProviderError> {
        let delta = match event {
            ProviderStreamEvent::TextDelta(text) => {
                let size = self
                    .bytes
                    .fetch_add(text.len(), std::sync::atomic::Ordering::Relaxed)
                    + text.len();
                if size > OUTPUT_LIMIT {
                    return Err(ProviderError::new(
                        pawork_domain::ProviderErrorKind::InvalidRequest,
                        "gateway output limit exceeded",
                    ));
                }
                if self.sender.is_none() {
                    self.text.lock().expect("output").push_str(&text);
                }
                json!({"content":text})
            }
            ProviderStreamEvent::ImageOutput { url } => {
                let size = self
                    .bytes
                    .fetch_add(url.len(), std::sync::atomic::Ordering::Relaxed)
                    + url.len();
                if size > OUTPUT_LIMIT {
                    return Err(ProviderError::new(
                        pawork_domain::ProviderErrorKind::InvalidRequest,
                        "gateway output limit exceeded",
                    ));
                }
                self.images.lock().expect("output").push(url.clone());
                json!({"images":[{"url":url}]})
            }
            ProviderStreamEvent::ResponseStarted { .. } => json!({"role":"assistant","content":""}),
            _ => return Ok(()),
        };
        if let Some(sender) = &self.sender {
            sender
                .send(sse(chunk(
                    &self.id,
                    &self.model,
                    self.created,
                    delta,
                    None,
                    self.include_usage,
                )))
                .await
                .map_err(|_| ProviderError::cancelled("gateway client disconnected"))?;
        }
        Ok(())
    }
}

/// PID files alone cannot establish process ownership.
pub fn gateway_is_running(directory: &std::path::Path) -> io::Result<bool> {
    let path = directory.join("gateway.lock");
    if !path.exists() {
        return Ok(false);
    }
    Ok(pawork_auth::try_acquire_file_lock(&path)
        .map_err(io::Error::other)?
        .is_none())
}
