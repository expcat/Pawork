# P1-4：Event Store

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P1-3

**最终目的**：实现事件 append-only 持久化与按 sequence 重放（ADR-003/016）。这是可重放、崩溃恢复、分支、审计、差分测试的存储基础。

**涉及范围**：`session-store`

## 细分步骤

1. **session_events 表 append** —— 按 session 严格递增 sequence。目的：事件不破坏、有序。
2. **按 sequence 重放接口** —— 目的：可重建状态。
3. **完整性约束** —— sequence 唯一、不跳号。目的：防止事件损坏。
4. **重放测试** —— 目的：可从事件重建投影。

## 主要产出物

- Event Store append + 重放接口

## 验收标准

- [x] 事件不破坏、可按 sequence 重放
- [x] sequence 唯一不跳号

**相关文档**：[sessions](../docs/features/sessions.md) · [ADR-003](../docs/adr/ADR-003-sqlite-event-store.md) · [ADR-016](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)
