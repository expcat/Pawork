# P9-2：Streamable HTTP Transport

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P9-1

**最终目的**：实现 MCP Streamable HTTP Transport（MCP 2025-03-26 起的规范，旧 HTTP+SSE 已弃用），覆盖远程 MCP server，含 timeout/restart。

**涉及范围**：`mcp-client`

## 细分步骤

1. **Streamable HTTP 通信** —— 目的：远程接入。
2. **timeout / restart** —— 目的：稳健。
3. **错误处理** —— 目的：可恢复。
4. **测试** —— 目的：可用。

## 主要产出物

- MCP Streamable HTTP Transport

## 验收标准

- [x] 含 timeout/restart，可用

## 验证记录（2026-08-09）

- 官方 `rmcp` Streamable HTTP client 已接入；握手 / request timeout、有界指数退避重连、HTTPS Secret 边界和 Authorization 冲突均有定向测试。
- `cargo test -p mcp-client`：48 passed；`cargo clippy -p mcp-client --all-targets -- -D warnings`：通过。

**相关文档**：[mcp](../docs/features/mcp.md) · [ADR-011](../docs/adr/ADR-011-mcp-first-extension.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：HTTP 客户端用 reqwest（rustls + stream）；协议编解码用官方 rmcp SDK 的 transport + codec 层，锁定小版本并隔离在 mcp-client 内。
