# ADR-017：GUI 不直接访问 Provider、数据库和工具

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

GUI 直接接触底层会造成契约耦合、安全边界模糊，并难以保证背压、重连与错误转换。

## 决策

GUI 只通过 GUI Connection Protocol 与 app-service 的 Command/Event 交互（经 CLI 进程内的 GUI Server），不直接加载 Core crate、不直接调用 agent-engine、不直接访问数据库、Provider 或工具，不绕过 CLI 访问本地文件系统。

## 后果

- GUI 实现可独立演进；Core 内部重构不影响 GUI。
- 安全与权限集中在 app-service / Policy / Sandbox。
- 大型数据只能经 Artifact ID 获取（[ADR-018](ADR-018-large-payload-artifact-id.md)）。

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [gui-connection](../features/gui-connection.md) · [ADR-006 GUI 经 app-service](ADR-006-tauri-via-app-service.md) · [ADR-018](ADR-018-large-payload-artifact-id.md) · [ADR-022 GUI 经 CLI 连接](ADR-022-gui-connects-via-cli.md)
