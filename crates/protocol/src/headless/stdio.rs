//! stdin/stdout JSONL 异步运行循环。
//!
//! [`run_loop`] 从 `AsyncBufRead` 逐行读取请求，把 [`HeadlessRequest`] 交给
//! [`Handler`]：`hello` 帧走握手路径（版本协商 + 能力授予），其余帧翻译后
//! 走分发路径；**握手成功前**的非 `hello` 帧以显式 `NotHandshaked` 错误帧
//! 拒绝（协议强制握手先行）。事件帧由 [`Handler::poll_event`] 与请求读取
//! 交错写出。
//!
//! 输出默认每行 flush（保持流式语义）；批量模式下由 [`StdioWriter`] 的待写
//! 上限提供显式背压：待写字节超过 [`StdioWriter::max_pending_bytes`] 时先写
//! `backpressure` 错误帧再终止（fail-closed，不静默丢帧）。

use async_trait::async_trait;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufWriter};

use super::translate::{
    encode_protocol_response, error_frame, parse_request_line, translate_request,
};
use super::wire::{
    HeadlessRequest, HeadlessResponse, HelloRequest, ProtocolErrorKind, TranslatedRequest,
};

/// Host 接线层：接收解析后的请求帧，返回要写出的响应帧。
///
/// - [`Handler::handshake`]：`hello` 帧的握手应答（版本协商 / 能力授予），
///   失败时返回显式错误帧（[`ProtocolErrorKind::IncompatibleApiVersion`] 等）。
/// - [`Handler::handle`]：分发翻译后的 Command / Query / compat 请求。
/// - [`Handler::poll_event`]：事件源出口；无可用事件时必须**尽快返回
///   `None`**（不得长时间阻塞），由运行循环与请求读取交错写出。
#[async_trait]
pub trait Handler: Send {
    /// 握手应答。`hello` 帧由本方法消费，不进入 [`Handler::handle`]。
    async fn handshake(&mut self, hello: HelloRequest) -> HeadlessResponse;

    /// 分发已翻译的请求，返回要写出的响应帧（可多条）。
    async fn handle(&mut self, request: TranslatedRequest) -> Vec<HeadlessResponse>;

    /// 事件源出口：有可用事件帧时返回 `Some(frame)`，否则返回 `None`
    /// （快速返回；内部可做短暂等待避免忙轮询）。
    async fn poll_event(&mut self) -> Option<HeadlessResponse>;
}

/// 运行循环配置。
#[derive(Clone, Copy, Debug)]
pub struct LoopConfig {
    /// 批量模式：不逐行 flush，由写满缓冲或显式 [`StdioWriter::flush`] 触发。
    pub batch_mode: bool,
    /// 单帧载荷上限（默认 [`MAX_FRAME_BYTES`](super::wire::MAX_FRAME_BYTES)）。
    pub max_frame_bytes: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            batch_mode: false,
            max_frame_bytes: super::wire::MAX_FRAME_BYTES,
        }
    }
}

/// 从任意异步输入逐行读取请求，把响应/事件写入异步输出，直到 EOF。
pub async fn run_loop<R, W, H>(
    reader: R,
    writer: W,
    config: LoopConfig,
    handler: &mut H,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    H: Handler,
{
    let mut reader = reader;
    let mut writer = StdioWriter::new(writer, config);
    let mut line = String::new();
    // 握手状态：`hello` 成功（HelloAck）后置 true；握手失败保持 false。
    let mut handshaked = false;
    loop {
        tokio::select! {
            read = read_line(&mut reader, &mut line) => {
                match read? {
                    // EOF：先排空仍挂起的事件，避免 select 竞态把最后一帧丢掉。
                    None => {
                        while let Some(frame) = handler.poll_event().await {
                            writer.write_frame(&frame).await?;
                        }
                        writer.flush().await?;
                        return Ok(());
                    }
                    Some(trimmed) => {
                        if trimmed.is_empty() {
                            continue;
                        }
                        for response in dispatch_line(trimmed, &mut handshaked, handler).await {
                            writer.write_frame(&response).await?;
                        }
                    }
                }
            }
            event = handler.poll_event() => {
                if let Some(frame) = event {
                    writer.write_frame(&frame).await?;
                } else {
                    tokio::task::yield_now().await;
                }
            }
        }
    }
}

/// 单行请求的完整处理：解析 → 握手或分发 → 响应帧列表。
///
/// 握手语义：`hello` 帧由 [`Handler::handshake`] 消费，返回 `hello_ack`
/// 才置 `handshaked = true`（错误帧保持 false）；非 `hello` 帧在握手成功前
/// 一律返回 `NotHandshaked` 显式错误，不进入分发路径。
async fn dispatch_line<H: Handler>(
    line: &str,
    handshaked: &mut bool,
    handler: &mut H,
) -> Vec<HeadlessResponse> {
    match parse_request_line(line) {
        Ok(request) => match request {
            HeadlessRequest::Hello { .. } => {
                let hello = request
                    .as_hello()
                    .expect("Hello variant always converts to HelloRequest");
                let response = handler.handshake(hello).await;
                *handshaked = matches!(response, HeadlessResponse::HelloAck { .. });
                vec![response]
            }
            _ if !*handshaked => vec![HeadlessResponse::Error {
                request_id: request.request_id(),
                kind: ProtocolErrorKind::NotHandshaked,
                message: "request received before a successful hello handshake".into(),
            }],
            _ => match translate_request(&request) {
                Ok(translated) => handler.handle(translated).await,
                // 请求已解析成功时 request_id 是可知的：翻译失败的 error 帧
                // 必须保留它（客户端按 id 关联错误），不因翻译失败而丢失。
                Err(error) => vec![error_frame(request.request_id(), error.kind, error.message)],
            },
        },
        // 解析失败时 request_id 不可知（帧可能根本不是 JSON）：
        // error 帧不带 request_id。
        Err(error) => vec![error_frame(None, error.kind, error.message)],
    }
}

async fn read_line<'a, R: AsyncBufRead + Unpin>(
    reader: &'a mut R,
    line: &'a mut String,
) -> std::io::Result<Option<&'a str>> {
    line.clear();
    let read = reader.read_line(line).await?;
    if read == 0 {
        return Ok(None);
    }
    let trimmed = line.trim_end_matches(['\n', '\r']);
    Ok(Some(trimmed))
}

