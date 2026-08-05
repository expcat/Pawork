# P9-1：stdio Transport

> Phase 9 · MCP · 状态：🟡未开始 · 依赖：P0-6、P4-12

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

- [ ] 可启动并通信

**相关文档**：[mcp](../docs/features/mcp.md) · [ADR-011 MCP 第一](../docs/adr/ADR-011-mcp-first-extension.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：使用官方 rmcp SDK，只用其 transport + codec 层，把协议升级细节隔离在 mcp-client 内；锁定小版本（2.x→3.0 有 breaking，遵循官方迁移指南），跟进 MCP 2026-07-28 规范。
