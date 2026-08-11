# 模型用量与额度监控

## 职责

Phase 14 为每个 `tenant/account/credential/provider/model` 作用域提供统一的额度快照、多窗口聚合、耗尽预测、预算信号、查询展示、自动刷新与告警。Provider 差异只存在于 `quota-service` adapter；Agent Engine、app-service、CLI 与 GUI 只消费 canonical 类型。

本功能只维护“当前远端额度读数、派生窗口与刷新状态”。P18-8 `Usage/Cost Ledger` 是本地 Usage/Cost 的唯一事实源：P14 可以扩展其维度、查询和幂等重放能力，但不得创建第二套累计账本。远端 billing/usage 读数也不会回写成一套新的本地累计记录。

架构红线：

- 所有缓存键和查询都强制包含 `tenant_id + account_id`；`credential_id` 与 `model_id` 是可选的进一步隔离维度。
- 明文 API Key、OAuth token、refresh token、AccessKey Secret 与 cookie 只以进程内 `ResolvedCredential` 短暂传给 adapter，不序列化、不落库、不进日志或审计。
- 不支持的 endpoint、窗口或单位返回 `Unsupported`；未知值使用 `Unknown`，不得推测。
- 来源优先级固定为 `Exact > Derived > Scraped`，同时向调用方暴露来源、可信度、抓取/观察时间、stale 与部分失败。

## 当前状态边界（2026-08-11）

Phase 14 交付的是**库级完整、生产链路未闭合**的 quota library 与本地投影骨架：六家 provider adapter、缓存聚合、Ledger 投影、singleflight/取消、刷新调度与告警状态机均有测试闭环，但正式 `pawork` 主流程只装配了本地 Ledger 适配器。以下边界为当前事实，后续接线分别登记在 P18-2/3/4/8/13/14 与 P19-2/10 的验收标准（评审依据见 [p14-review](../review/p14-review.md)）：

- **quota-service 不依赖 auth-service，无 OAuth 通用层**。`Cargo.toml` 只依赖 agent-domain / provider-api / provider-runtime（HTTP 客户端与错误分类）/ usage-ledger；`adapters/oauth.rs` 已删除。`AdapterKind::OAuthApi` 仅保留为契约枚举，没有实现；首个真实 OAuth 供应商接入时再恢复通用层（见下文 OAuth API）。
- **API Key 通用层只有 Moonshot 一个生产消费者**。`ApiKeyQuotaAdapter` / `ApiKeyQuotaEndpoint` 目前仅 Moonshot 使用；其余 provider 因多端点或签名差异直接实现 `QuotaAdapter`。P18 接线后若仍只有单消费者，应内联到 Moonshot。
- **本地 Ledger 是进程内的，归属是 synthetic 的**。`QuotaRuntime::production` 每次 CLI 进程新建 `InMemoryUsageLedger`；`record_run_usage` 固定 `tenant=local`、`account=local/default`、`credential_id=None`，principal/agent 为默认身份，费用按 builtin 定价估算。一次 `pawork run` 写入的用量不能被下一次 `pawork usage` 读取；跨进程持久化与真实归属待 P18-2/3/4/8。
- **查询必须显式 provider**。`QuotaOverview` 缺省或空 `provider_id` 直接返回 validation error，不再静默选择“第一个已注册 provider”或默认 ID；多 provider/多模型聚合待 P18 binding enumeration 成为事实源后由 app-service 批量查询。
- **scheduler/sink 存在但无生产 targets**。`RefreshScheduler`、`AuditSink`、`AlertSink` 与退避/去重/恢复状态机测试充分，但生产 composition root 不构造 scheduler、不注册远端 target；`RunSupervisor::alert_sink()` 的告警桥已就绪，等待 P18-14 把六家远端 adapter 注册为 target 并启动生命周期。
- **WebScrape 内置审计待 canonical**。WebScrape 仍持有有界内存 audit Vec（`audit_entries`），与 scheduler `AuditSink` 职责重叠；生产路径只应保留外部 sink，第二份审计记录待 P18-13 合并。
- **GUI 只有协议、没有页面**。core-api / gui-protocol 已携带 `QuotaOverview` / `QuotaChanged` / `QuotaAlert` 类型与事件传输，protocol-test-gui 覆盖 QuotaAlert roundtrip（含旧 JSON 兼容）；仓库没有 quota projection / controller / page。Desktop 投影与页面登记在 P19-2 / P19-10。
- **告警 kind/source 为 Option，兼容旧重放**。`QuotaAlert.kind` / `source` 是后加字段：旧事件 JSON 缺省时解码为 `None`（重放兼容），新事件总是 `Some`（source 已脱敏，不携带端点 query/fragment 或 secret）。

