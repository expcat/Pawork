# P12-4：Worker 预算 / 模型 / 并发上限

> Phase 12 · Multi-Agent · 状态：🟡未开始 · 依赖：P3-6、P12-1

**最终目的**：为 Worker 设定 token/模型/并发预算，让多 Agent 并发开销可控。

**涉及范围**：`orchestration`

## 细分步骤

1. **worker token/模型预算** —— 目的：成本可控。
2. **并发上限** —— 目的：资源可控。
3. **达预算行为** —— 目的：优雅降级。
4. **测试** —— 目的：预算生效。

## 主要产出物

- worker 预算控制

## 验收标准

- [ ] 并发与预算可控

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
