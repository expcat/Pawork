# ADR-029：GUI 之间不进行点对点状态同步

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若 GUI 之间进行点对点状态同步，会引入复杂的冲突解决与一致性协议，且仍难保证与 Core 权威状态一致。

## 决策

多个 GUI 不进行客户端之间的点对点同步。任何 GUI 的操作必须先提交给 CLI/Core，成功后再由 Core Event 广播给所有 GUI。

## 后果

- 一致性模型简单：单写者（Core），多读者（CLI + GUI）。
- 所有客户端看到相同的 Session Revision；网络重试不会重复创建 Run 或消息。

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [ADR-023 一 Core 多 GUI](ADR-023-one-core-many-guis.md) · [ADR-030 Core 唯一权威](ADR-030-core-sole-source-of-truth.md)
