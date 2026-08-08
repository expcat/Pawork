# P14-5：具体供应商实现（六初始供应商）

> Phase 14 · 模型用量与额度监控 · 状态：🟡未开始 · 依赖：P14-2、P14-3、P14-4

**最终目的**：把三种适配器框架落地到六个初始供应商（OpenAI、Anthropic、xAI、智谱、阿里、Moonshot），针对各自真实的额度机制（API 直连、OAuth 订阅、登录控制台、网页抓取、本地推算）分别实现，使每个供应商至少有一条可用额度数据来源，并正确映射其特有的窗口口径。

**涉及范围**：`quota-service`（供应商配置）；与各 `provider-*` crate 仅共享 `ProviderId`，不在 Core 走供应商特例分支

## 细分步骤

1. **OpenAI（GPT，API Key 直连）** —— 目的：用 `ApiKeyApiAdapter` 调用 billing / usage / rate-limit 相关 endpoint，获取整体花费与月度 / 速率窗口；标注 `exact`。
2. **Anthropic（Claude，登录控制台为主 + 滚动周窗口）** —— 目的：用 `OAuthApiAdapter` 或 `WebScrapeAdapter` 获取 Claude 的按周（weekly）额度与用量百分比；窗口映射为 `Weekly`，重置倒计时来自控制台。
3. **xAI Grok（OAuth 订阅 + API Key 双源）** —— 目的：订阅模式经 `OAuthApiAdapter` 取订阅请求配额窗口（按消息/时间滚动，多为 `Weekly` 或 `Overall`）；API Key 模式经 `ApiKeyApiAdapter` 取 token 用量与花费（`Overall`/`Monthly`）。两者按当前鉴权方式择优，订阅窗口若只能抓取则降级 `WebScrape`/`Scraped`。
4. **智谱 GLM（API Key 直连 + 控制台兜底）** —— 目的：经 `ApiKeyApiAdapter` 调用 `GET /api/paas/v4/usage` 取整体 token 用量与剩余（`Overall`，`exact`）；资源包明细（按模型分组的剩余量）若 API 不返回，经 `WebScrapeAdapter` 抓取 BigModel 控制台补充（`Scraped`）。
5. **阿里 Qwen（DashScope API Key 不可查余额，需 AccessKey 或抓取或本地推算）** —— 目的：DashScope 的 `sk-` key 不提供余额查询；提供两条可选来源——(a) 用户额外配置阿里云 AccessKey（`AccessKey ID`+`Secret`），经 `ApiKeyApiAdapter` 调用 BSS OpenAPI `QueryAccountBalance` / `QueryUserOmsData` 取账户余额与用量（`Overall`/`Monthly`，`exact`，凭据独立存储）；(b) 无 AccessKey 时经 `WebScrapeAdapter` 抓取百炼控制台余额（`Scraped`）。两条都不可用时仅保留本地用量推算（`Derived`）。
6. **Moonshot Kimi（API Key 直连余额接口）** —— 目的：经 `ApiKeyApiAdapter` 调用 `GET /v1/users/me/balance`，解析 `available_balance` / `voucher_balance` / `cash_balance`，取可用余额映射为 `Overall`（`exact`，金额维度）。
7. **窗口口径对齐** —— 目的：把供应商各自表述（spending limit / rate limit / 5h 滚动 / 周 / 月 / 订阅配额 / 余额）统一映射到 Overall / Rolling5h / Weekly / Monthly，对不上口径的窗口返回 `Unsupported` 而非猜测。
8. **逐供应商契约测试** —— 目的：每个供应商用各自夹具验证归一结果与窗口映射，失败时给出可读差异。
9. **能力矩阵** —— 目的：以「供应商 × 窗口 × 适配器」矩阵文档化每个供应商支持的数据来源与可信度，供 P14-8 UI 展示来源标签。

## 主要产出物

- OpenAI / Anthropic / xAI / 智谱 / 阿里 / Moonshot 的额度适配配置与归一
- 供应商 × 窗口 × 适配器 能力矩阵文档
六供应商契约测试

## 验收标准

- [ ] 六个初始供应商各至少一种窗口可用真实或夹具数据获取额度
- [ ] Anthropic 周窗口与 Qwen 余额口径（余额 / 本地推算二选一）被正确映射
- [ ] Moonshot `available_balance` 与智谱 `/usage` 正确归一为 `Overall`
- [ ] Qwen 在无 AccessKey 且抓取失败时降级到本地推算（`Derived`），不伪造远端额度
- [ ] 不支持的窗口返回 `Unsupported`，不在 Agent Core 走供应商特例
- [ ] 能力矩阵与实际实现一致

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不新增依赖，复用 P14-1/2/3/4 已确定的栈。
