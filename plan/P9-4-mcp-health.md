# P9-4：Health / restart / cancel / logging

> Phase 9 · MCP · 状态：🟡未开始 · 依赖：P9-1

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

- [ ] server 崩溃不影响 Agent Core

**相关文档**：[mcp](../docs/features/mcp.md) · [ADR-011](../docs/adr/ADR-011-mcp-first-extension.md) · [ROADMAP](../ROADMAP.md)
