//! # client-claude-gateway — Claude Code Gateway 适配器（P18-12）
//!
//! 职责：把 Claude Code / Claude Agent SDK 的 Anthropic Messages 线协议与
//! `X-Claude-Code-*` 身份头映射到 Pawork canonical 域。Adapter 是 `app-service`
//! 上方的并列 Client Channel，不取代 GUI Connection Protocol，不构造第二个 Core。
//!
//! ## 设计要点
//!
//! - **身份提取**：`X-Claude-Code-Session-Id` / `X-Claude-Code-Agent-Id` /
//!   `X-Claude-Code-Parent-Agent-Id` → [`ExternalAgentIdentity`]，无需解析 body
//!   即可归属 agent cost；缺失 / 重复 / 畸形 / 伪造（parent 无 agent、agent
//!   自引用）一律 fail-closed。header **绝不**作为跨 tenant affinity key。
//! - **Messages streaming 映射**：text / thinking / tool_use / tool_result /
//!   usage / error / cancel → canonical [`ProviderStreamEvent`] 与 SDK 层可观察
//!   事件，保持 Provider/Agent 事件边界。
//! - **权限与生命周期**：permission、subagent start/stop、task、hook 可观察事件
//!   只做显式翻译；最终决策仍由 Core policy，adapter 不接管业务、不持有
//!   Provider credential。
//! - **signed thinking continuity**（ADR-032 / P15-7）：仅经能力协商
//!   （`reasoning.signed_continuity`）与 Protected Blob 引用处理。明文
//!   `signature` / `data` 永不进入 canonical 事件、`Debug`、日志或普通存储；
//!   未协商时显式失败，不静默丢弃、不明文落库。
//!
//! ## 依赖方向
//!
//! 只依赖 `agent-domain` / `agent-events` / `provider-api` /
//! `client-adapter-api` 的 canonical 契约；不依赖 Provider runtime、SQLite、
//! HTTP Client、OS Keychain 或任何具体 Provider。signed thinking 保护经本地
//! [`SignedThinkingProtector`] seam 注入，宿主用 `provider-runtime` 的
//! `ReasoningProtector`（P15-10 统一抽象）桥接。

pub mod adapter;
pub mod control;
pub mod error;
pub mod identity;
pub mod reasoning;
pub mod stream;
pub mod wire;

pub use adapter::{
    capability, ClaudeGatewayAdapter, ClaudeGatewayAdapterFactory, NegotiatedClaudeAdapter,
    CLAUDE_GATEWAY_PROTOCOL, CLAUDE_GATEWAY_PROTOCOL_VERSION, DEFAULT_SUPPORTED_CAPABILITIES,
    REASONING_SIGNED_CONTINUITY_CAPABILITY,
};
pub use control::{ControlEvent, GatewayPermissionDecision};
pub use error::ClaudeGatewayError;
pub use identity::{
    bind_tenant, extract_identity, ClaudeAgentId, ClaudeSessionId, ExternalAgentIdentity,
    HeaderPair, TenantBinding, TrustedTenantContext, HEADER_AGENT_ID, HEADER_PARENT_AGENT_ID,
    HEADER_SESSION_ID, MAX_ID_LENGTH,
};
pub use reasoning::{
    build_reasoning_item, protect_signed_thinking, InMemorySignedThinkingProtector,
    SignedThinkingMaterial, SignedThinkingProtector, ANTHROPIC_BLOCK_KIND_KEY,
};
pub use stream::{map_sse_event, protect_pending_signed, ClaudeStreamState, GatewayEvent};
pub use wire::{
    decode_frame, parse_event, ClaudeContentBlockDelta, ClaudeContentBlockStart, ClaudeStreamEvent,
    SignedThinkingBlock, SseFrame, SseParser, ThinkingBlockKind,
};
