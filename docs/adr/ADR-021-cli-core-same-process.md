# ADR-021：Core 与 CLI 运行在同一进程和二进制

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若将 Core Daemon、CLI Client、GUI 作为三个相互独立的服务，会出现额外的 IPC、进程生命周期与状态同步复杂度，并削弱「单一可执行入口」的运维简洁性。

## 决策

Rust Core 与 CLI 是同一个程序和进程边界：`pawork` 二进制同时承载 Core Runtime、CLI 接口、GUI Protocol Server、Local Transport Server 与 Remote Transport Adapter。CLI 自身发起的操作和任一 GUI 发起的操作都进入同一个 Core。

## 后果

- 不存在必须单独部署的 Core Daemon；`pawork` 是 Core 的唯一正式二进制。
- CLI 可完全脱离 GUI 工作；GUI 退出不影响 CLI/Core。
- 已删除原计划中的 `core-daemon` / `core-client` / `core-server` 入口。

## 相关

- [overview](../architecture/overview.md) · [workspace-layout](../architecture/workspace-layout.md) · [ADR-001 纯 Rust Core](ADR-001-pure-rust-core.md) · [ADR-025 CLI 是唯一宿主](ADR-025-cli-is-sole-host.md)
