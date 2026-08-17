//! `pawork acp serve`：ACP JSON-RPC stdio 入口。

use std::sync::Arc;
use std::time::Duration;

use pawork_app::AppCore;
use pawork_channels::acp::wire::{ERROR_PARSE, JsonRpcError};
use pawork_channels::acp::OutboxItem;
use pawork_channels::{AcpHost, JsonRpcMessage};
use pawork_protocol::adapter::SessionRegistry;
use pawork_session::SqliteClientSessionRegistryStore;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, stdin, stdout};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::adapter::CliAcpCommandHost;
use crate::CliError;

const ACP_PUMP_INTERVAL: Duration = Duration::from_millis(5);
const ACP_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const ACP_INFLIGHT_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn run_acp_serve(core: AppCore) -> Result<(), CliError> {
    let store = core
        .store()
        .map_err(|error| CliError::Usage(error.to_string()))?
        .clone();
    let adapter = Arc::new(crate::adapter::adapter_with_gui_approvals(core));
    let backend = Arc::new(SqliteClientSessionRegistryStore::new(store));
    let registry = Arc::new(
        SessionRegistry::new(backend)
            .await
            .map_err(|error| CliError::Usage(error.to_string()))?,
    );
    let host = Arc::new(AcpHost::new(
        Arc::new(CliAcpCommandHost::new(adapter)),
        registry,
    ));
    run_loop(host, BufReader::new(stdin()), stdout()).await
}

async fn run_loop<R, W>(
    host: Arc<AcpHost>,
    reader: R,
    writer: W,
) -> Result<(), CliError>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let writer = Arc::new(Mutex::new(writer));
    let pump_host = Arc::clone(&host);
    let pump_writer = Arc::clone(&writer);
    let pump = tokio::spawn(async move {
        let mut subscription = pump_host.subscribe();
        loop {
            let mut batch = Vec::new();
            match subscription.recv().await {
                Ok(event) => batch.push(event),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    eprintln!("acp: event subscriber lagged; fail-closed");
                    pump_host.fail_closed_all_prompts("event subscription lagged");
                    break;
                }
            }
            while let Ok(event) = subscription.try_recv() {
                batch.push(event);
            }
            pump_host.pump_events(batch).await;
            if flush_outbox(&pump_host, &pump_writer).await.is_err() {
                break;
            }
        }
    });

    let (result, inflight) = run_frame_loop(Arc::clone(&host), Arc::clone(&writer), reader).await;
    let deadline = tokio::time::Instant::now() + ACP_DRAIN_TIMEOUT;
    while host.has_active_runs() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(ACP_PUMP_INTERVAL).await;
        host.drain_and_pump().await;
        let _ = flush_outbox(&host, &writer).await;
    }
    pump.abort();
    if flush_outbox(&host, &writer).await.is_err() {
        host.resolve_queued_prompts();
    }
    join_inflight(inflight).await;
    result.map_err(CliError::Io)
}

async fn run_frame_loop<R, W>(
    host: Arc<AcpHost>,
    writer: Arc<Mutex<W>>,
    reader: R,
) -> (std::io::Result<()>, Vec<JoinHandle<()>>)
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
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
                    let id = value.get("id").cloned().unwrap_or(Value::Null);
                    if let Err(error) = write_error(&writer, id, &error).await {
                        return (Err(error), inflight);
                    }
                    continue;
                }
            },
            Err(_) => {
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
        if flush_outbox(&host, &writer).await.is_err() {
            break;
        }
    }
    (Ok(()), inflight)
}

async fn dispatch_request<W: tokio::io::AsyncWrite + Unpin>(
    host: &AcpHost,
    writer: &Arc<Mutex<W>>,
    id: Value,
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
        match tokio::time::timeout(ACP_INFLIGHT_JOIN_TIMEOUT, task).await {
            Ok(Ok(())) | Ok(Err(_)) => {}
            Err(_) => abort.abort(),
        }
    }
}

async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &Arc<Mutex<W>>,
    frame: Value,
) -> std::io::Result<()> {
    let mut writer = writer.lock().await;
    let mut line = serde_json::to_string(&frame).expect("json-rpc frame serializes");
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

async fn write_error<W: tokio::io::AsyncWrite + Unpin>(
    writer: &Arc<Mutex<W>>,
    id: Value,
    error: &JsonRpcError,
) -> std::io::Result<()> {
    write_frame(
        writer,
        json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    )
    .await
}

async fn flush_outbox<W: tokio::io::AsyncWrite + Unpin>(
    host: &AcpHost,
    writer: &Arc<Mutex<W>>,
) -> std::io::Result<()> {
    let mut items = host.drain_outbox_items().into_iter();
    while let Some(item) = items.next() {
        match item {
            OutboxItem::Frame(frame) => {
                if let Err(error) = write_frame(writer, frame).await {
                    AcpHost::release_drained_barriers(items);
                    return Err(error);
                }
            }
            OutboxItem::FlushBarrier {
                completion,
                resolution,
            } => {
                let _ = completion.send(resolution).await;
            }
        }
    }
    Ok(())
}
