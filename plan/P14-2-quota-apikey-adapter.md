# P14-2：API Key 直连适配器

> Phase 14 · 模型用量与额度监控 · 状态：🟡未开始 · 依赖：P14-1、P2-1、P2-6

**最终目的**：实现「持有 API Key 即可直接通过官方 REST billing / usage API 获取额度」这一最高可信度适配器，作为多数主流供应商（OpenAI 等）的默认数据来源，奠定 P14-5 具体供应商实现的通用底座。

**涉及范围**：`quota-service`；复用 `provider-runtime`（HTTP / 超时 / 重试）、`auth-service`（解析 key）

## 细分步骤

1. **API Key 凭据解析** —— 目的：经 `auth-service` 把 Provider 绑定的 key 解析为 `ResolvedCredential`，明文不落库不进日志，请求时注入 header。
2. **请求构造通用层** —— 目的：在 `quota-service` 内提供 `ApiKeyApiAdapter`，复用 `provider-runtime` 的 reqwest 客户端（超时 / 代理 / trace / cancel），统一发起带认证的 GET。
3. **响应归一** —— 目的：把供应商各异的 billing / usage JSON 归一为 `QuotaSnapshot`，按计量维度（Count / Token / Cost）与窗口（Overall / 5h / 周 / 月）映射；窗口不存在的字段留空而非伪造。
4. **限流与错误处理** —— 目的：复用 `provider-runtime` 重试（P2-10）；遇 401/403 标记凭据失效、429 标记限速并回退，避免拖垮额度查询节奏。
5. **可信度与来源标注** —— 目的：`confidence = exact`、`source` 记录 endpoint 与数据时间戳，供 UI 与 P14-9 决定是否需要兜底。
6. **测试** —— 目的：用 wiremock 构造各窗口响应与错误，验证归一与降级。

## 主要产出物

- `ApiKeyApiAdapter` 实现与请求 / 响应归一
- billing / usage 响应的 contract 测试夹具

## 验收标准

- [ ] 持有有效 API Key 时能获取至少一种窗口的真实额度
- [ ] 明文 Key 不写入日志与数据库
- [ ] 401 / 403 / 429 被正确归类，不静默崩溃

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [providers](../docs/features/providers.md) · [auth](../docs/features/auth.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：仅复用 `provider-runtime`（reqwest rustls）与 `auth-service`，不新增第三方依赖。
