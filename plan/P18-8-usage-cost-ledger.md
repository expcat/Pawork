# P18-8：Usage / Cost Ledger

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢有界完成 · 交付成熟度：HostWired（持久 Ledger 与真实 lease attribution 已进入正式宿主） · 依赖：P18-2、P18-3、P2-9、P2-7、P1-4

**最终目的**：把每次 canonical usage 按 tenant/principal/account/credential/session/agent/provider/model 持久归属，为预算、Quota 对账、审计和后续 chargeback 提供不可变事实源。

**涉及范围**：新增 `usage-ledger`；`agent-events` Usage v2；`model-registry` pricing；`app-database` projection

## 细分步骤

1. **UsageRecord v2** —— 定义 tokens/cache/cost/unit/currency、全部身份维度、trace 与 provenance；目的：归属粒度完整。
2. **不可变写入/幂等** —— 使用 provider request/event id 去重，append 后投影聚合；目的：重放不重复计费。
3. **定价快照** —— 记录 rate-card/version 与估算/实收 provenance；目的：历史费用不因模型价格更新漂移。
4. **预算/Quota API** —— 为 P3-6 与 P14-7 提供 tenant/account/session/agent/window 聚合；目的：统一事实源，不复制计数器。
5. **对账与隔离测试** —— 覆盖 stream usage 更新、retry/failover、cache token、重放幂等、跨 tenant query；目的：总量可核对。

## 主要产出物

- `usage-ledger` crate + UsageRecord v2 schema/projection
- pricing snapshot 与多维聚合 API
- replay/idempotency/reconciliation/isolation tests

## P14 现状与登记（2026-08-11）

P14-7/8 本地 Ledger 为 `InMemoryUsageLedger`：`QuotaRuntime::production` 每次 CLI 进程新建，`pawork run` 的用量无法跨进程被 `pawork usage` 读取。`LedgerQuotaAdapter` / `refresh_local_cache` 的投影与对账已就绪，只等持久化账本注入（见 [usage-quota](../docs/features/usage-quota.md)）。

## 验收标准

- [x] usage 可按 tenant/account/session/agent 对账且重放不重复累计
- [x] retry/failover 的每次实际上游调用均可归属，客户端只看到合适的汇总
- [x] rate-card/version 与 confidence 可追溯
- [x] Phase 14 消费 ledger，不另建冲突的本地 usage 事实源
- [x] `QuotaRuntime::production` 注入持久化 `UsageLedger` 并在启动时 replay；`pawork run` 后新进程 `pawork usage` 可读到幂等记录
- [x] 本地 Quota 投影（`LedgerQuotaAdapter` / `refresh_local_cache`）消费同一持久账本，不另建第二套累计事实源

## 验证记录（2026-08-13）

- Validation Level: L1
- Affected crates: `usage-ledger`、`app-service`、`core-runtime`、`pawork`，以及 P14 quota 消费链
- Validated: `cargo test -p usage-ledger`（36）；`cargo test -p app-service`（54 unit + 46 integration）；`cargo test -p core-runtime`（13）；`cargo test -p pawork --test cli`（4）；相关 Clippy/fmt；独立 DeepSeek reviewer PASS
- Targeted regressions: immutable/idempotent/conflict、request/attempt 去重、retry/failover 实际调用归属、pricing snapshot、currency-isolated replay、tenant 隔离、SQLite v2→v3、跨进程 RunId 唯一与累计 300、不兼容 schema 启动 fail-loud
- Full workspace gate: NOT RUN（未命中升级条件）

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [usage-quota](../docs/features/usage-quota.md) · [models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)
