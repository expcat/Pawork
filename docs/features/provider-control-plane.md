# Provider Account Control Plane

## 职责

在 `RoutePlanner` 与现有 `ModelProvider` 之间管理 Provider Account、Credential Lease、路由策略和健康状态。该层回答“本次请求获准使用哪个账号/凭据”，不负责 Agent 生命周期、客户端协议翻译或 Provider wire protocol。

## 设计要点

- Provider Registry / Model Catalog、RoutingPolicy、CredentialPool、AgentScheduler 与 ClientAdapter 是不同 abstraction。
- Agent 只提交 `AcquireRequest`，不得直接读取 Secret；`CredentialLease` 是有期限、可回收、受并发上限约束的运行时能力。
- `release()` 根据 `LeaseOutcome` 更新用量与健康；client cancel 不惩罚账号。
- 过滤顺序固定为 capability → tenant policy → health → priority bucket → affinity → weighted/fill-first → concurrency admission。
- Provider-specific 错误只在 Provider adapter / ErrorClassifier 扩展点内解释，core 不按 Provider 名或单一 HTTP status 走特例。

## 接口与数据模型

```rust
#[async_trait]
pub trait CredentialPool: Send + Sync {
    async fn acquire(&self, req: AcquireRequest) -> Result<CredentialLease, PoolError>;
    async fn acquire_guard(&self, req: AcquireRequest) -> Result<LeaseGuard, PoolError>;
    async fn release(
        &self,
        lease_id: LeaseId,
        outcome: LeaseOutcome,
    ) -> Result<ReleaseReceipt, PoolError>;
    async fn reclaim_expired(&self) -> Result<ReclaimReport, PoolError>;
    async fn restore(&self) -> Result<ReclaimReport, PoolError>;
}

impl RoutingPolicy {
    pub fn plan(
        &self,
        ctx: &RouteContext,
        candidates: &[RouteCandidate],
        tenant: &dyn TenantPolicy,
        health: &mut dyn HealthView,
    ) -> RouteDecision;
}

pub trait ErrorClassifier: Send + Sync {
    fn classify(&self, failure: &UpstreamFailure) -> ClassifiedFailure;
}
```

最低实体：`ProviderAccount`、`Credential`（仅 `secret_ref`）、`CredentialLease`、`HealthState`、`RoutePolicy`、`SessionBinding`。所有持久化记录带 `schema_version` 与 `tenant_id`；active lease 的计数和回收必须在 SQLite Actor/运行时状态机中原子更新。

P18-1 已冻结 `account-control-v1`：关闭 feature 时保留 `tenant_id=local/default`、`account_id=local/default`、`principal_id=local/user` 与 `SingleCandidate` synthetic account 回退；开启时控制面记录使用 versioned、unknown-field fail-closed 的 serde 契约。控制面 SQLite migration 使用独立版本账本、整批事务和迁移前备份，不复用其它 schema 的 `user_version`。

P18-3 将控制面 schema 提升到 v2：`ProviderAccountRecord` 独立持有 priority、weight、max concurrency 与 lifecycle state；`CredentialMetadata` 只保存 `secret_ref`、credential kind、expiry 与 refresh state。所有 take 路径先执行统一的 state/refresh/expiry fail-closed gate，之后才允许宿主 resolver 短时解析 `ResolvedCredential`；repository 的管理摘要不暴露 `secret_ref`，SQLite、Event、日志和诊断均不得出现 plaintext。Provider factory 通过 provider-id registry 查找 descriptor/builder，不允许 Core 按 Provider 名称分支。真实 Provider、builtin model 与 persistent protector 的生产宿主组合在 P18-14 统一完成。

P18-5 的错误与健康运行时按 credential/account/model/provider 四类 scope 分离 cooldown 与 circuit；401 只允许 refresh-once，billing blocked 需显式恢复，Retry-After deadline 只能延长不能被旧成功覆盖。half-open probe 是受限在途名额，成功、取消和客户端错误都会归还；只有匹配 scope 的上游故障才重新打开 circuit。默认未知/未识别 4xx fail-closed，不触发无依据的账号轮换或健康惩罚。

P18-6 把 capability → tenant policy → health → priority bucket 固定为不可绕过的候选链，并提供 `SingleCandidate`、严格 `Priority`、普通 Round-Robin、Smooth Weighted Round-Robin 与 Fill-First。Fill-First 只在高 priority 容量耗尽后下沉；其余策略不会穿透最高优先级桶。每次选择、淘汰与 fallback 开关都进入不含 Secret 的 `RouteDecision`。健康预览使用不预留名额的 `can_admit`；只有最终 Route winner 在进入 Lease 前才能通过 `is_admissible` 预留 HalfOpen probe，避免未选候选耗尽探针并发。Session Affinity 仍由 P18-7 的独立 binding 状态机完成，不在路由器内复制粘性状态。

## 状态机

```text
ProviderAccount: Active → CoolingDown → Active
                         ↘ BillingBlocked / Disabled

CredentialLease: Requested → Acquired → Released
                                └──────→ Expired → Reclaimed

SessionBinding: Unbound → Bound → Rebinding → Bound
                           └──────────────→ Released
```

## 错误策略

| 错误 | Scope | 动作 | 账号轮换 |
| --- | --- | --- | --- |
| `AuthRejected` | credential | refresh-once；失败后 cooldown/disable | 是 |
| `PaymentRequired` / `AccountBlocked` | account | billing blocked；failover | 是 |
| `RateLimited` | account/model/provider | 尊重 Retry-After，scope-aware cooldown | 条件式 |
| `QuotaExceeded` | account/model | hard quota → 账号 failover；soft quota → 策略（降级/告警），不与 RateLimited 语义混淆 | 是/策略 |
| `ProviderUnavailable` | provider/account | bounded retry + circuit breaker | 条件式 |
| `ContextTooLarge` | request/model | compact 或选择更大模型 | 否 |
| `ProtocolIncompatible` | protocol | 显式失败或协议降级 | 否盲目轮换 |
| `ClientCancelled` | request/session | 结束请求，不改变 health | 否 |

## 优先级

- P0：synthetic default account 迁移、Lease/并发、ErrorClassifier、priority/weighted/fill-first、affinity 安全 rebind。
- P1：Provider factory registry、主动健康检查、pool reconciliation、配置热切换与 session binding migration。
- P2：成本/延迟自适应路由、跨节点分布式 scheduler、账号自动 provisioning。

## 验收标准

- 旧单 Provider + 单 credential 配置行为不变，默认策略为 `SingleCandidate`
- 同一 account 的 active lease 永不超过 configured concurrency，drop/restart 后可幂等回收
- priority 可 deterministic 测试，weighted routing 有 property test，healthy affinity 稳定且 unavailable 后安全 rebind
- `ClientCancelled`、`ContextTooLarge`、`ProtocolIncompatible` 不错误轮换或惩罚 credential
- Secret 不以 plaintext 进入 SQLite、Event Store、日志或诊断包

## 相关文档

- [providers](providers.md) · [auth](auth.md) · [usage-quota](usage-quota.md) · [tenant-audit](tenant-audit.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ROADMAP Phase 18](../../ROADMAP.md)
