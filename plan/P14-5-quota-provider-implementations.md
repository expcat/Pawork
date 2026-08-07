# P14-5：具体供应商实现（OpenAI / Anthropic / Google）

> Phase 14 · 模型用量与额度监控 · 状态：🟡未开始 · 依赖：P14-2、P14-3、P14-4

**最终目的**：把三种适配器框架落地到三大主供应商，针对各自真实的额度机制（API 直连、登录控制台、滚动窗口）分别实现，使每个供应商至少有一条可用额度数据来源，并正确映射其特有的窗口口径。

**涉及范围**：`quota-service`（供应商配置）；与 `provider-openai` / `provider-anthropic` / `provider-google` 仅共享 `ProviderId`，不在 Core 走供应商特例分支

## 细分步骤

1. **OpenAI（API Key 直连）** —— 目的：用 `ApiKeyApiAdapter` 调用 billing / usage / rate-limit 相关 endpoint，获取整体花费与月度 / 速率窗口；标注 `exact`。
2. **Anthropic（登录控制台为主 + 滚动周窗口）** —— 目的：用 `OAuthApiAdapter` 或 `WebScrapeAdapter` 获取 Claude 的按周（weekly）额度与用量百分比；窗口映射为 `Weekly`，重置倒计时来自控制台。
3. **Google（OAuth + API）** —— 目的：基于 `OAuthApiAdapter` 调用 Gemini 的 quota / usage API，获取 RPM / TPM 与项目级配额，映射 Count / Token 窗口。
4. **窗口口径对齐** —— 目的：把供应商各自表述（spending limit / rate limit / 5h 滚动 / 周 / 月 / 项目配额）统一映射到 Overall / Rolling5h / Weekly / Monthly，对不上口径的窗口返回 `Unsupported` 而非猜测。
5. **逐供应商契约测试** —— 目的：每个供应商用各自夹具验证归一结果与窗口映射，失败时给出可读差异。
6. **能力矩阵** —— 目的：以「供应商 × 窗口 × 适配器」矩阵文档化每个供应商支持的数据来源与可信度，供 P14-8 UI 展示来源标签。

## 主要产出物

- OpenAI / Anthropic / Google 的额度适配配置与归一
- 供应商 × 窗口 × 适配器 能力矩阵文档
- 三供应商契约测试

## 验收标准

- [ ] 三大供应商各至少一种窗口可用真实或夹具数据获取额度
- [ ] Anthropic 周窗口与 Google 配额窗口被正确映射
- [ ] 不支持的窗口返回 `Unsupported`，不在 Agent Core 走供应商特例
- [ ] 能力矩阵与实际实现一致

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不新增依赖，复用 P14-1/2/3/4 已确定的栈。
