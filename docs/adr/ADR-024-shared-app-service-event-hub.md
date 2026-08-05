# ADR-024：CLI 与 GUI 共享 App Service 和 Event Hub

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若 CLI 与 GUI 走不同的业务路径，会出现行为分叉：CLI 显示的状态与 GUI 不一致，GUI 发起的操作在 CLI 看不到。

## 决策

CLI 命令与 GUI Command 进入同一个 Command Router，由同一个 `app-service` 执行；产生的 Core Event 进入同一个 Event Hub，以相同顺序扇出到 CLI 渲染器与所有 GUI。每条命令记录 CommandSource 与身份。

## 后果

- CLI 显示的状态和 GUI 相同；CLI 不会错过 GUI 发起的操作，`pawork watch` 可显示所有客户端活动。
- 日志、GUI 与 CLI 使用同一事件顺序。

## 相关

- [GUI Connection Protocol](../architecture/api-surface.md) · [control-flow](../architecture/control-flow.md) · [ADR-006 GUI 经 app-service](ADR-006-tauri-via-app-service.md)
