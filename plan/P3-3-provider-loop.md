# P3-3：Provider Loop

> Phase 3 · Agent Loop · 状态：🟡未开始 · 依赖：P2-8、P3-1

**最终目的**：跑通 Agent 的 Provider Loop——流式提交请求、解析 tool call、提交消息，支持多 tool call 多轮，这是 Agent 循环的主干。

**涉及范围**：`agent-engine`

## 细分步骤

1. **流式提交与消费** —— 目的：实时增量。
2. **解析 tool call 并触发调度** —— 目的：工具交互闭环。
3. **工具结果回填消息并继续** —— 目的：多轮循环。
4. **多 tool call 处理** —— 目的：并行/串行工具。

## 主要产出物

- Provider Loop 实现

## 验收标准

- [ ] Mock Provider 可完成多轮工具循环

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [控制流](../docs/architecture/control-flow.md) · [ROADMAP](../ROADMAP.md)
