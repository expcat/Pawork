# ADR-026：GUI 断线不影响 Core 任务

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

若 GUI 断线或关闭会取消正在运行的 Agent 或迫使 Core 退出，远程与多窗口场景下的可靠性无法保证。

## 决策

GUI 断线或退出不影响 CLI/Core 中正在执行的任务。任务的生命周期由 Core 持有，不绑定到任何 GUI 连接。GUI 重连后通过 Event Replay 或 Snapshot 重建恢复完整状态。

## 后果

- CLI 退出策略按运行模式独立判断，不以 GUI 是否在线为唯一依据。
- 需要慢客户端隔离，确保单个 GUI 不阻塞 Core 或其他 GUI。

## 相关

- [gui-connection](../features/gui-connection.md) · [CLI Host 退出策略](../features/cli-host.md) · [ADR-023 一 Core 多 GUI](ADR-023-one-core-many-guis.md)
