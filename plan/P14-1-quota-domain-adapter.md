# P14-1：Quota 领域模型与适配器 Trait

> Phase 14 · 模型用量与额度监控 · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：库级契约验证，生产接线待 P18） · 依赖：P2-9、P2-7、P6-4、P18-2、P18-3

**最终目的**：为「显示绑定模型的用量与剩余额度」建立统一的领域模型与适配器抽象，使后续 P14-2 ~ P14-5 的三种适配器（API Key 直连 / OAuth 登录授权 / 网页抓取）与具体供应商实现都落在同一契约上，Agent Core 与 UI 只依赖 canonical 额度数据，不感知供应商差异。

**涉及范围**：新增 `quota-service` crate；实际依赖 `agent-domain`、`provider-api`、`provider-runtime`、`usage-ledger`（复用 `ProviderId` / `ResolvedCredential`）

## 细分步骤

1. **新建 `quota-service` crate** —— 目的：承接额度获取、归一与缓存，按 workspace-layout 依赖方向登记；只依赖 `provider-api`，不引入 `agent-domain` 禁依赖项。
2. **定义额度快照领域模型** —— 目的：统一「剩余 / 已用 / 总量 / 重置时间 / 数据时间戳」口径，覆盖请求次数、token、金额三种计量维度。`QuotaSnapshot` 必须带 tenant/account/provider/model scope；credential 只使用 opaque ID，不暴露 Secret。
3. **定义额度窗口枚举** —— 目的：覆盖用户关心的四种限制窗口：`Overall`（整体 API 用量剩余）、`Rolling5h`（5 小时滚动窗口）、`Weekly`（一周）、`Monthly`（一月）；每个窗口带 `resets_at`（绝对时间）或 `resets_in`（相对倒计时）语义与不确定性标记。
4. **定义适配器种类 `AdapterKind`** —— 目的：显式区分 `ApiKeyApi`（key+REST API 直取）、`OAuthApi`（需登录授权后调 console API）、`WebScrape`（无公开 API 的页面抓取），驱动 P14-2/3/4 的实现拆分与 UI 上的数据来源可信度提示。
5. **定义 `QuotaAdapter` Trait** —— 目的：对象安全的统一获取入口。方法：`adapter_kind`、`supports(provider, window)`、`fetch_quota(provider, credential, window, cancel) -> QuotaSnapshot`；返回值携带 `source`（adapter kind + endpoint 摘要）、`confidence`（exact/derived/scraped）、`fetched_at`。
6. **能力发现与降级语义** —— 目的：当某供应商某窗口无法直接获取时，返回 `Unsupported` 而非报错，由 P14-6 聚合层决定是否用本地推算（P14-7）兜底。
7. **测试** —— 目的：用 Mock 适配器验证 Trait 契约、窗口枚举与降级语义，作为后续适配器的黄金基线。

## 主要产出物

- `quota-service` crate 骨架与领域类型
- `QuotaAdapter` Trait + `AdapterKind` + `QuotaWindow`
- Mock 适配器单元测试

## 验收标准

- [x] `quota-service` 不依赖 `agent-domain` 禁依赖项
- [x] `QuotaSnapshot` 能表达剩余 / 已用 / 总量 / 重置时间，并能表示无限额度与未知总量
- [x] 四种窗口（Overall / Rolling5h / Weekly / Monthly）均可表达重置倒计时与不确定性
- [x] `QuotaAdapter` 对不支持的情形返回 `Unsupported`，不 panic
- [x] 不同 tenant/account 的额度窗口不被错误合并；legacy synthetic account 可正常查询

**相关文档**：[usage-quota](../docs/features/usage-quota.md) · [providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：领域类型纯数据，复用 `provider-api` 既有 `ProviderId` / `ResolvedCredential` / `ProviderError`；不引入新依赖。实际依赖与 `Cargo.toml` 一致为 `agent-domain` / `provider-api` / `provider-runtime` / `usage-ledger`（非「只依赖 provider-api」）；无 `auth-service` 参与——凭据经 provider contract 注入 `ResolvedCredential`，真实绑定/租约由 P18 账号控制面提供。