## Canonical 契约

`QuotaScope` 包含 tenant、account、provider，以及可选 credential/model。`QuotaSnapshot` 的核心字段如下：

```rust
pub enum QuotaWindow { Overall, Rolling5h, Weekly, Monthly }
pub enum QuotaUnit { Count, Token, Cost { currency: String } }
pub enum QuotaMeasure { Exact(u64), Infinite, Unknown }
pub enum QuotaReset {
    Absolute { at: Timestamp, uncertain: bool },
    Relative { after_secs: u64, observed_at: Timestamp, uncertain: bool },
    Unknown,
}

pub struct QuotaSnapshot {
    pub scope: QuotaScope,
    pub window: QuotaWindow,
    pub unit: QuotaUnit,
    pub values: QuotaValues, // used / limit / remaining
    pub reset: QuotaReset,
    pub confidence: Confidence, // Exact / Derived / Scraped
    pub provenance: QuotaProvenance,
}
```

金额统一为对应 ISO-4217 币种的整数 micros，避免浮点累计误差。`Infinite` 与 `Unknown` 分离；`reset` 同时表达绝对时间、相对倒计时和不确定性。`QuotaAdapter` 是对象安全、可取消的异步 trait，adapter 类型为 `ApiKeyApi`、`OAuthApi`（仅契约，暂无实现）、`WebScrape`、`LocalLedger`。

## 供应商能力矩阵

下表按 2026-08-11 的官方公开接口核验，属说明性文档；可执行事实源是各 adapter 的 `supports()`（运行时 capability matrix 代码已删除，不再维护第三份静态矩阵）。所有供应商均可额外消费 Ledger 得到 `Derived` 本地用量；该派生值不改变远端能力结论。

| 供应商 | Exact 远端能力 | 凭据与作用域 | 诚实降级 / Unsupported |
| --- | --- | --- | --- |
| OpenAI | `Monthly / USD`：组合 `GET /v1/organization/spend_limit` 的硬上限与 `GET /v1/organization/costs` 的分页月度花费 | Organization Admin key；普通 inference key 不具备该权限 | 任一子端点失败时仅保留已知字段并标部分失败；无公开 Overall 余额、token quota 或 5h/周窗口 |
| Anthropic | `Monthly / USD`：`GET /v1/organizations/spend_limits/effective` | `x-api-key` Admin key、`read:spend_limits`；仅 Claude Enterprise 且组织使用 usage credits；响应按 `scope.type=user` + `user_id` 选择 | 普通 Claude Platform 组织及 consumer 5h/周限制无公开 exact API；`amount = null` 表示无限，金额字符串单位为 cents |
| xAI | `Overall / USD` prepaid balance；`Monthly / USD` 由 postpaid spending limit 与 invoice preview 组合 | Management API bearer key + 明确 `team_id` | 普通 inference key及 5h/周窗口 Unsupported；子端点失败显式部分失败，不推算另一字段 |
| 智谱 GLM | 无公开 exact usage/quota endpoint | 可选、显式启用的已登录控制台 session | Coding Plan 的 `Rolling5h / Weekly` 只可作为版本化 `Scraped` 兜底；不把控制台余额伪装成模型 Overall quota |
| 阿里 Qwen | `Overall / CNY`：Alibaba BSS `QueryAccountBalance` | Alibaba Cloud AccessKey pair + HMAC-SHA1；结果是整个阿里云账号余额 | DashScope inference key 不能查余额；BSS 结果不得标成 DashScope 专属额度，月度/token quota Unsupported |
| Moonshot Kimi | `Overall / CNY`：`GET /v1/users/me/balance` | Moonshot bearer API key | 无公开月度、5h、周窗口 |

