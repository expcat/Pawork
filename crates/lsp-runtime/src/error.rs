//! P17-4 LSP client runtime 错误类型。

use std::time::Duration;

/// LSP 客户端运行时的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    /// `Content-Length` framing 解析失败（非法 / 超大 / 重复 / 缺失 header、EOF 半帧等）。
    #[error("lsp framing error: {0}")]
    Framing(#[from] FrameError),
    /// JSON-RPC 序列化 / 反序列化失败。
    #[error("lsp json error: {0}")]
    Json(#[from] serde_json::Error),
    /// 传输层错误（读 / 写 / 关闭）。
    #[error("lsp transport error: {0}")]
    Transport(String),
    /// 经注入的 Sandbox→Process 路径 spawn 语言服务失败。
    #[error("lsp spawn error: {0}")]
    Spawn(String),
    /// 语言服务返回了 JSON-RPC error 响应。
    #[error("lsp server error ({code}): {message}")]
    ServerError {
        code: i32,
        message: String,
        data: Option<serde_json::Value>,
    },
    /// 请求在给定超时内未收到响应。
    #[error("lsp request `{method}` timed out after {timeout:?}")]
    Timeout { method: String, timeout: Duration },
    /// 请求被取消。
    #[error("lsp request `{method}` cancelled")]
    Cancelled { method: String },
    /// 服务端能力不支持该方法（能力协商未通过）。
    #[error("lsp method `{method}` not supported by server capabilities")]
    Unsupported { method: String },
    /// 客户端当前状态不允许该操作（未初始化 / 已关闭 / 已耗尽重启预算等）。
    #[error("lsp invalid state: {0}")]
    InvalidState(String),
    /// 写操作（rename / code_action）被策略拒绝。
    #[error("lsp write edit denied by policy: {0}")]
    PolicyDenied(String),
    /// 策略放行了写编辑，但未注入 [`crate::write_policy::EditApplier`]：
    /// 不能假成功，必须显式报错。
    #[error("write edit allowed by policy but no edit applier is configured")]
    NoEditApplier,
    /// 大体积结果经 artifact 引用归一化时失败。
    #[error("lsp artifact store error: {0}")]
    Artifact(String),
}

/// `Content-Length` framing 解析错误。所有错误均「有界」——即不是 panic，调用方可决定丢弃 / 重连。
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// header 段缺少必需的 `Content-Length`。
    #[error("missing Content-Length header")]
    MissingContentLength,
    /// 出现多个 `Content-Length` header（协议非法）。
    #[error("duplicate Content-Length header")]
    DuplicateContentLength,
    /// `Content-Length` 值不是合法十进制。
    #[error("invalid Content-Length value: {0}")]
    InvalidContentLength(String),
    /// 声明的帧体超过配置上限（在上限前拒绝，避免大内存分配）。
    #[error("frame length {declared} exceeds max {max}")]
    OversizedFrame { declared: u64, max: u64 },
    /// header 行格式非法（缺 `:`、控制字符、非 ASCII 等）。
    #[error("malformed header line: {0}")]
    MalformedHeader(String),
    /// header 段超过严格上界（恶意 / 损坏流，在分配前拒绝）。
    #[error("header section exceeds max {max} byte(s)")]
    HeaderTooLarge { max: usize },
    /// EOF 时仍存在未完成的 header 或 body（半帧）。
    #[error("unexpected eof with partial frame ({0} byte(s) buffered)")]
    UnexpectedEof(usize),
}

impl FrameError {
    /// 解析器在遇到致命 framing 错误后是否已不可恢复（流已损坏，必须重连）。
    pub fn is_fatal(&self) -> bool {
        true
    }
}

pub type LspResult<T> = std::result::Result<T, LspError>;
