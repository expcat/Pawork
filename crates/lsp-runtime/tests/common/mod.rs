#![allow(dead_code)]
//! 测试公共：in-memory mock 语言服务 + 注入式 MockSpawner。
//!
//! 不启动真实 OS 进程，也不使用 tokio::process；全部经 `tokio::io::duplex` 在进程内
//! 模拟 server↔client 的 Content-Length 字节流。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use lsp_runtime::framing::{
    encode_message, FrameEvent, LspFrameDecoder, MAX_FRAME_BYTES_HARD_LIMIT,
};
use lsp_runtime::transport::{
    ServerLifecycle, ServerReader, ServerWriter, SharedSpawner, SpawnedServer,
};
use lsp_runtime::ResultPayload;
use lsp_runtime::{CancellationToken, LanguageServerDescriptor, LspError, ServerSpawnConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::oneshot;
use tokio::sync::Mutex;

/// 解包 `ResultPayload` 的内联值；大结果未配置 sink 时应显式失败，
/// 测试断言走到 Artifact 分支即为测试错误。
pub fn inline<T>(payload: ResultPayload<T>) -> T {
    match payload {
        ResultPayload::Inline(v) => v,
        ResultPayload::Artifact(_) => panic!("unexpected artifact payload without sink"),
    }
}

/// mock server 对一条消息的响应动作。
#[derive(Debug, Clone)]
pub enum MockAction {
    Respond(Value),
    Error(i32, String),
    Notify(String, Value),
    Crash,
    Ignore,
}

/// mock 消息处理器：输入 (method, params, has_id)，输出动作。
pub type MockHandler = Arc<dyn Fn(&str, &Value, bool) -> MockAction + Send + Sync>;

#[derive(Clone)]
pub struct MockServerSpec {
    pub capabilities: Value,
    pub handler: MockHandler,
    /// `initialize` 响应的可选延迟：用于制造可观测的 restart 握手窗口。
    pub init_delay: Option<std::time::Duration>,
}

/// 全部 provider 开启的能力 JSON。
pub fn full_capabilities() -> Value {
    serde_json::json!({
        "textDocumentSync": 2,
        "hoverProvider": true,
        "definitionProvider": true,
        "referencesProvider": true,
        "documentSymbolProvider": true,
        "workspaceSymbolProvider": true,
        "callHierarchyProvider": true,
        "renameProvider": true,
        "codeActionProvider": true,
        "diagnosticProvider": true,
    })
}

struct MockReader(tokio::io::ReadHalf<DuplexStream>);
struct MockWriter(tokio::io::WriteHalf<DuplexStream>);
/// close() 时触发 mock server 任务退出并 drop 服务端流，使客户端读到 EOF——
/// 与真实 Sandbox/Process Runtime 终止进程树的行为对齐。
struct MockLifecycle {
    shutdown: Option<oneshot::Sender<()>>,
    closes: Arc<AtomicUsize>,
}

#[async_trait]
impl ServerReader for MockReader {
    async fn read(&mut self) -> Result<Option<Vec<u8>>, LspError> {
        let mut buf = vec![0u8; 4096];
        match self.0.read(&mut buf).await {
            Ok(0) => Ok(None),
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(e) => Err(LspError::Transport(e.to_string())),
        }
    }
}

#[async_trait]
impl ServerWriter for MockWriter {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), LspError> {
        self.0
            .write_all(bytes)
            .await
            .map_err(|e| LspError::Transport(e.to_string()))
    }
}

#[async_trait]
impl ServerLifecycle for MockLifecycle {
    async fn close(&mut self) -> Result<(), LspError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

/// 跨所有 spawn 的生命周期观测（验证失败路径不泄漏新进程）。
#[derive(Clone, Default)]
pub struct MockSpawnerStats {
    /// 所有已产出 lifecycle 的 close() 调用总数。
    pub lifecycle_closes: Arc<AtomicUsize>,
}

/// 注入式 mock spawner：每次 spawn 弹出下一份规格，启动对应 mock 服务任务。
#[derive(Clone)]
pub struct MockSpawner {
    specs: Arc<Mutex<VecDeque<MockServerSpec>>>,
    pub stats: MockSpawnerStats,
}

impl MockSpawner {
    pub fn new(specs: Vec<MockServerSpec>) -> Self {
        Self {
            specs: Arc::new(Mutex::new(specs.into())),
            stats: MockSpawnerStats::default(),
        }
    }

    pub fn single(handler: MockHandler) -> Self {
        Self::new(vec![MockServerSpec {
            capabilities: full_capabilities(),
            handler,
            init_delay: None,
        }])
    }

