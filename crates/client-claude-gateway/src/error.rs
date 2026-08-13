//! 网关适配错误：所有错误一律 fail-closed。
//!
//! 红线：错误消息只允许脱敏上下文（静态标签 / 结构名），禁止携带 signed
//! thinking 材料（`signature` / `data`）、身份头原文或上游敏感载荷。

use client_adapter_api::AdapterError;
use provider_api::ProviderError;
use thiserror::Error;

/// Claude Gateway 适配错误。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ClaudeGatewayError {
    /// 缺少必需身份头。
    #[error("missing identity header `{0}`")]
    MissingIdentityHeader(&'static str),

    /// 身份头重复出现（含大小写不同的重复）。
    #[error("duplicate identity header `{0}`")]
    DuplicateIdentityHeader(&'static str),

    /// 身份头值格式非法（空白 / 控制字符 / 超长）。
    #[error("malformed identity header `{0}`")]
    MalformedIdentityHeader(&'static str),

    /// Agent 树结构非法（parent 无 agent、agent 自引用等）。
    #[error("invalid agent identity tree: {0}")]
    InvalidAgentTree(&'static str),

    /// 缺少受信租户上下文（tenant binding fail-closed）。
    #[error("tenant binding requires a trusted tenant context: {0}")]
    MissingTenantContext(&'static str),

    /// SSE 帧解析失败。
    #[error("malformed SSE frame: {0}")]
    MalformedSse(String),

    /// 事件 JSON 非法或缺失必需字段。
    #[error("malformed claude event `{0}`: {1}")]
    MalformedEvent(String, &'static str),

    /// 不支持的事件 / 子类型：显式失败，不静默丢弃。
    #[error("unsupported claude event `{0}`: {1}")]
    UnsupportedEvent(String, &'static str),

    /// signed thinking 材料缺失必需字段或形状不符。
    #[error("malformed signed thinking block: {0}")]
    MalformedSignedThinking(&'static str),

    /// signed thinking 材料在 `reasoning.signed_continuity` 能力未协商时到达。
    ///
    /// 显式失败而非静默丢弃 / 明文落库；错误只携带静态能力名，不携带材料。
    #[error("signed thinking received but capability `{0}` was not negotiated")]
    SignedThinkingNotNegotiated(&'static str),

    /// signed thinking 保护器不可用（能力未协商 / 未注入）。
    #[error("signed thinking protector unavailable: {0}")]
    SignedThinkingProtectorUnavailable(&'static str),

    /// 上游流错误（脱敏转发）。
    #[error("upstream stream error: {0}")]
    UpstreamStream(#[from] ProviderError),
}

impl From<ClaudeGatewayError> for AdapterError {
    fn from(error: ClaudeGatewayError) -> Self {
        AdapterError::InvalidFrame(error.to_string())
    }
}
