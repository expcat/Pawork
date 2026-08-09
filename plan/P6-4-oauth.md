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

- [x] PKCE/Device Flow 可完成
- [x] callback 测试通过

**相关文档**：[auth](../docs/features/auth.md) · [ROADMAP](../ROADMAP.md)

**依赖决策（2026-08-09 落地）**：维持最小手写 OAuth，基于 `reqwest` + `url` 实现 PKCE / refresh / RFC 8628，使用 `base64` / `rand` / `sha2` 提供编码、随机与 S256 原语。实现面仅覆盖 Pawork 所需子集，已有 RFC 7636 向量、state 校验、auto-refresh、轮换 token 回写、callback 分片/端口对齐测试，因此移除零引用的 `oauth2` 基线声明。
