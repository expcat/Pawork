# 模型用量与额度监控

## 职责

显示每个绑定模型的用量与剩余额度，通过多种适配器从各供应商获取真实额度数据，覆盖整体 API 用量、5 小时滚动、一周、一月等不同限制窗口，并归一为统一视图供预算、CLI 与 GUI 使用。Agent Core 与 UI 只依赖 canonical 额度数据，不感知供应商差异。

## 额度窗口

| 窗口 | 含义 | 典型来源 |
| --- | --- | --- |
| `Overall` | 整体 API 用量剩余 / 账户总额度 | billing / spending limit |
| `Rolling5h` | 5 小时滚动窗口限制 | 控制台 / 滚动速率窗口 |
| `Weekly` | 一周额度限制 | Anthropic 周窗口等 |
| `Monthly` | 月度额度限制 | 月度花费上限 / 月度配额 |

每个窗口携带重置时间（绝对或倒计时）与不确定性标记；供应商不支持的窗口返回 `Unsupported`，不伪造。

## 适配器种类

| 种类 | 适用场景 | 可信度 |
| --- | --- | --- |
| `ApiKeyApi` | 持有 API Key，可直接调用官方 billing / usage API（如 OpenAI） | exact |
| `OAuthApi` | 需登录授权后调用平台 console / usage API | exact |
| `WebScrape` | 无公开 API，需抓取控制台页面解析额度数字 | scraped |

同一窗口多适配器可用时按 exact > derived > scraped 排序；本地用量推算（基于 P2-9）标记为 `derived`，作为远端未刷新时的兜底。

## 数据模型（节选）

```rust
pub enum QuotaWindow { Overall, Rolling5h, Weekly, Monthly }

pub struct QuotaSnapshot {
    pub provider: ProviderId,
    pub model: Option<String>,
    pub window: QuotaWindow,
    pub measure: QuotaMeasure,   // Count | Token | Cost
    pub used: Option<u64>,
    pub limit: Option<u64>,       // None 表示无限额度或未知总量
    pub remaining: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
    pub confidence: QuotaConfidence, // Exact | Derived | Scraped
    pub fetched_at: DateTime<Utc>,
}
```

## 供应商能力矩阵（P14-5 落地后更新）

| 供应商 | 鉴权方式 | 主适配器 | 支持窗口 | 说明 |
| --- | --- | --- | --- | --- |
| OpenAI（GPT） | API Key | `ApiKeyApi` | Overall / Monthly / 速率 | 经 billing / usage API 直取，`exact` |
| Anthropic（Claude） | API Key / Console | `OAuthApi` / `WebScrape` | Weekly / Overall | 登录控制台取周窗口，`exact` 或 `scraped` |
| xAI Grok | OAuth 订阅 / API Key | `OAuthApi`（订阅）/ `ApiKeyApi`（key） | Weekly / Overall | 订阅按请求配额；key 按 token 用量，订阅端点不稳定时降级抓取 |
| 智谱 GLM | API Key | `ApiKeyApi` / `WebScrape` | Overall / Monthly | `GET /api/paas/v4/usage` 取剩余；资源包明细抓控制台 |
| 阿里 Qwen | DashScope key（不可查余额） | `ApiKeyApi`（AccessKey）/ `WebScrape` / 本地推算 | Overall / Monthly | 需额外阿里云 AccessKey 调 BSS `QueryAccountBalance`；否则抓百炼控制台或仅本地推算 `derived` |
| Moonshot Kimi | API Key | `ApiKeyApi` | Overall | `GET /v1/users/me/balance` 返回 `available_balance`，`exact` |
| Google Gemini（次要 P1） | OAuth | `OAuthApi` | RPM / TPM / 项目配额 | OAuth + quota API；已降级次要，非初始集合 |

## 与既有能力的关系

- 单次请求的 token / 费用归一见 [P2-9](../plan/P2-9-usage-stopreason.md)，本特性在此基础上做窗口级累计与对照。
- 模型定价与费用估算见 [models](models.md) 与 [P2-7](../plan/P2-7-model-registry.md)。
- 凭据与 Secret 见 [auth](auth.md)；明文凭据不落库不进日志。
- 预算联动见 [context（token 预算）](context.md) 与 [P3-6](../plan/P3-6-budget-control.md)，触限动作有事件可追溯。
- 错误模型复用 `QuotaExceeded` / `RateLimited`。

## 展示

CLI 经 `pawork usage` 输出各绑定模型的多窗口额度、重置倒计时与来源标签；GUI 经 GUI Connection Protocol 订阅额度查询与变更事件，脱敏展示，并明确区分精确 / 推算 / 抓取数据与「需重新登录」「抓取失败」状态。

## 验收标准

- 三大供应商各至少一种窗口可获取真实或夹具额度
- 明文凭据不写入数据库与日志
- 多窗口以统一视图呈现，来源与可信度被清晰标注
- 限额接近 / 触及时触发告警与可执行建议

## 相关文档

- [providers](providers.md) · [models](models.md) · [auth](auth.md) · [context](context.md) · [observability](observability.md)
- [workspace-layout](../architecture/workspace-layout.md)（`quota-service`）
- [ROADMAP Phase 14](../ROADMAP.md)
