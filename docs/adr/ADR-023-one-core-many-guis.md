# ADR-023：一个 CLI/Core 实例支持多个 GUI

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

单用户常有多窗口、多设备、本地 + 远程并存的查看与控制需求；若一个 Core 只能服务一个 GUI，会迫使运行多份 Core，导致状态分裂。

## 决策

一个 CLI/Core 实例可以同时连接多个本地或远程 GUI。所有 GUI 通过 CLI 接收统一的 Snapshot 与 Event；每个 GUI 拥有独立权限、独立订阅与独立心跳/重连。

## 后果

- 用户可在 CLI 中启动任务，同时在多个 GUI 中查看进度或审批。
- 引入 Connection Manager、Subscription Hub、慢客户端隔离等运行时复杂度。

## 相关

- [gui-connection](../features/gui-connection.md) · [ADR-029 不点对点同步](ADR-029-no-peer-gui-sync.md) · [ADR-030 Core 唯一权威](ADR-030-core-sole-source-of-truth.md)
