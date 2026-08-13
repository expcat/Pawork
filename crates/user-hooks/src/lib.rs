//! Pawork User Hooks（P17-1：用户声明式事件钩子）。
//!
//! 为用户提供声明式（配置驱动）的事件钩子系统：按 trigger point 把 Agent / Run
//! 生命周期事件桥接到六类 handler——`Command`（外部命令）、`Http`（webhook）、
//! `PromptTransform`（改写 prompt）、`PromptEval`（模型判定）、`AgentEval`（受限
//! Agent 判定）、`McpTool`（MCP tool 作为 handler），并区分同步阻断与 async
//! fire-and-forget。
//!
//! ## 信任边界与执行所有权
//! - 本 crate 与 P10-3 WASM lifecycle hook（`hook-runtime`）**互不调用**：二者
//!   **共享同一组 canonical trigger point 词汇**（`plugin_api::PluginLifecycleEventKind`）
//!   但走独立 dispatcher、独立运行时、独立信任边界；P17 专有点为扩展。
//! - 所有外部执行经依赖注入的执行器 trait；Command handler 强制经注入的
//!   `CommandExecutor`（app-service 接 Sandbox Runtime → Process Runtime），本
//!   crate 内不直接 spawn 进程。
//! - Http / Provider（PromptEval / AgentEval）/ MCP 均依赖注入，且不按 Provider
//!   名分支（统一走 canonical 接口）。
//! - Secret 只存引用，运行时解析、用后清零（zeroize）；审计 / 日志全程 redaction。
//!
//! ## 接线（app-service / apps/pawork）
//! 本 crate 不依赖 policy-engine / http-runtime / process-runtime / sandbox-runtime
//! / provider-api / mcp-client；这些由消费层（app-service 的 `user_hook` 模块）
//! 注入 [`exec::Executors`] 的实现，并把 canonical `AgentEvent` →
//! [`trigger::TriggerPoint`] 的映射、以及把本 crate 接入 agent run loop 的
//! pre-prompt / pre-tool 权威回灌位点（`agent-engine::LoopContext` 扩展钩子）
//! 落地。正式 `pawork` 宿主在 `apps/pawork` 构造最小宿主并加载配置。

pub mod audit;
pub mod capability;
pub mod config;
pub mod dispatch;
pub mod error;
pub mod exec;
pub mod handler;
pub mod registry;
pub mod secret;
pub mod trigger;

pub use audit::{
    DispatchOutcome, HookEffect, PromptTransformDiff, UserHookEvent, UserHookEventPayload,
    USER_HOOK_EVENT_SCHEMA_VERSION,
};
pub use capability::HookCapability;
pub use config::{
    AgentEvalHandler, BudgetLimit, CommandHandler, EvalFallback, HandlerConfig, HandlerLifecycle,
    HookConfig, HookScope, HttpHandler, McpFallback, McpToolHandler, PromptEvalHandler,
    PromptTarget, PromptTransformHandler,
};
pub use dispatch::HookDispatcher;
pub use error::{HookError, HookStatus};
pub use exec::{
    AsyncRunner, AuditSink, CommandExecutor, CommandRequest, CommandResult, Executors,
    ExecutorsBuilder, HookClock, HttpExecutor, JudgeDecision, JudgeMode, JudgeRequest,
    McpToolInvoker, McpToolRequest, McpToolResult, PolicyAction, PolicyGate, PolicyOutcome,
    ProviderJudge, SecretResolver, SystemHookClock, TransformRequest, TransformResult,
    WebhookRequest, WebhookResult,
};
pub use handler::{HookHandler, HookId};
pub use registry::TriggerRegistry;
pub use secret::{
    redact, redact_url, redact_value, SecretRef, SecretString, SecretValue, REDACTED,
};
pub use trigger::{TriggerPayload, TriggerPayloadBuilder, TriggerPoint};

#[cfg(test)]
mod smoke {
    use super::*;

    #[test]
    fn trigger_vocabulary_covers_required_points() {
        // user hook trigger 词汇（17 项；与 P10-3 共享 canonical 子集 + P17 扩展）。
        assert_eq!(TriggerPoint::ALL.len(), 17);
        assert!(TriggerPoint::ALL.contains(&TriggerPoint::SessionStart));
        assert!(TriggerPoint::ALL.contains(&TriggerPoint::Notification));
    }
}
