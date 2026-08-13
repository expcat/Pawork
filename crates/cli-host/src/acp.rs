//! `acp serve`（P17-7）：ACP（Agent Client Protocol v1）stdio 传输层。
//!
//! 帧循环职责：stdin 逐行读 JSON-RPC → 交给 [`AcpHost`]（协议翻译 → Core
//! 执行）→ stdout 只写协议帧（成功/错误响应、`session/update` 通知、
//! `session/request_permission` 请求）；非协议诊断一律进 stderr，不污染
//! stdout 协议流。事件源是共享 Event Hub（调用方运行 EventPump），与
//! GUI/CLI 其他模式同一条 canonical 事件流，不竞争 `drain_events`。
//!
//! `session/prompt` 会等待 run 终态，因此读循环必须把它放到独立 task：
//! 长 Prompt 期间 stdin 仍能处理 `session/cancel`、`$/cancel_request`
//! 与 `session/request_permission` 响应。其它请求保持串行，握手/建会话
//! 顺序不变。出站仍走单一有序 outbox：先写帧，再释放 prompt 屏障。

//! Hub Lagged 必须可靠 replay 错过的事件；replay 不可用时 fail-closed 解除
//! 全部未决 prompt / 审批，禁止静默丢终态。outbox 半写失败必须释放已 drain
//! 的剩余屏障；join 超时必须 abort 未完成的 prompt task。

use std::sync::Arc;
use std::time::Duration;

use acp_host::wire::ERROR_PARSE;
use acp_host::{AcpHost, JsonRpcError, JsonRpcId, JsonRpcMessage, OutboxItem};
use app_service::AppService;
use client_adapter_api::SessionRegistry;
use serde_json::{json, Value};
use subscription_hub::{EventHub, HubError};
use core_api::GlobalSequence;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// 事件泵轮询间隔（drain Hub → 回译 → 刷出站）。
const ACP_PUMP_INTERVAL: Duration = Duration::from_millis(5);
/// EOF 后等待活跃 run 收敛的窗口（prompt 终态回译不因 stdin 关闭而丢失）。
const ACP_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
/// EOF 冲刷后等待已派发 prompt 写出响应的窗口。
const ACP_INFLIGHT_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// ACP stdio 循环：读 JSON-RPC 帧、派发 [`AcpHost`]、写协议帧。
pub async fn run_loop<R, W>(
    service: Arc<AppService>,
    hub: Arc<EventHub>,
    registry: Arc<SessionRegistry>,
    reader: R,
    writer: W,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let host = Arc::new(AcpHost::with_hub(service, registry, hub));
    let writer = Arc::new(Mutex::new(writer));

    // 事件泵：订阅共享 Hub 的 canonical 事件流，回译为 ACP 出站帧。
    let pump_host = Arc::clone(&host);
    let pump_writer = Arc::clone(&writer);
    let pump = tokio::spawn(async move {
        let mut subscription = pump_host.subscribe();
        let mut last_seen = GlobalSequence(0);
        loop {
            let mut batch = Vec::new();
            match subscription.recv().await {
                Ok(event) => batch.push(event),
                Err(HubError::Closed) => break,
                Err(HubError::Lagged { missed }) => {
                    if !recover_lagged_events(&pump_host, last_seen, missed).await {
                        break;
                    }
                    if flush_outbox(&pump_host, &pump_writer).await.is_err() {
                        break;
                    }
                    continue;
                }
                Err(HubError::Empty) | Err(HubError::ReplayUnavailable { .. }) => {
                    // recv 路径不应出现；防御性 continue 避免空转。
                    tokio::time::sleep(ACP_PUMP_INTERVAL).await;
                    continue;
                }
            }
            while let Ok(event) = subscription.try_recv() {
                batch.push(event);
            }
            if let Some(sequence) = batch.last().map(|event| event.global_sequence) {
                last_seen = sequence;
            }
            pump_host.pump_events(batch).await;
            if flush_outbox(&pump_host, &pump_writer).await.is_err() {
                break;
            }
        }
    });

    let (result, inflight) = run_frame_loop(Arc::clone(&host), Arc::clone(&writer), reader).await;

    // EOF：stdin 关闭后先等活跃 run 收敛（终态回译），最终冲刷一次再停泵。
    let deadline = tokio::time::Instant::now() + ACP_DRAIN_TIMEOUT;
    while host.has_active_runs() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(ACP_PUMP_INTERVAL).await;
    }
    pump.abort();
    if flush_outbox(&host, &writer).await.is_err() {
        host.resolve_queued_prompts();
    }
    join_inflight(inflight).await;
    result
}

