# P13-2：CLI Host 装配与运行模式

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P13-1

**最终目的**：完成 CLI/Core 一体化装配——`core-runtime` 装配完整 Core，`cli-host` 将 Core、CLI 与 GUI Server 装配到同一进程，`pawork` 成为 Core 的唯一正式二进制。落地一次性/服务/交互/系统服务四种运行模式、Event Hub 与 CLI Renderer，使 CLI 既能独立运行，也能作为 GUI 宿主（[ADR-021](../docs/adr/ADR-021-cli-core-same-process.md)/[025](../docs/adr/ADR-025-cli-is-sole-host.md)/[026](../docs/adr/ADR-026-gui-disconnect-safe.md)）。

**涉及范围**：`core-runtime`、`cli-host`、`cli-command`、`cli-renderer`、`subscription-hub`、`apps/pawork`

## 细分步骤

1. **core-runtime 装配** —— 初始化 Core、打开数据库与 Artifact Store、启动 Agent Engine、装配各引擎。目的：完整 Core 运行时。
2. **cli-host 一体化装配** —— Core + CLI + GUI Server 同进程。目的：单一二进制宿主。
3. **Event Hub 与 CLI Renderer** —— Core Event 统一扇出到 CLI 渲染器与订阅中心；CLI 实时输出来自 Event Hub。目的：CLI 显示与 GUI 一致。
4. **四种运行模式** —— 一次性（`run`，可 `--serve`）、服务（`serve`）、交互（`shell`）、系统服务（`service install/start/stop`）。目的：覆盖部署形态。
5. **单/多实例与退出策略** —— 默认实例与命名实例；按模式判断退出条件。目的：可靠生命周期。

## 主要产出物

- `core-runtime` / `cli-host` 装配、`pawork` 运行模式、Event Hub + CLI Renderer

## 验收标准

- [ ] `pawork serve` 可独立启动完整 Core
- [ ] CLI 可在无 GUI 时执行 Agent 任务
- [ ] Core 与 CLI 编译为同一个正式二进制
- [ ] CLI 实时输出与 GUI 状态一致
- [ ] GUI 断线/关闭不迫使 CLI/Core 退出

**相关文档**：[CLI Host](../docs/features/cli-host.md) · [总体架构](../docs/architecture/overview.md) · [ADR-021](../docs/adr/ADR-021-cli-core-same-process.md) · [ADR-025](../docs/adr/ADR-025-cli-is-sole-host.md) · [ADR-026](../docs/adr/ADR-026-gui-disconnect-safe.md) · [ROADMAP](../ROADMAP.md)
