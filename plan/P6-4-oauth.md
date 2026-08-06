# P6-4：OAuth

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P2-6

**最终目的**：实现 OAuth（PKCE / Device Flow / auto refresh / callback 接收），为需要 OAuth 的 provider 与 MCP 提供鉴权基础。

**涉及范围**：`auth-service`

## 细分步骤

1. **PKCE / Device Flow** —— 目的：覆盖主流 OAuth 流程。
2. **auto refresh** —— 目的：token 自动续期。
3. **callback 接收** —— 目的：本地回调可接收授权码。
4. **callback 测试** —— 目的：流程可验证。

## 主要产出物

- OAuth 模块

## 验收标准

- [ ] PKCE/Device Flow 可完成
- [ ] callback 测试通过

**相关文档**：[auth](../docs/features/auth.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：基于 oauth2 crate 实现 PKCE / refresh；Device Flow 直接实现 RFC 8628（协议很小，可参考 oauth-device-flows）；回调本地监听用 tiny_http / hyper，不引入 axum。
