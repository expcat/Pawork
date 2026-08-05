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

## 5. 相关文档

- [控制流](control-flow.md)
- [sessions](../features/sessions.md)
- [agent-engine](../features/agent-engine.md)
- [ADR-002 Agent Engine 与 Provider 解耦](../adr/ADR-002-agent-engine-provider-decoupled.md)
