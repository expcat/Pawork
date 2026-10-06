# B13 · MCP OAuth 未接线模块裁决

> 状态：待裁决。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#待裁决api-与产品取舍)

## 目标

裁决 MCP OAuth 生产接入或保留/归档边界。

## 当前线索与读取范围

登记项为 OAuth 模块整体零生产接线：`begin_pkce_login` / `complete_pkce_login` / `McpBearerProvider` / `OAuthHttpConnector`。

[mcp Spec](../spec/crates/mcp.md)、[auth Spec](../spec/crates/auth.md)、[凭证流程](../spec/flows.md)。

## 实施范围

- 核对 MCP HTTP 实际认证路径和 Secret 存储边界，再裁决模块用途。
- 若接线，明确 PKCE、回调关联、刷新、失效与脱敏；不把凭证绕过 auth backend。

## 完成条件

- 生产用途或保留/归档结论有证据，MCP 凭证安全语义明确。
- 实际变更带对应认证拒绝、回调/刷新和脱敏定向回归，同步 Spec。

## 完成记录

待裁决，未在本次执行实现或产品验证。裁决保留时记录事实依据；裁决修改时记录实际改动与验证。满足完成条件后同步本文件状态、完成日期与证据，并将 ROADMAP 对应项标为 ✅。

