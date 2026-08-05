# ADR-016：Core Event 必须可持久化和重放

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

为支持崩溃恢复、分支、压缩、审计与差分测试，所有状态转换必须可追溯。

## 决策

所有状态转换产生持久化事件，含全局事件 ID、Session ID、Run ID、严格递增 sequence、时间戳、可选 parent event、schema version，可序列化、可重放。

## 后果

- 崩溃后可恢复 Interrupted Run；可从任意事件 Fork。
- 事件 schema 须版本化与向前兼容。
- Projection 可删后从事件重建。

## 相关

- [领域模型](../architecture/domain-model.md) · [sessions](../features/sessions.md) · [ADR-003 Event Store](ADR-003-sqlite-event-store.md)