官方口径依据：[OpenAI spend limit](https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/spend_limit/methods/retrieve)、[OpenAI costs](https://developers.openai.com/api/reference/resources/admin/subresources/organization/subresources/usage/methods/costs)、[Anthropic spend limits](https://platform.claude.com/docs/en/manage-claude/spend-limits-api)、[xAI Management Billing](https://docs.x.ai/developers/rest-api-reference/management/billing)、[智谱 Coding Plan FAQ](https://docs.bigmodel.cn/cn/coding-plan/faq)、[Alibaba BSS QueryAccountBalance](https://help.aliyun.com/en/user-center/developer-reference/api-bssopenapi-2017-12-14-queryaccountbalance)、[Moonshot balance](https://platform.kimi.com/docs/api/balance)。

## Adapter 行为

### API Key API

通用 adapter 负责凭据缺失、取消、HTTP 状态分类、JSON 解析边界和 provenance 脱敏。401、403、429 分别映射为未授权、权限不足、限频；`Retry-After` 被保留。URL 的 query/fragment、响应正文和认证 header 不进入错误文本。

当前 `ApiKeyQuotaAdapter` / `ApiKeyQuotaEndpoint` 只有 Moonshot 一个生产消费者（`MoonshotEndpoint`，`GET /v1/users/me/balance`）；其余 provider 直接实现 `QuotaAdapter`。P18-14 接线后若仍只有单消费者，该层应内联到 Moonshot 删除 endpoint trait。

### OAuth API（仅契约）

`AdapterKind::OAuthApi` 保留为稳定枚举形态（provenance / 序列化兼容），但通用 `OAuthQuotaAdapter` 层已随 P14 remediation 移除，quota-service 不再依赖 auth-service。OAuth 类额度（xAI Management 的 OAuth bearer、智谱控制台 session 等）在首个真实 provider 接入时再恢复通用层，行为约定：通过注入的 credential resolver 获得短期 token，首次 401 触发一次 refresh 后重试，`invalid_grant` / refresh token 失效映射为 `ReauthorizationRequired` 并给出重新登录动作；refresh token 仍只存在于 Secret backend。

### WebScrape

WebScrape 默认关闭，只能作为低可信度兜底。每个 profile 必须声明 selector 版本、支持窗口、最小请求间隔和 TTL；相同作用域命中缓存时不发请求。限频采用并发预约，不能让多个 caller 在同一延迟后突发请求。解析失败返回可诊断错误并写脱敏审计（当前为有界内存 Vec，测试/诊断用；P18-13 canonical audit 落地后并入外部 sink），但不记录 URL query、cookie、HTML 正文或 DOM 中可能出现的 secret。

## 聚合、缓存与部分失败

`QuotaService` 按完整 scope/window/unit 缓存并对同 key 并发刷新去重。候选 adapter 并发执行，选择可信度最高且更新较新的快照；其余错误作为 typed partial failures 保留。单窗口失败不影响其他窗口。全部新鲜来源失败时可返回旧缓存，但必须同时标记 `served_stale` 与 `provenance.stale`。

取消只取消当前等待者；共享刷新不由第一个 caller 的取消令牌拥有。singleflight leader 被 abort/drop 后，follower 必须能接管并有界完成，不能永久等待。

## Ledger 派生、预测与预算

`LedgerQuotaAdapter` 直接查询同一个 P18-8 `UsageLedger`（当前生产为 `InMemoryUsageLedger`，P18-8 持久化实现注入后行为不变），按 tenant/account/credential/provider/model、币种和半开时间范围过滤。重复 replay 使用稳定 record ID 幂等；相同 ID 不同内容报冲突。Rolling5h 与 Weekly 按滚动时间范围派生，当前 `Monthly` 派生采用 30 天近似并把 reset 标为 uncertain；远端 exact 月度仍遵从供应商日历月。

RunSupervisor 在终态汇总 Provider usage，并先以稳定 record ID 写入 Ledger；成功写入后再刷新本地额度缓存。每条 record 同时投影完整 credential/model scope 与 account 级聚合 scope，分别生成 Overall / Rolling5h / Weekly / Monthly 的 Token 与 Cost 快照。这样显式过滤查询能看到完整 scope，account 级聚合查询能看到账号总量；Ledger 或缓存刷新失败只记录脱敏告警，不改变 run 终态。归属维度（tenant/principal/account/credential）目前是 synthetic 默认值，真实归属由 P18-2/3/4 注入。

用量增长率可生成耗尽时间预测及置信度。只有 fresh `Exact` 且明确耗尽的信号可触发 Agent Engine 硬停止；Derived、Scraped 或 stale 信号只产生软告警，避免低可信度数据误杀运行。

## 查询、展示与权限

`AppQuery::QuotaOverview` 经 app-service 执行同步、cache-only 查询，不在 GUI/CLI 请求路径阻塞外网。**provider 必填**：请求必须携带非空 `provider_id`（缺省或空字符串返回 validation error，不再选择“第一个已注册 provider”）；tenant/account 默认 legacy 作用域 `local` / `local/default`，可选 credential/model/windows；响应包含脱敏 credential hint、窗口值、reset、source、confidence、stale 和 partial failures。多 provider/多模型批量聚合待 P18 binding enumeration 成为事实源后实现。

`pawork usage` 提供文本与 JSON 输出，`--window` / `--unit` 使用 clap typed enum（无效值在参数边界拒绝），`--provider` / `--credential` / `--model` 过滤；未指定 provider 时服务端返回明确错误，不做默认选择。

本地 CLI 使用 legacy scope `tenant=local, account=local/default`；系统内部可查询任意 scope；Remote GUI、automation、plugin 与 MCP 需要显式 quota read grant。每次 Ledger 投影成功刷新缓存后，app-service 在对应 run stream 发布完整 model scope 的 `QuotaChanged`；刷新调度器通过 `RunSupervisor::alert_sink()` 在 Global stream 发布 `QuotaAlert`（scheduler 生产接线待 P18-14）。两类事件都消费同一 core-api 类型，生成的 TypeScript schema 由 `schema-typegen --check` 防漂移。`QuotaAlert.kind` / `source` 为 Option：旧事件 JSON 缺省可解码为 `None`（重放兼容），新事件总是 `Some`（source 已脱敏）。

## 自动刷新、退避与告警

刷新按 target 独立运行并支持取消。401/重新授权、403、429、timeout/transient、Unsupported 使用不同状态：只有可重试错误进入带 jitter 的有界指数退避，429 同时尊重 `Retry-After`。刷新失败可以降级到 cache/Ledger，但来源与 stale 必须可见。

`RefreshScheduler` 提供自动循环、幂等 target 注册和手动触发；target 包含 adapter、credential resolver、scope/window/unit 与刷新策略。具体远端 target 由账号 / 凭据控制面按绑定关系注册（P18-3/4），Phase 14 不复制账号选择逻辑；生产 composition root 尚未启动 scheduler（P18-14）。当前未注册远端 target 时，查询与 Ledger 投影仍完整可用，且不会在交互查询路径发起网络请求。

阈值告警按 scope/window/threshold/confidence 去重；Scraped 告警升级为 Exact 时允许重新通知，恢复到阈值上方后清除去重状态。只有 fresh Exact 告警可以标为非 advisory，并在事件桥中映射为 Critical；advisory 阈值与 stale/部分失败为 Warning，恢复为 Info，需重新授权为 Critical。刷新、部分失败、重新授权、阈值触发和恢复均写脱敏审计（canonical audit 接线待 P18-13）。

## 验收标准

- 六供应商 contract fixtures 覆盖成功、Unsupported 与关键错误口径，且文档能力矩阵与 adapter `supports()`（可执行事实源）一致。
- tenant/account/credential 隔离、401/403/429、重新授权状态机、窗口 reset、缓存优先级、部分失败和取消并发均有定向测试。
- `QuotaOverview` 缺省 provider 被拒绝；`QuotaAlert.kind`/`source` 旧 JSON 解码为 `None`、新事件为 `Some`（脱敏）。
- Ledger replay 不重复累计，失败/取消运行仍提交已消费用量，预算只对 fresh Exact 执行硬停止。
- WebScrape 受 opt-in、版本、并发最小间隔、TTL 和脱敏审计约束。
- CLI、core-api 与 GUI Protocol 输出不含明文凭据，Rust/TypeScript schema 一致。

## 优先级（P0–P2）

- **P0**：P18-8 持久化 Ledger 注入与启动 replay；P18-2/3/4 真实归属；P18-14 生产 scheduler/target 生命周期。
- **P1**：P18 binding enumeration 后 `QuotaOverview` 全绑定聚合；P19-2/10 Desktop 投影与页面；P18-13 统一审计（WebScrape 内置 Vec 并入外部 sink）。
- **P2**：API Key 通用层在真实接线后决定内联或保留（当前仅 Moonshot 单消费者）。

## 相关文档

- [providers](providers.md) · [provider-control-plane](provider-control-plane.md) · [tenant-audit](tenant-audit.md) · [models](models.md) · [auth](auth.md) · [context](context.md) · [observability](observability.md)
- [workspace-layout](../architecture/workspace-layout.md)（`quota-service`）
- [p14-review](../review/p14-review.md)
- [ROADMAP Phase 14 / Phase 18](../../ROADMAP.md)
