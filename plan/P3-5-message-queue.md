# P3-5：消息队列

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P3-1

**最终目的**：实现用户消息排队与 replace queued 语义，保证用户在 Agent 运行中发送的消息不丢失、可替换待处理项。

**涉及范围**：`agent-engine`

## 细分步骤

1. **用户消息入队** —— 目的：运行中可发送。
2. **replace queued 语义** —— 目的：覆盖未处理的待办消息。
3. **队列持久化/不丢** —— 目的：崩溃可恢复。
4. **队列测试** —— 目的：并发发送不丢。

## 主要产出物

- 消息队列

## 验收标准

- [x] 排队不丢消息

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
