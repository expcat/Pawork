# ADR-006：GUI 经 app-service 访问 Core

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若 GUI 直接调用 agent-engine、数据库、Provider 或工具，会把内部实现耦合进稳定契约，且难以保证重连、背压与错误转换的一致性。

## 决策

GUI 作为独立进程，经 GUI Connection Protocol 连接 CLI 进程内的 GUI Server，再由 GUI Server 调用 `app-service`；CLI 命令也直接调用同一个 `app-service`。由 `app-service` 提供类型化 Command/Event、状态聚合、事件限流、任务监督与错误转换。Rust 类型（`core-api` / `gui-protocol`）是唯一 schema source；Rust 客户端直接消费这些类型，TypeScript 声明继续为非 Rust 客户端与契约工具自动生成。两条路径共享同一个 app-service 与 Event Hub（[ADR-024](ADR-024-shared-app-service-event-hub.md)）。

## 后果

- Core 内部可自由演进而不破坏 GUI 契约。
- GUI 断开不影响 Run；重连可获取 Snapshot。
- app-service 成为唯一稳定边界，需严格版本化。

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [gui-connection](../features/gui-connection.md) · [ADR-017 GUI 不直接访问](ADR-017-gui-no-direct-access.md) · [ADR-022 GUI 经 CLI 连接](ADR-022-gui-connects-via-cli.md) · [ADR-024 共享 app-service 与 Event Hub](ADR-024-shared-app-service-event-hub.md) · [ROADMAP Phase 13](../../ROADMAP.md)
