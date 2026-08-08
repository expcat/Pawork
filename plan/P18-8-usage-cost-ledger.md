# P18-8：Usage / Cost Ledger

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-2、P18-3、P2-9、P2-7、P1-4

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

## 验收标准

- [ ] usage 可按 tenant/account/session/agent 对账且重放不重复累计
- [ ] retry/failover 的每次实际上游调用均可归属，客户端只看到合适的汇总
- [ ] rate-card/version 与 confidence 可追溯
- [ ] Phase 14 消费 ledger，不另建冲突的本地 usage 事实源

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [usage-quota](../docs/features/usage-quota.md) · [models](../docs/features/models.md) · [ROADMAP](../ROADMAP.md)

