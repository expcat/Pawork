# ADR-011：MCP 是第一外部扩展机制

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

放弃 TypeScript Extension 后外部扩展生态为空，需要一种跨语言、进程隔离、能力可控的优先机制。

## 决策

以 MCP 为第一外部扩展机制，优先于 WASM。支持 stdio / HTTP / 流式 HTTP，提供 Tools / Resources / Prompts、能力发现、健康检查、重启、取消、OAuth、输出限制、Secret 注入，每个 Server 独立配置权限。

## 后果

- 外部工具/资源可通过标准协议接入，进程隔离天然安全。
- 每个 Server 独立权限配置成为强需求。
- MCP Server 崩溃不得影响 Core，需故障隔离。

## 相关

- [mcp](../features/mcp.md) · [plugins](../features/plugins.md) · [ROADMAP Phase 9](../../ROADMAP.md)
