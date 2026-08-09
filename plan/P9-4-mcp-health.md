# P9-4：Health / restart / cancel / logging

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P9-1

**最终目的**：实现 MCP server 健康监控、重启、取消与日志，保证 server 崩溃不影响 Agent Core（故障隔离）。

**涉及范围**：`mcp-client`

## 细分步骤

1. **健康检查与重启** —— 目的：自愈。
2. **取消传播** —— 目的：可中断调用。
3. **日志归一** —— 目的：可观测。
4. **故障隔离测试** —— 目的：崩溃不影响 core。

## 主要产出物

- MCP 健康监控与故障隔离

## 验收标准

- [x] server 崩溃不影响 Agent Core

## 验证记录（2026-08-09）

- 独立 manager 提供 health snapshot、握手 / 调用 timeout、有界 restart、主动 shutdown 与 MCP cancellation；工具调用不自动重放，避免副作用重复执行。
- 故障、重启预算耗尽、timeout、cancel 与 shutdown 均返回 typed error，无 panic；Secret-bearing stderr 使用有界整体脱敏。
- GLM 审查后补充：Agent cancellation 可中断 lifecycle gate、OAuth 检查、重连退避与握手；连接期间 health snapshot 不被 I/O 锁阻塞。

**相关文档**：[mcp](../docs/features/mcp.md) · [ADR-011](../docs/adr/ADR-011-mcp-first-extension.md) · [ROADMAP](../ROADMAP.md)
