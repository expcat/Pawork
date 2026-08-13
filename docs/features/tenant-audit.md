# Tenant、Usage Ledger 与 Audit

## 职责

为共享 Provider Account、Session、Agent、用量与审计建立明确的租户边界。`tenant_id` 表示组织/逻辑租户，`principal_id` 表示当前用户或服务账号，`agent_id` 表示执行者；三者不得由同一个 API key hash 代替。

## 身份与兼容

```text
未配置 tenant 的本地用户
    tenant_id    = local/default
    principal_id = local/user
```

`account_id` 的兼容默认值同样是 `local/default`。历史 Quota 查询仍保留其既有 `tenant=local` wire 兼容常量；Control Plane 身份不得复用该旧常量，迁移与新持久记录统一使用上述 `local/default` tenant。

旧 session、credential 与 usage 通过 versioned migration 补 tenant side table 或 nullable/versioned column；迁移失败整批回滚。Secret 仍只由 SecretBackend 解析，tenant 数据库只保存 `secret_ref` 与脱敏 metadata。

P18-2 已将 `IdentityContext { tenant_id, principal_id }` 固定为 Core 命令边界：`LocalUser` 映射 `local/user`，认证客户端、Automation、Plugin 与 MCP 分别使用带类型前缀的稳定 principal，`System` 映射 `local/system`；缺失或空白 identity 一律 fail-closed。外部 resolver 的返回值也必须再次校验，不能以自定义 resolver 绕过该边界。

Session Store schema v7 在 session 行持久化 tenant/principal，并把 legacy 行回填为 `local/default` / `local/user`。Session export schema v3 显式携带两者；v1/v2 导入只能采用安全 legacy 默认值，v3 缺字段或 import 调用方 identity 不匹配时拒绝。Session/Run 的读取、取消、重试、审批与 snapshot 投影均按 tenant 过滤；命令 ID 和 idempotency key 同样以 tenant 为命名空间，跨 tenant 相同 key 不会互相 replay。

GUI 连接在认证握手后解析并固定连接级 `IdentityContext`；初始 Snapshot、显式 SnapshotRequest 与 Resume 降级快照都复用该身份并调用 tenant-scoped projection。身份解析失败直接关闭连接，不允许退回全局 snapshot。本地 GUI 默认映射 `local/default`，未来远程 adapter 通过连接身份 resolver 注入真实 tenant。

当前 P18-2 只闭合 tenant/principal 基线。account/credential 来自 P18-3/4，唯一持久 Usage Ledger 来自 P18-8，canonical Audit Event 来自 P18-13；这些任务必须复用同一 `IdentityContext`，不得另建平行身份状态。

## Tenant Policy

最低策略字段：allowed providers/models/accounts、max concurrent agents/requests、daily token/cost budget、permission profile、data retention、audit export。策略在 route candidate、credential acquire、Agent spawn、Session/Audit query 四个入口强制执行，deny 优先；adapter 或 GUI 不能覆盖 Core 决策。

## Usage / Cost Ledger

P18 Ledger 记录 canonical `UsageRecord`，至少含：`tenant_id`、`principal_id`、`account_id`、`credential_id`、`session_id`、`agent_id`、`provider_id`、`model_id`、tokens/cache/cost、trace 与时间。Phase 14 Quota 负责外部额度快照、窗口聚合和展示；Ledger 负责本地不可变归属、预算与后续 chargeback，两者不互相冒充。

P14-7 只扩展并消费这一个 Ledger：record ID 的幂等命名空间至少包含 tenant/account；相同 ID 与相同内容 replay 为 no-op，相同 ID 与不同内容显式冲突。complete、failed 与 cancelled 终态都必须提交已发生的用量，提交失败保留稳定记录供重试。Quota 派生查询沿 tenant/account/credential/provider/model 与币种过滤，不得把另一个账号的记录纳入窗口。

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

Quota 刷新、WebScrape、部分失败、重新授权、阈值触发与恢复同样进入审计。允许记录 provider、account 的脱敏提示、window、adapter kind、confidence、HTTP 错误类别与 selector version；禁止记录 API Key/token/cookie、URL query、认证 header、原始响应正文或 HTML。

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