/// 帧循环主体：逐行解析并派发（请求/通知/响应），stdout 只写协议帧。
async fn run_frame_loop<R, W>(
    host: Arc<AcpHost>,
    writer: Arc<Mutex<W>>,
    reader: R,
) -> (std::io::Result<()>, Vec<JoinHandle<()>>)
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut lines = reader.lines();
    let mut inflight = Vec::new();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => return (Err(error), inflight),
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let message = match serde_json::from_str::<Value>(&line) {
            Ok(value) => match JsonRpcMessage::parse(value.clone()) {
                Ok(message) => message,
                Err(error) => {
                    // 结构可解析但不符合 JSON-RPC：尽量带 id 回错误帧。
                    let id = value.get("id").cloned().unwrap_or(Value::Null);
                    if let Err(error) = write_error(&writer, id, &error).await {
                        return (Err(error), inflight);
                    }
                    continue;
                }
            },
            Err(_) => {
                // 行不是合法 JSON：按 JSON-RPC 规范回 Parse error（id=null）。
                if let Err(error) = write_error(
                    &writer,
                    Value::Null,
                    &JsonRpcError::new(ERROR_PARSE, "Parse error"),
                )
                .await
                {
                    return (Err(error), inflight);
                }
                continue;
            }
        };
        match message {
            JsonRpcMessage::Request(request) => {
                let id = request.id;
                if request.method == "session/prompt" {
                    // 长 Prompt 不占用读循环：cancel / permission 才能在
                    // run 进行中入站。响应仍等 outbox 屏障释放后写出。
                    let prompt_host = Arc::clone(&host);
                    let prompt_writer = Arc::clone(&writer);
                    inflight.push(tokio::spawn(async move {
                        let _ = dispatch_request(
                            &prompt_host,
                            &prompt_writer,
                            id,
                            request.method,
                            request.params,
                        )
                        .await;
                    }));
                } else if let Err(error) =
                    dispatch_request(&host, &writer, id, request.method, request.params).await
                {
                    return (Err(error), inflight);
                }
            }
            JsonRpcMessage::Notification(notification) => {
                if let Err(error) = host
                    .handle_notification(&notification.method, notification.params)
                    .await
                {
                    // 通知无响应：诊断只进 stderr，不污染 stdout 协议流。
                    eprintln!(
                        "acp: notification `{}` failed: {} (code {})",
                        notification.method, error.message, error.code
                    );
                }
            }
            JsonRpcMessage::Response(response) => {
                if let Err(error) = host.handle_response(response.id, Ok(response.result)).await {
                    eprintln!(
                        "acp: response failed: {} (code {})",
                        error.message, error.code
                    );
                }
            }
            JsonRpcMessage::Error(error_response) => {
                if let Err(error) = host
                    .handle_response(error_response.id, Err(error_response.error))
                    .await
                {
                    eprintln!(
                        "acp: error response failed: {} (code {})",
                        error.message, error.code
                    );
                }
            }
        }
    }
    (Ok(()), inflight)
}

/// 派发一条 client 请求并写出对应响应帧。
async fn dispatch_request<W: AsyncWrite + Unpin>(
    host: &AcpHost,
    writer: &Arc<Mutex<W>>,
    id: JsonRpcId,
    method: String,
    params: Option<Value>,
) -> std::io::Result<()> {
    match host.handle_request(id.clone(), &method, params).await {
        Ok(result) => {
            write_frame(
                writer,
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            )
            .await
        }
        Err(error) => write_error(writer, id, &error).await,
    }
}

async fn join_inflight(inflight: Vec<JoinHandle<()>>) {
    for task in inflight {
        let abort = task.abort_handle();
        let result = tokio::time::timeout(ACP_INFLIGHT_JOIN_TIMEOUT, task).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => {
                eprintln!("acp: inflight prompt task failed: {error}");
            }
            Err(_) => {
                abort.abort();
            }
        }
    }
}

/// 写单帧 JSON-RPC 消息（独占 writer）。
async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &Arc<Mutex<W>>,
    frame: Value,
) -> std::io::Result<()> {
    let mut writer = writer.lock().await;
    let mut line = serde_json::to_string(&frame).expect("json-rpc frame serializes");
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

/// 写 JSON-RPC 错误响应帧。
async fn write_error<W: AsyncWrite + Unpin>(
    writer: &Arc<Mutex<W>>,
    id: JsonRpcId,
    error: &JsonRpcError,
) -> std::io::Result<()> {
    write_frame(
        writer,
        json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    )
    .await
}

/// 冲刷宿主 outbox（单一有序队列）：先按序写出全部帧（`session/update`
/// 通知 / `session/request_permission` 请求 / `$/cancel_request` 通知），
/// 遇到 flush barrier 才释放对应 prompt 的完成信号——因此 `session/prompt`
/// 响应保证在该 prompt 的全部 `session/update` 写出之后才返回。写出失败时
/// 丢弃无法写出的帧并就地释放剩余屏障，等待中的 prompt 不悬挂。
async fn flush_outbox<W: AsyncWrite + Unpin>(
    host: &AcpHost,
    writer: &Arc<Mutex<W>>,
) -> std::io::Result<()> {
    let mut items = host.drain_outbox_items().into_iter();
    while let Some(item) = items.next() {
        match item {
            OutboxItem::Frame(frame) => {
                if let Err(error) = write_frame(writer, frame).await {
                    // 半写失败：当前帧已丢失，后续帧也写不出。剩余屏障必须
                    // 就地释放，不能悬挂已 drain 出队列的 prompt。
                    AcpHost::release_drained_barriers(items);
                    return Err(error);
                }
            }
            OutboxItem::FlushBarrier {
                completion,
                resolution,
            } => {
                // 屏障前帧已全部写出；接收方已 drop 时静默忽略。
                let _ = completion.send(resolution).await;
            }
        }
    }
    Ok(())
}

/// Hub Lagged：按 last_seen 之后的全局序列 replay。replay 不可用时 fail-closed
/// 解除全部未决 prompt / 审批，禁止静默丢终态。
async fn recover_lagged_events(host: &AcpHost, last_seen: GlobalSequence, missed: u64) -> bool {
    match host.replay_missed_events(last_seen).await {
        Ok(_) => true,
        Err(reason) => {
            eprintln!(
                "acp: hub lagged by {missed} events and replay failed ({reason}); fail-closed"
            );
            host.fail_closed_all_prompts(&reason);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    #[test]
    fn join_timeout_aborts_inflight_prompt_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime");
        runtime.block_on(async {
            let finished = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&finished);
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                flag.store(true, Ordering::SeqCst);
            });
            let started = Instant::now();
            join_inflight(vec![handle]).await;
            assert!(
                started.elapsed() < Duration::from_secs(8),
                "join timeout must abort instead of waiting for the 30s task",
            );
            assert!(
                !finished.load(Ordering::SeqCst),
                "aborted task must not run to completion",
            );
        });
    }
}
