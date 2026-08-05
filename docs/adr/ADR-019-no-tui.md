# ADR-019：不实现 TUI

> 注：CLI 已从「无界面辅助测试工具」升级为 Core 的唯一正式宿主（[ADR-021](ADR-021-cli-core-same-process.md)、[ADR-025](ADR-025-cli-is-sole-host.md)），但本决策「不实现全屏 TUI」仍然成立。

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

TUI 维护成本高、与桌面 GUI 目标重叠，且非本项目首要交付。

## 决策

不实现全屏 TUI。CLI（`pawork`）作为 Core 宿主提供一次性命令、服务、交互（`shell`）与系统服务四种运行模式；交互模式只提供普通命令行交互，不实现全屏 TUI。桌面交互体验由 GUI 承担。

## 后果

- 工程聚焦于 Core 与桌面 GUI 契约。
- CLI 仍需覆盖 serve/shell/run/watch/status/shutdown、workspace/session/approval、gui/remote 管理、provider/auth/plugin/mcp/doctor 等关键流程，详见 [CLI Host](../features/cli-host.md)。

## 相关

- [CLI Host](../features/cli-host.md) · [ADR-001 纯 Rust](ADR-001-pure-rust-core.md) · [ADR-021 CLI 与 Core 同进程](ADR-021-cli-core-same-process.md) · [ADR-025 CLI 是唯一宿主](ADR-025-cli-is-sole-host.md) · [ROADMAP P1-12 / Phase 13](../../ROADMAP.md)
