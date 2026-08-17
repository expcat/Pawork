//! Transport 契约与 stdio 进程实现。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use pawork_protocol::headless::MAX_FRAME_BYTES;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command as TokioCommand};

use crate::error::SdkError;

/// 传输契约：SDK 只依赖本 trait 与 Host 通信；`mock::MockTransport` 与
/// `StdioTransport` 都实现它。测试与下游集成可用 mock 替换真实进程。
///
/// 方法接收 `&self`（内部互斥）：reader 任务与请求发送方共享同一传输，
/// 不会因持锁等待而互相阻塞。
#[async_trait]
pub trait Transport: Send + Sync {
    /// 写出一行帧（自动追加 `\n` 并 flush，保持流式语义）。
    async fn write_line(&self, line: &str) -> Result<(), SdkError>;

    /// 读入一行帧（不含 `\n`）；连接关闭返回 [`SdkError::Closed`]。
    async fn read_line(&self) -> Result<String, SdkError>;

    /// 显式 flush 待写数据。
    async fn flush(&self) -> Result<(), SdkError>;

    /// 关闭连接（等待子进程退出）。
    async fn close(&self) -> Result<(), SdkError>;

    fn is_open(&self) -> bool;
}

/// 启动 `pawork` 的选项。
#[derive(Clone, Debug)]
pub struct PaworkOptions {
    /// 二进制路径（默认 `PAWORK_BIN` 环境变量或 `pawork`）。
    pub binary: PathBuf,
    /// 附加参数；默认 `["headless", "--json-stdio"]`（协议稳定入口）。
    pub args: Vec<String>,
    /// 子进程工作目录。
    pub working_dir: Option<PathBuf>,
    /// 附加环境变量。
    pub env: Vec<(String, String)>,
    /// 单次协议操作（握手、往返）的等待上限。
    pub timeout: Duration,
    /// 握手声明的客户端名称。
    pub client_name: String,
    /// 握手声明的客户端版本。
    pub client_version: String,
    /// 请求的能力；Host 按自身能力筛选后授予。
    pub capabilities: Vec<pawork_protocol::headless::SdkCapability>,
}

impl Default for PaworkOptions {
    fn default() -> Self {
        let binary = std::env::var("PAWORK_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("pawork"));
        Self {
            binary,
            args: vec!["headless".into(), "--json-stdio".into()],
            working_dir: None,
            env: Vec::new(),
            timeout: Duration::from_secs(10),
            client_name: "pawork-sdk".into(),
            client_version: crate::SDK_VERSION.into(),
            capabilities: vec![
                pawork_protocol::headless::SdkCapability::Sessions,
                pawork_protocol::headless::SdkCapability::Runs,
                pawork_protocol::headless::SdkCapability::Streaming,
                pawork_protocol::headless::SdkCapability::CompatImport,
                pawork_protocol::headless::SdkCapability::CompatHistory,
            ],
        }
    }
}

/// 进程 stdio 传输：把 `pawork headless --json-stdio` 的 stdin/stdout
/// 当作 JSONL 管道。
pub struct StdioTransport {
    child: tokio::sync::Mutex<Child>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    reader: tokio::sync::Mutex<BufReader<ChildStdout>>,
    open: AtomicBool,
}

impl StdioTransport {
    pub fn spawn(options: &PaworkOptions) -> Result<Self, SdkError> {
        let mut command = TokioCommand::new(&options.binary);
        command
            .args(&options.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        if let Some(dir) = &options.working_dir {
            command.current_dir(dir);
        }
        for (key, value) in &options.env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|error| SdkError::Spawn(format!("{}: {error}", options.binary.display())))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SdkError::Spawn("stdin not piped".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SdkError::Spawn("stdout not piped".into()))?;
        Ok(Self {
            child: tokio::sync::Mutex::new(child),
            stdin: tokio::sync::Mutex::new(Some(stdin)),
            reader: tokio::sync::Mutex::new(BufReader::new(stdout)),
            open: AtomicBool::new(true),
        })
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn write_line(&self, line: &str) -> Result<(), SdkError> {
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| SdkError::Closed("stdin already closed".into()))?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn read_line(&self) -> Result<String, SdkError> {
        // 按字节 `\n` 切行并剥 `\r`（V1 StdioTransport 的 read_line 语义）。
        // 不用会把 U+2028 LINE SEPARATOR 当行界的 readline；JSONL 载荷里的
        // U+2028 必须原样保留在同一帧内。
        let mut reader = self.reader.lock().await;
        let mut bytes = Vec::new();
        let read = reader.read_until(b'\n', &mut bytes).await?;
        if read == 0 {
            return Err(SdkError::Closed("host closed the stream".into()));
        }
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(SdkError::MalformedFrame(format!(
                "line exceeds MAX_FRAME_BYTES ({MAX_FRAME_BYTES})"
            )));
        }
        let mut line = String::from_utf8(bytes)
            .map_err(|error| SdkError::MalformedFrame(format!("non-UTF8 line: {error}")))?;
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    }

    async fn flush(&self) -> Result<(), SdkError> {
        if let Some(stdin) = self.stdin.lock().await.as_mut() {
            stdin.flush().await?;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), SdkError> {
        self.stdin.lock().await.take();
        self.open.store(false, Ordering::SeqCst);
        let _ = self.child.lock().await.wait().await;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }
}
