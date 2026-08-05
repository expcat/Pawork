# ADR-001：纯 Rust Core，不保留 JavaScript Runtime

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

Pi 当前以 TypeScript 实现模型接口、Agent Runtime、Coding Agent 与 TUI，并通过 Sidecar 与 Electron/Tauri GUI 交互。沿袭该架构会引入 Node/Bun、嵌入 JS Runtime、Sidecar 进程与 `@earendil-works/pi-*` 依赖，带来体积、启动、安全与跨平台复杂度。

## 决策

CLI 与 Rust Core 运行在同一进程、同一二进制（`pawork`）：不使用 Node；不使用 Bun；不嵌入 JavaScript Runtime；不启动 Pi Sidecar；不依赖 `@earendil-works/pi-*`；不实现 TUI。GUI 作为独立进程（后续 Tauri）经 GUI Connection Protocol 连接 CLI/Core，不嵌入 Core。沿用 Pi 的职责拆分，但不沿用其 TypeScript 实现。详见 [ADR-021](ADR-021-cli-core-same-process.md)、[ADR-022](ADR-022-gui-connects-via-cli.md)、[ADR-025](ADR-025-cli-is-sole-host.md)。

## 后果

- 优点：单进程、低内存、快启动、可控安全边界、统一类型系统。
- 代价：放弃 TypeScript 扩展生态，需以 MCP/WASM/声明式资源重建（见 [ADR-011](ADR-011-mcp-first-extension.md)、[ADR-012](ADR-012-wasm-first-plugin.md)）。
- 风险：扩展生态早期为空，缓解见 [ROADMAP 风险监控](../../ROADMAP.md)。

## 相关

- [overview](../architecture/overview.md) · [ADR-013](ADR-013-no-native-dylib-plugin.md) · [ADR-019](ADR-019-no-tui.md) · [ADR-021](ADR-021-cli-core-same-process.md) · [ADR-022](ADR-022-gui-connects-via-cli.md) · [ADR-025](ADR-025-cli-is-sole-host.md)
