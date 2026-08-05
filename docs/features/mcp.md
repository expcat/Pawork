# MCP 支持

## 职责

提供 Model Context Protocol 客户端能力，作为 Pawork 的第一外部扩展机制，支持外部 server 注册工具、资源与提示。

## Transport

支持：stdio；Streamable HTTP（MCP 2025-03-26 起的远程传输规范，旧 HTTP+SSE 已弃用）；用户配置自定义 Server。

## 功能

Tools；Resources；Prompts；Server capability discovery；Server health；timeout；restart；cancel；logging；OAuth；Workspace scoped server；Global server；Tool approval；输出限制；Secret 注入。

## 安全

每个 MCP Server 单独配置：

```text
是否可信
允许的工具
允许的 Workspace
网络权限
Secret 权限
自动启动
超时
最大输出
```

## 验收标准

- MCP Server 崩溃不影响 Agent Core
- 每个 Server 有独立权限
- Tool Output 限制与 Cancellation 有效

## 相关文档

- [plugins](plugins.md) · [policy](policy.md) · [auth（OAuth）](auth.md)
- [ADR-011 MCP 第一扩展机制](../adr/ADR-011-mcp-first-extension.md)
- [ROADMAP Phase 9](../../ROADMAP.md)
