# P9-7：OAuth（P1）

> Phase 9 · MCP · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P6-4

**最终目的**：为需要鉴权的 MCP server 接入 OAuth。标记为 P1，可在 MVP 后交付。

**涉及范围**：`auth-service`、`mcp-client`

## 细分步骤

1. **复用 OAuth（P6-4）** —— 目的：避免重复实现。
2. **MCP server 鉴权接入** —— 目的：保护型 server 可用。
3. **token 存储与刷新** —— 目的：长期可用。
4. **测试** —— 目的：流程可用。

## 主要产出物

- MCP OAuth

## 验收标准

- [x] 保护型 MCP server 可完成 OAuth

## 验证记录（2026-08-09）

- 复用 `auth-service` PKCE、CSRF state、credential storage、singleflight refresh 与 refresh-token rotation；MCP 不复制 Token 生命周期。
- `OAuthHttpConnector` 注入 bearer，并在请求前检测轮换、要求 manager 重建 transport；fresh token、自动刷新、轮换持久化、PKCE 与 connector 脱敏均通过定向测试。

**相关文档**：[mcp](../docs/features/mcp.md) · [auth](../docs/features/auth.md) · [ROADMAP](../ROADMAP.md)
