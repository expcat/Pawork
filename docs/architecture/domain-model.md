# 核心领域模型

本文定义 Pawork 的领域类型基线，落地于 `agent-domain`。这些类型是跨 crate 的共享词汇，必须保持零外部 IO 依赖。

## 1. 依赖约束

`agent-domain` 不得依赖 Tauri、SQLite、HTTP Client、OS Keychain、Git、任何具体 Provider。详见 [workspace 结构 §6](workspace-layout.md) 与 [ADR-002](../adr/ADR-002-agent-engine-provider-decoupled.md)。

## 2. 统一消息模型

```rust
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

pub struct Message {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: Vec<ContentPart>,
    pub metadata: MessageMetadata,
}

pub enum ContentPart {
    Text(TextContent),
    Image(ImageContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCallContent),
    ToolResult(ToolResultContent),
    ArtifactRef(ArtifactReference),
}
```

必须支持：普通文本；图片；Provider reasoning/thinking；工具调用；工具结果；大型内容引用；Provider 原始 metadata；Token 和费用信息；完成原因；不完整流式消息。

## 3. Agent Run 状态机

```text
Created
   ↓
PreparingContext
   ↓
WaitingForProvider
   ↓
StreamingResponse
   ↓
CollectingToolCalls
   ↓
WaitingForApproval
   ↓
ExecutingTools
   ↓
AppendingToolResults
   ↓
WaitingForProvider
   ↓
Completed / Cancelled / Failed / Interrupted
```

要求：

- 所有状态转换都产生持久化事件
- 每个 Run 有唯一 ID
- 支持取消
- 支持崩溃恢复
- 支持 Provider 断流重试
- 支持多个 Tool Call
- 支持用户消息排队
- 支持人工审批
- 支持 Token、费用和迭代预算
- 支持最大循环次数
- 支持重复工具调用检测

## 4. Agent Event

```rust
pub enum AgentEvent {
    RunStarted,
    ContextPrepared,
    ProviderRequestStarted,
    AssistantTextDelta,
    AssistantThinkingDelta,
    ToolCallStarted,
    ToolCallArgumentsDelta,
    ToolApprovalRequested,
    ToolApprovalResponded,
    ToolExecutionStarted,
    ToolOutputDelta,
    ToolExecutionCompleted,
    MessageCommitted,
    CompactionStarted,
    CompactionCompleted,
    CheckpointCreated,
    CheckpointRolledBack,
    RunCompleted,
    RunCancelled,
    RunFailed,
}
```

> 基线枚举须覆盖 [P0-3](../../plan/P0-3-event-model.md) 冻结清单（Run/Message/ToolCall/ToolResult/Compaction/Cancel/Checkpoint）的全部状态转换；最终枚举以 P0-3 冻结结果为准。

事件需要：

- 全局事件 ID
- Session ID
- Run ID
- 严格递增 sequence
- 时间戳
- 可选 parent event
- 可序列化
- 可重放
- schema version

事件持久化与重放约束见 [ADR-016](../adr/ADR-016-core-event-persist-replay.md) 与 [sessions](../features/sessions.md)。

## 5. Phase 15–18 扩展领域类型（登记在册）

以下类型是 Phase 15–18 引入的 canonical 领域词汇，按职责落在 `agent-domain` 或纯 API crate（如 `provider-api` / `client-adapter-api`），均不携带外部 IO 实现；运行时语义见对应 feature / plan。新增类型不得按 Provider 名称分支（ADR-002），敏感制品只存安全引用（ADR-032）。

**Provider Native（Phase 15）**

- `ToolKind { ClientFunction, ProviderHosted, ProviderExtension }` + `ExecutionOwner { Core, Provider, Extension }`（一一对应）+ `ContinuationMode { CoreSuppliedResult, ProviderTranscript }`
- `HostedToolRequest`（声明启用 server tools，不含 Provider 名）
- `ServerToolEvent`（生命周期：Started/Progress/Completed、CitationAdded/SourceAdded、ComputerActionRequested/Screenshot、ProgramStarted/Output）
- `Citation` / `Source`（三家引用归一）
- `ReasoningItem { id, summary?, protected_blob_ref, opaque_metadata, continuation_metadata }`（安全引用；原文在 Protected Blob Store）
- `ReasoningEffort { None, Low, Medium, High, XHigh, Max }` + `ReasoningConfig`（canonical effort，经 P15-8 协商）
- `EmbeddingRequest` / `EmbeddingResponse` / `EmbeddingModelDefinition` / `EmbeddingCapabilities`（canonical embedding，落 `provider-api`）

