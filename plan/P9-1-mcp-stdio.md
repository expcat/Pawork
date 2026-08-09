# P9-1：stdio Transport

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-6、P4-12

**最终目的**：实现 MCP stdio Transport（启动/通信），作为第一外部扩展机制的本地进程接入方式（ADR-011）。

**涉及范围**：`mcp-client`

## 细分步骤

1. **进程启动与 stdio 通信** —— 目的：本地 MCP server 接入。
2. **消息帧与协议握手** —— 目的：符合 MCP。
3. **错误与断连处理** —— 目的：稳健。
4. **测试** —— 目的：可用。

## 主要产出物

- MCP stdio Transport

## 验收标准

- [x] 可启动并通信

## 验证记录（2026-08-09）

- `TokioChildProcess` 完成命令 / 参数 / Secret env 启动与 `initialize` 握手，断连和握手失败归一为 typed error；统一 manager 测试覆盖 handshake、list、call 与 ping。
- `cargo test -p mcp-client`：48 passed；`cargo clippy -p mcp-client --all-targets -- -D warnings`：通过。

**相关文档**：[mcp](../docs/features/mcp.md) · [ADR-011 MCP 第一](../docs/adr/ADR-011-mcp-first-extension.md) · [ROADMAP](../ROADMAP.md)

**执行所有权约束（后续增强）**：当前实现经 rmcp `TokioChildProcess` 直接 spawn，未声明经 Sandbox Runtime。按「Core-owned 进程统一 Sandbox/Process Runtime 所有权」约束（见 Phase 11 增强 brief §4），Core-owned MCP server 子进程须经 `Sandbox Runtime（P11）→ Process Runtime` 统一路径执行，禁止以 `tokio::process::Command` 绕过；后续增强任务将把 stdio child 纳入统一 sandbox 生命周期（统一 spawn 路径、cleanup 与 guarantee reporting），记为后续 remediation，不修改本任务已完成的 🟢 状态。

**依赖建议（2026-08 review）**：使用官方 rmcp SDK，只用其 transport + codec 层，把协议升级细节隔离在 mcp-client 内；锁定小版本（2.x→3.0 有 breaking，遵循官方迁移指南），跟进 MCP 2026-07-28 规范。
