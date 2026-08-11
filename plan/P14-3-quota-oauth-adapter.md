# P14-3：OAuth 登录授权适配器

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P14-1、P6-4、P2-1

**最终目的**：为「需要登录授权才能查看额度」的供应商（如部分平台需用户登录控制台后调用其 console / usage API）提供适配器，复用 Phase 6 的 OAuth 基础设施，自动 refresh，避免用户为查额度反复登录。

**涉及范围**：`quota-service`；复用 `auth-service::oauth`、`provider-runtime`

## 细分步骤

1. **OAuth 凭据解析** —— 目的：经 `auth-service::oauth` 取得 access token（自动 refresh），明文 token 只存 SecretBackend，调用前注入 Bearer header。
2. **`OAuthApiAdapter` 实现** —— 目的：在 `quota-service` 内基于 `QuotaAdapter` 实现第二种适配器，复用 `provider-runtime` 发起带 OAuth 的 console / usage API 调用。
3. **token 过期与重授权** —— 目的：遇 401 触发 refresh 后重试一次；refresh 失败时返回需用户重新登录的状态，交由 P14-8 提示。
4. **响应归一与窗口映射** —— 目的：将控制台 API 的用量结构归一为 `QuotaSnapshot`，重点支持登录态才能看到的滚动窗口（5h / 周 / 月）。
5. **scope / 端点配置** —— 目的：以 Provider 为维度声明所需 OAuth scope 与 usage endpoint，便于 P14-5 的具体供应商各自配置。
6. **测试** —— 目的：wiremock 模拟 OAuth token、refresh、401 重授权与各窗口响应。

## 主要产出物

- `OAuthApiAdapter` 实现
- OAuth scope / endpoint 声明结构
- 重授权与窗口归一测试

## 验收标准

- [x] OAuth 过期自动 refresh 后重试成功
- [x] refresh 失败时返回可恢复的「需重新登录」状态，不报错中断
- [x] 明文 token 不落库不进日志

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [auth](../docs/features/auth.md) · [providers](../docs/features/providers.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：复用 `auth-service::oauth`（P6-4 最小手写实现）与 `provider-runtime`，不新增 OAuth SDK 依赖。