    pub fn into_shared(self) -> SharedSpawner {
        Arc::new(self)
    }
}

#[async_trait]
impl lsp_runtime::ServerSpawner for MockSpawner {
    async fn spawn(
        &self,
        _descriptor: &LanguageServerDescriptor,
        _config: &ServerSpawnConfig,
        _cancel: CancellationToken,
    ) -> Result<SpawnedServer, LspError> {
        let spec = {
            let mut q = self.specs.lock().await;
            if q.len() == 1 {
                q[0].clone()
            } else {
                q.pop_front().expect("mock spawner specs exhausted")
            }
        };
        let (client_end, server_end) = tokio::io::duplex(8 * 1024);
        let (client_read, client_write) = tokio::io::split(client_end);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let closes = self.stats.lifecycle_closes.clone();
        tokio::spawn(run_mock_server(server_end, spec, shutdown_rx));
        Ok(SpawnedServer {
            reader: Box::new(MockReader(client_read)),
            writer: Box::new(MockWriter(client_write)),
            lifecycle: Box::new(MockLifecycle {
                shutdown: Some(shutdown_tx),
                closes,
            }),
        })
    }
}

async fn run_mock_server(
    mut stream: DuplexStream,
    spec: MockServerSpec,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut decoder = LspFrameDecoder::new(MAX_FRAME_BYTES_HARD_LIMIT);
    let mut buf = vec![0u8; 4096];
    loop {
        tokio::select! {
            _ = &mut shutdown => return,
            r = stream.read(&mut buf) => match r {
                Ok(0) => return,
                Ok(n) => {
                    decoder.feed(&buf[..n]);
                    loop {
                        match decoder.decode_next() {
                            Ok(FrameEvent::Complete(body)) => {
                                if handle_one(&mut stream, &body, &spec).await.is_err() {
                                    return;
                                }
                            }
                            Ok(FrameEvent::NeedMoreData) => break,
                            Err(_) => return,
                        }
                    }
                }
                Err(_) => return,
            },
        }
    }
}

async fn handle_one(
    stream: &mut DuplexStream,
    body: &[u8],
    spec: &MockServerSpec,
) -> Result<(), LspError> {
    let msg: Value =
        serde_json::from_slice(body).map_err(|e| LspError::Transport(e.to_string()))?;
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let id = msg.get("id").cloned();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let has_id = id.is_some();

    if method == "initialize" {
        if let Some(id) = id {
            // 允许 handler 用 Crash 模拟「initialize 握手期间崩溃」（restart
            // 预算重试 / 失败清理测试用）；其余情况仍返回标准 capabilities。
            if matches!(
                (spec.handler)("initialize", &params, true),
                MockAction::Crash
            ) {
                return Err(LspError::Transport("mock crash during initialize".into()));
            }
            if let Some(delay) = spec.init_delay {
                tokio::time::sleep(delay).await;
            }
            // LSP initialize result 形如 `{ "capabilities": {...} }`。
            write_response(stream, &id, json!({ "capabilities": spec.capabilities })).await?;
        }
        return Ok(());
    }
    if matches!(method, "initialized" | "exit" | "$/cancelRequest") {
        return Ok(());
    }
    if method == "shutdown" {
        if let Some(id) = id {
            write_response(stream, &id, Value::Null).await?;
        }
        return Ok(());
    }

    let action = (spec.handler)(method, &params, has_id);
    match action {
        MockAction::Respond(v) => {
            if let Some(id) = id {
                write_response(stream, &id, v).await?;
            }
        }
        MockAction::Error(code, message) => {
            if let Some(id) = id {
                write_error(stream, &id, code, message).await?;
            }
        }
        MockAction::Notify(m, params) => {
            write_notify(stream, &m, params).await?;
        }
        MockAction::Crash => return Err(LspError::Transport("mock crash".into())),
        MockAction::Ignore => {}
    }
    Ok(())
}

async fn write_response(
    stream: &mut DuplexStream,
    id: &Value,
    result: Value,
) -> Result<(), LspError> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let bytes = serde_json::to_vec(&body).map_err(LspError::Json)?;
    stream
        .write_all(&encode_message(&bytes))
        .await
        .map_err(|e| LspError::Transport(e.to_string()))
}

async fn write_error(
    stream: &mut DuplexStream,
    id: &Value,
    code: i32,
    message: String,
) -> Result<(), LspError> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    let bytes = serde_json::to_vec(&body).map_err(LspError::Json)?;
    stream
        .write_all(&encode_message(&bytes))
        .await
        .map_err(|e| LspError::Transport(e.to_string()))
}

async fn write_notify(
    stream: &mut DuplexStream,
    method: &str,
    params: Value,
) -> Result<(), LspError> {
    let body = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params });
    let bytes = serde_json::to_vec(&body).map_err(LspError::Json)?;
    stream
        .write_all(&encode_message(&bytes))
        .await
        .map_err(|e| LspError::Transport(e.to_string()))
}

/// 构造一个最小可用 descriptor（stdio、restart 开启、预算 5）。
pub fn test_descriptor(id: &str) -> LanguageServerDescriptor {
    let mut d = LanguageServerDescriptor::new(id, "mock-server", "rust");
    d.startup_timeout = std::time::Duration::from_secs(2);
    d.shutdown_timeout = std::time::Duration::from_secs(2);
    d.restart_on_crash = true;
    d.max_restarts = 5;
    d
}

/// 构造 handler：按 method 路由到固定动作。
pub fn route_handler(routes: Vec<(&'static str, MockAction)>) -> MockHandler {
    use std::collections::HashMap;
    let map: HashMap<&'static str, MockAction> = routes.into_iter().collect();
    Arc::new(move |method, _params, _has_id| {
        map.get(method)
            .cloned()
            .unwrap_or(MockAction::Respond(Value::Null))
    })
}
