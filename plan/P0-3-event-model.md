# P0-3：事件模型

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成 · 依赖：P0-2

**最终目的**：建立可持久化、可重放的事件模型（ADR-016）。它是崩溃恢复、分支、压缩、审计与差分测试的共同前提——没有事件链就没有重放。

**涉及范围**：`agent-domain`、`agent-events`

## 细分步骤

1. **定义全局事件 ID 与 sequence** —— 每事件含严格递增 sequence、时间戳、可选 parent event。目的：建立全局有序因果链。
2. **定义 AgentEvent 枚举** —— 涵盖 Run/Message/ToolCall/ToolResult/Compaction/Cancel/Checkpoint 等状态转换。目的：所有状态转换都有对应事件。
3. **定义 schema version** —— 每条事件携带版本，向前兼容。目的：演进不破坏旧数据。
4. **序列化与往返测试** —— JSON 序列化 + 往返断言。目的：事件可落库、可无损重放。

## 主要产出物

- `agent-events` crate：`AgentEvent` + 序列化 + 往返测试

## 验收标准

- [x] 事件可序列化、sequence 严格递增
- [x] 含 schema version
- [x] 序列化往返无损

**相关文档**：[领域模型](../docs/architecture/domain-model.md) · [ADR-003 Event Store](../docs/adr/ADR-003-sqlite-event-store.md) · [ADR-016](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)
