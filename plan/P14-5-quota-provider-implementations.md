# P14-5：具体供应商实现（六初始供应商）

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：无静态能力矩阵；六供应商实现仅库级/contract fixture，未注册生产 refresh target） · 依赖：P14-2、P14-3（已归档）、P14-4

**最终目的**：把三种适配器框架落地到六个初始供应商（OpenAI、Anthropic、xAI、智谱、阿里、Moonshot），只实现当前官方接口或可验证控制台实际提供的额度机制；没有公开能力时明确 `Unsupported`，不把 consumer 订阅限制、账号余额或本地估算伪装成模型精确额度。

**涉及范围**：`quota-service`（供应商配置）；与各 `provider-*` crate 仅共享 `ProviderId`，不在 Core 走供应商特例分支

## 细分步骤

1. **OpenAI（Organization Admin API）** —— 组合 `GET /v1/organization/spend_limit` 与分页 `GET /v1/organization/costs`，归一为 `Monthly / USD / Exact`；普通 inference key、Overall、token、5h 与周窗口均明确 `Unsupported`。
2. **Anthropic（Enterprise Spend Limits API）** —— 用 Admin key + `read:spend_limits` 调用 `GET /v1/organizations/spend_limits/effective`，按 `scope.type=user` 与 `user_id` 选择月度 USD 上限/花费；`amount=null` 表示无限，`spend=null` 表示未知。consumer 5h/周订阅限制没有公开 exact API。
3. **xAI（Management Billing API）** —— Management bearer key + `team_id`：prepaid `/prepaid/balance` 提供 `Overall / USD`；postpaid spending limit 与 invoice preview 组合 `Monthly / USD`。普通 inference key 与订阅消息窗口不在该能力内。
4. **智谱 GLM（显式 WebScrape 兜底）** —— 当前没有公开 exact usage/quota endpoint；仅对用户显式启用、已登录的 Coding Plan 控制台用版本化 selector 提取 `Rolling5h / Weekly / Count / Scraped`，其余返回 `Unsupported`。
5. **阿里 Qwen（Alibaba BSS 账号余额）** —— DashScope inference key 不提供余额查询；用户额外配置 Alibaba Cloud AccessKey pair 后，以 HMAC-SHA1 调用 BSS OpenAPI `QueryAccountBalance`，归一为账号级 `Overall / CNY / Exact`。该结果不是 DashScope 专属额度；无 AccessKey 时只可使用 Ledger `Derived`，不抓取或猜测远端余额。
6. **Moonshot Kimi（API Key 直连余额接口）** —— 目的：经 `ApiKeyApiAdapter` 调用 `GET /v1/users/me/balance`，解析 `available_balance` / `voucher_balance` / `cash_balance`，取可用余额映射为 `Overall`（`exact`，金额维度）。
7. **窗口口径对齐** —— 目的：把供应商各自表述（spending limit / rate limit / 5h 滚动 / 周 / 月 / 订阅配额 / 余额）统一映射到 Overall / Rolling5h / Weekly / Monthly，对不上口径的窗口返回 `Unsupported` 而非猜测。
8. **逐供应商契约测试** —— 目的：每个供应商用各自夹具验证归一结果与窗口映射，失败时给出可读差异。
9. **能力矩阵（已收敛）** —— 目的：以「供应商 × 窗口 × 适配器」矩阵文档化数据来源与可信度，供 UI 展示来源标签。运行时静态矩阵（`providers/capability.rs`）无生产消费者已删除，事实源为 adapter `supports()`，docs 表格仅作说明。

## 主要产出物

- OpenAI / Anthropic / xAI / 智谱 / 阿里 / Moonshot 的额度适配配置与归一
- 供应商 × 窗口 × 适配器 能力矩阵文档
六供应商契约测试

## 验收标准

- [x] 六个初始供应商各至少一种窗口可用真实或夹具数据获取额度
- [x] Anthropic Enterprise 月度 user scope 与 Qwen 账号级余额口径被正确映射
- [x] Moonshot `available_balance` 归一为 `Overall / CNY`；智谱仅把 Coding Plan 控制台窗口标为 `Scraped`
- [x] Qwen 在无 AccessKey 时只保留 Ledger 本地推算（`Derived`），不伪造远端额度
- [x] 不支持的窗口返回 `Unsupported`，不在 Agent Core 走供应商特例
- [x] 能力矩阵与实际实现一致（运行时静态矩阵已删除，由 adapter `supports()` 契约测试保证）

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 核验）**：复用 P14-1/2/4 栈（P14-3 已归档）；Qwen BSS 签名使用 workspace 基线中的 `hmac` + `sha1`，不引入供应商 SDK。六家 provider factory 与 contract fixtures 已交付但未进入生产 composition root（未注册 refresh target），接线延 P18。
