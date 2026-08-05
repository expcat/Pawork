# ADR-025：CLI 是 Core 生命周期的唯一正式宿主

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Core 生命周期（初始化、Provider/Workspace/Session 管理、工具执行、Git/PTY/MCP/插件、状态持久化）需要一个明确的、唯一的宿主承担。

## 决策

`pawork`（CLI）是 Core 生命周期的唯一正式宿主。它负责初始化 Core、打开数据库与 Artifact Store、启动 Agent Engine、管理 Provider/Workspace/Session/Run、执行工具、管理 Git/Diff/PTY/MCP/插件，并持久化所有权威状态。

## 后果

- 所有权威状态由 CLI/Core 持有；GUI 只保存纯 UI 偏好。
- 服务模式、系统服务模式与开机启动均由同一二进制承担。

## 相关

- [CLI Host](../features/cli-host.md) · [overview](../architecture/overview.md) · [ADR-021 CLI 与 Core 同进程](ADR-021-cli-core-same-process.md) · [ADR-030 Core 唯一权威](ADR-030-core-sole-source-of-truth.md)
