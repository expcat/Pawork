# P3-1：Run 状态机

> Phase 3 · Agent Loop · 状态：🟡未开始 · 依赖：P0-3、P1-4

**最终目的**：实现 Run 的完整状态机（全部状态与转换，每次转换产生持久化事件），让 Agent 循环可观察、可重放、可恢复。

**涉及范围**：`agent-engine`

## 细分步骤

1. **定义全部状态与转换** —— pending/running/waiting_tool/approval/.../completed/failed/cancelled。目的：覆盖 Run 生命周期。
2. **每次转换产生持久化事件** —— 目的：可重放、可审计。
3. **非法转换防御** —— 目的：状态一致。
4. **状态机测试** —— 目的：覆盖合法/非法路径。

## 主要产出物

- Run 状态机 + 事件化转换

## 验收标准

- [ ] 全部状态转换有事件
- [ ] 非法转换被拒绝

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [控制流](../docs/architecture/control-flow.md) · [领域模型](../docs/architecture/domain-model.md) · [ROADMAP](../ROADMAP.md)
