# ADR-030：Core 是所有客户端状态的唯一权威来源

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若 GUI 本地缓存可写的业务权威状态，会出现与 Core 不一致、难以仲裁的冲突。

## 决策

CLI 内部运行的 Core 是所有客户端状态的唯一权威来源（Workspace / Session / Branch / Run / Message / Tool Call / Approval / Git 与 Diff / Terminal / Provider / Plugin / MCP / Artifact）。CLI 输出和所有 GUI 都是该状态的观察者与操作入口；GUI 不保存可写的业务权威状态。

## 后果

- 断线重连只需对齐 `global_sequence`：可重放则补发缺失事件，否则重新发送 Snapshot。
- 所有客户端操作均记录来源与身份，便于审计与显示。

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [gui-connection](../features/gui-connection.md) · [ADR-016 事件持久化可重放](ADR-016-core-event-persist-replay.md) · [ADR-029 不点对点同步](ADR-029-no-peer-gui-sync.md)
