# ADR-022：GUI 只能通过 CLI 连接 Core

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若 GUI 直接嵌入或链接 Core，会把 GUI 与 Core 实现强耦合，难以独立演进、保证安全边界与重连一致性。

## 决策

GUI 不直接连接 Core，而是连接 CLI 暴露的 GUI Connection Protocol（GUI Server 运行在 CLI 进程内）。GUI 不直接加载 Core crate、不直接访问数据库/Provider/工具、不绕过 CLI 访问本地文件系统。

## 后果

- GUI 与 Core 可独立演进，Core 重构不影响 GUI。
- CLI 命令直接调用 app-service，无需对自身建立 IPC；GUI 经 Protocol → Server → app-service。
- 安全与权限集中在 app-service / Policy / Sandbox。

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [gui-connection](../features/gui-connection.md) · [ADR-006 GUI 经 app-service](ADR-006-tauri-via-app-service.md) · [ADR-017 GUI 不直接访问](ADR-017-gui-no-direct-access.md) · [ADR-027 本地远程同协议](ADR-027-local-remote-same-protocol.md)