**Modern Agent Workflow（Phase 16）**

- `Plan` / `PlanStep` / `PlanReview`（Plan 模式与评审）
- `Goal` / `SuccessCriterion`（durable objective）
- `BackgroundTask` / `Automation` / `Monitor`（后台任务、调度自动化、监视循环）
- `Memory`（跨会话长期记忆条目）
- `ReviewFinding` / `SuggestedPatch` / `PRComment` / `Resolution`（行锚点评审）

**Ecosystem & Host（Phase 17）**

- `HookHandler { Command, Http, PromptTransform, PromptEval, AgentEval, McpTool }` + trigger vocabulary（Session/Run/Prompt/Tool/Permission/Subagent/Task/Compact/Notification）
- `PluginPackage`（聚合 Skills/Agents/Hooks/MCP/LSP/Monitors）
- `LanguageServerDescriptor`（LSP Client Runtime 描述符）
- `AgentProfileV2`（prompt/model/effort/tools(denied)/skills/mcp/permissions/hooks/memory/max-turns/background/isolation；effort 为 canonical 一等字段）
- `AgentTeam`（peer messaging / shared task board）
- `BrowserComputerCapability`（Local/MCP/ProviderHosted 三执行位点 facade）

**Account Control Plane & Client Adapters（Phase 18）**

- `TenantId` / `PrincipalId` / `TenantPolicy`（legacy 默认 `local/default` / `local/user`）
- `ProviderAccount` / `CredentialMetadata { secret_ref, expires_at, refresh_state }`（禁止 plaintext secret）
- `AcquireRequest { tenant_id, session_id, agent_id?, provider_id, model_id, required_capabilities }`
- `CredentialLease` / `LeaseOutcome { Success(UsageRecord), Failure(ClassifiedFailure), Cancelled }`
- `HealthState` / `ClassifiedFailure { class, scope, retryable, retry_after, health_impact, safe_to_failover }`
- `RouteContext` / `RouteCandidate` / `RouteDecision` / `SessionBinding { ownership_epoch, revision, capability_hash }`
- `UsageRecord`（tenant/principal/account/credential/session/agent/provider/model/trace 多维归属）
- `CanonicalClientEvent` / `CanonicalCoreEvent` / `ClientCapabilities` / `ClientCapabilitySnapshot`
- `ExternalAgentIdentity { session_id?, agent_id?, parent_agent_id? }`
- `AuditEventV1`（actor/action/target/decision/trace/tenant；只含脱敏 allowlist）

上述持久化类型与 canonical event 必须带 schema/event version。Provider、Account、Agent、Session 与 Client Protocol 的状态机独立推进，只通过显式 ID、事件和 lease 交互。

## 6. 相关文档
- [控制流](control-flow.md)
- [sessions](../features/sessions.md)
- [agent-engine](../features/agent-engine.md)
- [providers](../features/providers.md) · [provider-control-plane](../features/provider-control-plane.md) · [client-adapters](../features/client-adapters.md) · [tenant-audit](../features/tenant-audit.md)
- [tools](../features/tools.md) · [plugins](../features/plugins.md)
- [ADR-002 Agent Engine 与 Provider 解耦](../adr/ADR-002-agent-engine-provider-decoupled.md)
- [ADR-016 事件持久化重放](../adr/ADR-016-core-event-persist-replay.md)
- [ADR-032 Protected Blob Store](../adr/ADR-032-protected-blob-store.md)
- [ADR-033 Provider、Account、Agent 与 Client Protocol 控制面分离](../adr/ADR-033-control-plane-separation.md)
- [ROADMAP Phase 15–18](../../ROADMAP.md)
