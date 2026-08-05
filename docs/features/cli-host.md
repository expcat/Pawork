# CLI Host（pawork）

## 职责

`pawork` 二进制是 Core 的唯一正式宿主：CLI 与 Rust Core 运行在同一进程、同一二进制。它不只是辅助测试工具，而是产品的核心运行入口，承担 Core 宿主、命令行界面、GUI 连接服务器与生命周期管理（[ADR-019](../adr/ADR-019-no-tui.md)、[ADR-021](../adr/ADR-021-cli-core-same-process.md)、[ADR-025](../adr/ADR-025-cli-is-sole-host.md)）。

CLI 可以在没有 GUI 的情况下独立工作；GUI 退出不会影响 CLI/Core 中正在执行的任务（[ADR-026](../adr/ADR-026-gui-disconnect-safe.md)）。不实现全屏 TUI。

## CLI Host 运行模式

### 一次性命令模式

```bash
pawork run --workspace ./repo --prompt "修复测试"
```

流程：启动 CLI/Core → 执行命令 → 等待任务完成 → 输出结果 → 若无 GUI 连接且无后台任务则退出。可通过 `--serve` 保持服务。

### 服务模式

```bash
pawork serve
```

启动 Core，打开本地 GUI Endpoint，可选启用 Remote Transport，在后台持续运行，等待 CLI 控制或 GUI 连接。仅在用户 shutdown、系统服务停止、系统关机或不可恢复的启动错误时退出。

### 交互模式

```bash
pawork shell
```

提供普通命令行交互（不实现全屏 TUI）。支持 `/run`、`/cancel`、`/sessions`、`/workspaces`、`/approve`、`/status`、`/watch`、`/connect` 等。退出时若无 GUI 与后台任务则退出，否则询问是否转入后台。

### 系统服务模式

```bash
pawork service install
pawork service start
pawork service stop
```

同一二进制可注册为 macOS LaunchAgent / Linux systemd service / Windows Service。

## 命令总览

```bash
pawork serve
pawork shell
pawork run
pawork watch
pawork status
pawork shutdown

pawork workspace list / add
pawork session list / open / export
pawork run cancel
pawork approval list / approve

pawork gui clients / disconnect / endpoint
pawork remote publish / unpublish

pawork provider list
pawork auth login
pawork plugin list
pawork mcp doctor
pawork doctor
```

保留的开发/自动化命令：`models list`、`tools list`、`import-pi`、`benchmark`、`--json` 稳定输出。

## `pawork gui clients`

显示当前连接的 GUI：名称、版本、本地或远程、连接时间、最后心跳、当前订阅、最后确认的事件序列、权限、网络状态。

## `pawork watch`

查看所有客户端活动：CLI 发起的运行、GUI 发起的运行、Tool Call、Tool Approval、文件修改、Session 更新、GUI 连接与断开、Provider 状态。

## 单实例与多实例

默认一个用户配置目录对应一个 CLI/Core 实例（`pawork://default`）。可启动命名实例：

```bash
pawork serve --instance work
pawork serve --instance personal
```

每个实例拥有独立的 Endpoint、数据库、Workspace Catalog、Session、Credential 引用、Artifact Store、日志、PID 与 Instance ID。GUI 可同时连接多个 CLI/Core 实例，但每个窗口或 Workspace Context 必须明确标识当前 Core。

## 退出策略

CLI/Core 不得因为某个 GUI 退出而自动结束正在运行的 Agent。退出条件按运行模式判断：

- 一次性模式：目标命令完成 + 无活跃 Run + 无活跃 Tool + 无 GUI 连接 + 未指定 `--serve`。
- 服务模式：仅用户 shutdown / 系统服务停止 / 系统关机 / 不可恢复启动错误。
- 交互模式：退出 Shell 时若无 GUI 与后台任务则退出，否则可转入后台服务。

## 优先级

P0：CLI 是 Core 的核心运行入口，`apps/pawork` 骨架与 `serve/run/shell/watch` 必须在早期可用（[P1-12](../../plan/P1-12-cli-skeleton.md)）。完整 GUI Connection Protocol Server 在 [Phase 13](../../ROADMAP.md) 落地。

## 验收标准

- [ ] `pawork serve` 可独立启动完整 Core
- [ ] CLI 可在无 GUI 时执行 Agent 任务
- [ ] Core 与 CLI 编译为同一个正式二进制
- [ ] 不存在必须单独部署的 Core Daemon
- [ ] CLI 与 GUI 使用同一 app-service、接收相同顺序的 Core Event
- [ ] GUI 断线/关闭不取消正在运行的 Agent，也不要求 CLI/Core 退出
- [ ] CLI 可列出与管理当前所有 GUI 连接
- [ ] `--json` 输出稳定可解析；基准结果可复现

## 相关文档

- [GUI Connection Protocol](../architecture/api-surface.md) · [GUI 连接与多客户端](gui-connection.md)
- [总体架构](../architecture/overview.md)
- [observability（doctor）](observability.md) · [sessions（export/import）](sessions.md)
- [ADR-019 不实现 TUI](../adr/ADR-019-no-tui.md) · [ADR-021 CLI 与 Core 同进程](../adr/ADR-021-cli-core-same-process.md) · [ADR-025 CLI 是唯一宿主](../adr/ADR-025-cli-is-sole-host.md)
- [ROADMAP P1-12 / Phase 13](../../ROADMAP.md)
