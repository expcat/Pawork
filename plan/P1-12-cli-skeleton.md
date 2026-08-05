# P1-12：CLI Host 骨架（pawork）

> Phase 1 · 基础设施 · 状态：🟡未开始 · 依赖：P0-8

**最终目的**：搭建 `pawork` 主二进制骨架，作为 Core 的唯一正式宿主入口（CLI 与 Core 同进程，[ADR-021](../docs/adr/ADR-021-cli-core-same-process.md)/[025](../docs/adr/ADR-025-cli-is-sole-host.md)）。提供 serve/run/shell/watch/status 子命令骨架与 `doctor` 自检，为开发、自动化与无头集成测试提供入口。完整 GUI Connection Protocol Server 留待 [Phase 13](../ROADMAP.md)。

**涉及范围**：`apps/pawork`、`cli-host`、`cli-command`、`cli-renderer`

## 细分步骤

1. **主二进制与子命令骨架** —— serve/shell/run/watch/status/shutdown + workspace/session/run/approval/provider/auth/plugin/mcp/doctor。目的：覆盖 CLI Host 关键流程入口。
2. **CLI Host 装配接口** —— `cli-host` 占位：初始化 Core、装配 app-service 与 Command Router。目的：CLI 与 Core 同进程的装配点。
3. **接入 app-service** —— CLI 命令直接调用 app-service，不绕回自身建立 IPC。目的：复用 Core API，CLI/GUI 行为一致。
4. **doctor 自检命令** —— 环境与依赖诊断。目的：可排查。

## 主要产出物

- `apps/pawork` 骨架与子命令
- `cli-host` / `cli-command` / `cli-renderer` 占位

## 验收标准

- [ ] `pawork serve` 与 `pawork doctor` 可跑
- [ ] CLI 命令经 app-service 访问 Core，不建立自环 IPC

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [总体架构](../docs/architecture/overview.md) · [ADR-019 不实现 TUI](../docs/adr/ADR-019-no-tui.md) · [ADR-021 CLI 与 Core 同进程](../docs/adr/ADR-021-cli-core-same-process.md) · [ADR-025 CLI 是唯一宿主](../docs/adr/ADR-025-cli-is-sole-host.md) · [ROADMAP](../ROADMAP.md)
