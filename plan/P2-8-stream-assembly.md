# P2-8：流式组装

> Phase 2 · 首个真实 Provider · 状态：🟡未开始 · 依赖：P2-5

**最终目的**：将 `ProviderStreamEvent` 组装为领域消息（含 partial 消息表达），让 Agent Engine 拿到的是结构化、可增量呈现的消息。

**涉及范围**：`provider-runtime`、`agent-engine`

## 细分步骤

1. **事件聚合为消息** —— text/tool/thinking 合并。目的：结构化输出。
2. **partial 消息表达** —— 目的：流式中途可渲染。
3. **多 tool call 组装** —— 目的：并行流可还原。
4. **组装测试** —— 目的：覆盖典型流模式。

## 主要产出物

- 流式组装器（事件 → 领域消息）

## 验收标准

- [ ] partial 消息可表达
- [ ] 多 tool call 正确组装

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
