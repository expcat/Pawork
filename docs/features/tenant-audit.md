# Tenant、Usage Ledger 与 Audit

## 职责

为共享 Provider Account、Session、Agent、用量与审计建立明确的租户边界。`tenant_id` 表示组织/逻辑租户，`principal_id` 表示当前用户或服务账号，`agent_id` 表示执行者；三者不得由同一个 API key hash 代替。

## 身份与兼容

```text
未配置 tenant 的本地用户
    tenant_id    = local/default
    principal_id = local/user
```

旧 session、credential 与 usage 通过 versioned migration 补 tenant side table 或 nullable/versioned column；迁移失败整批回滚。Secret 仍只由 SecretBackend 解析，tenant 数据库只保存 `secret_ref` 与脱敏 metadata。

## Tenant Policy

最低策略字段：allowed providers/models/accounts、max concurrent agents/requests、daily token/cost budget、permission profile、data retention、audit export。策略在 route candidate、credential acquire、Agent spawn、Session/Audit query 四个入口强制执行，deny 优先；adapter 或 GUI 不能覆盖 Core 决策。

## Usage / Cost Ledger

P18 Ledger 记录 canonical `UsageRecord`，至少含：`tenant_id`、`principal_id`、`account_id`、`credential_id`、`session_id`、`agent_id`、`provider_id`、`model_id`、tokens/cache/cost、trace 与时间。Phase 14 Quota 负责外部额度快照、窗口聚合和展示；Ledger 负责本地不可变归属、预算与后续 chargeback，两者不互相冒充。

## Audit Event

```rust
pub struct AuditEventV1 {
    pub tenant_id: TenantId,
    pub principal_id: PrincipalId,
    pub agent_id: Option<AgentId>,
    pub actor: AuditActor,
    pub action: AuditAction,
    pub target: AuditTarget,
    pub decision: AuditDecision,
    pub trace_id: TraceId,
    pub occurred_at: DateTime<Utc>,
}
```

审计覆盖身份解析、policy decision、route decision、lease acquire/release/rebind、Agent lifecycle、permission/tool、配置变更和数据导出。OTel/SIEM exporter 只能输出 allowlist 字段；凭据、prompt、tool output 与 Protected Blob 明文默认不导出。

## 优先级

- P0：`local/default` migration、tenant-scoped query、route/lease/session/agent/audit 隔离、多维 usage ledger。
- P1：RBAC 管理面、rate card/tenant chargeback、OTel exporter、retention enforcement。
- P2：完整 billing/payment/invoice 与跨组织 federation，不在首轮范围。

## 验收标准

- Tenant A 无法获取 Tenant B credential、session、agent transcript、affinity binding、usage 或 audit
- 所有新持久化实体与事件带 schema/event version，并能从 legacy local user 无损迁移
- usage 可按 tenant/account/session/agent 归属且总量可对账
- 审计导出脱敏、可重放，权限拒绝与 fallback 决策可解释
- tracing span 至少携带 tenant/session/agent/provider/account/trace 维度

## 相关文档

- [auth](auth.md) · [usage-quota](usage-quota.md) · [observability](observability.md) · [policy](policy.md)
- [ADR-016 事件持久化重放](../adr/ADR-016-core-event-persist-replay.md) · [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md)
- [ROADMAP Phase 18](../../ROADMAP.md)