/// 有界异步输出写入器：统计待写字节，超过上限时报背压错误。
pub struct StdioWriter<W: AsyncWrite + Unpin> {
    inner: BufWriter<W>,
    config: LoopConfig,
    pending_bytes: usize,
}

impl<W: AsyncWrite + Unpin> StdioWriter<W> {
    pub fn new(writer: W, config: LoopConfig) -> Self {
        Self {
            inner: BufWriter::new(writer),
            config,
            pending_bytes: 0,
        }
    }

    /// 待写字节上限（由配置的 `max_frame_bytes` 决定）。
    pub fn max_pending_bytes(&self) -> usize {
        self.config.max_frame_bytes
    }

    /// 当前待写字节数。
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// 写出一帧（追加 `\n`）。批量模式下待写字节（含换行）超过
    /// `max_frame_bytes` 时先写 `backpressure` 错误帧，再以错误终止
    /// （fail-closed）；流式模式下每帧后自动 flush。
    pub async fn write_frame(&mut self, frame: &HeadlessResponse) -> std::io::Result<()> {
        let line = encode_protocol_response(frame).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("encode response: {error}"),
            )
        })?;
        let frame_bytes = line.len() + 1;
        if self.pending_bytes + frame_bytes > self.config.max_frame_bytes {
            let message = format!(
                "output backpressure: pending {} bytes plus frame {} bytes exceeds limit {}",
                self.pending_bytes, frame_bytes, self.config.max_frame_bytes
            );
            let backpressure = error_frame(None, backpressure_kind(), message.clone());
            let error_line = encode_protocol_response(&backpressure).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("encode backpressure frame: {error}"),
                )
            })?;
            // 绕过待写预算写出终止通知帧并 flush：客户端必须在连接关闭前
            // 看到显式 backpressure 错误，而不是只拿到断流。
            self.inner.write_all(error_line.as_bytes()).await?;
            self.inner.write_all(b"\n").await?;
            self.inner.flush().await?;
            return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, message));
        }
        self.pending_bytes += frame_bytes;
        self.inner.write_all(line.as_bytes()).await?;
        self.inner.write_all(b"\n").await?;
        if !self.config.batch_mode {
            self.flush().await?;
        }
        Ok(())
    }

    /// 显式 flush（批量模式的落盘点）。
    pub async fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().await?;
        self.pending_bytes = 0;
        Ok(())
    }
}

/// 事件流背压（Host 订阅者落后）的协议错误类别（测试/接线断言用）。
pub fn backpressure_kind() -> ProtocolErrorKind {
    ProtocolErrorKind::Backpressure
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use tokio::io::AsyncWrite;

    /// 记录全部落盘字节的测试写入端（共享句柄便于在 writer 未归还时检查）。
    #[derive(Clone, Default)]
    struct RecordingSink {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl AsyncWrite for RecordingSink {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn backpressure_fails_closed_with_error_frame_and_intact_pending() {
        let frame = HeadlessResponse::Error {
            request_id: Some("req-1".into()),
            kind: ProtocolErrorKind::Internal,
            message: "m".into(),
        };
        let line = encode_protocol_response(&frame).expect("encode frame");
        // 上限取「帧 + 换行」的边界值：合法边界帧不得被误报背压。
        let config = LoopConfig {
            batch_mode: true,
            max_frame_bytes: line.len() + 1,
        };
        let sink = RecordingSink::default();
        let mut writer = StdioWriter::new(sink.clone(), config);
        writer
            .write_frame(&frame)
            .await
            .expect("boundary frame fits pending budget");
        assert_eq!(writer.pending_bytes(), line.len() + 1);

        let error = writer
            .write_frame(&frame)
            .await
            .expect_err("overflowing frame must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        // 先检查再累加：失败帧不得污染 pending_bytes。
        assert_eq!(writer.pending_bytes(), line.len() + 1);

        // fail-closed 前必须先写出 backpressure 错误帧。
        let written = String::from_utf8(sink.bytes.lock().unwrap().clone()).expect("utf8 output");
        let last_line = written.trim_end().rsplit('\n').next().expect("error line");
        let backpressure: HeadlessResponse =
            serde_json::from_str(last_line).expect("decode backpressure frame");
        assert!(
            matches!(
                backpressure,
                HeadlessResponse::Error {
                    kind: ProtocolErrorKind::Backpressure,
                    request_id: None,
                    ..
                }
            ),
            "expected backpressure error frame, got {backpressure:?}"
        );
    }
}
