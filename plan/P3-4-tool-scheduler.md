# P3-4：Tool Scheduler

> Phase 3 · Agent Loop · 状态：🟡未开始 · 依赖：P0-5

**最终目的**：实现工具调度器（只读并发、写/Shell 串行、同文件串行、Git index 串行、审批暂停），保证工具执行的并发安全与一致性。

**涉及范围**：`tool-runtime`

## 细分步骤

1. **基于 capability 的并发/串行策略** —— 目的：只读并发、写串行。
2. **同文件 / Git index 串行** —— 目的：避免写冲突。
3. **审批暂停点** —— 目的：等待用户审批。
4. **取消传播** —— 目的：可整体取消。

## 主要产出物

- `tool-runtime` 调度器

## 验收标准

- [ ] 只读并发、写/Shell 串行、同文件串行生效
- [ ] 审批可暂停与恢复

**相关文档**：[tools](../docs/features/tools.md) · [policy](../docs/features/policy.md) · [ADR-008 capability](../docs/adr/ADR-008-builtin-tools-capability.md) · [ROADMAP](../ROADMAP.md)
