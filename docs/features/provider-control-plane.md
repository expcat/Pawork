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
    async fn release(&self, lease: CredentialLease, outcome: LeaseOutcome);
}

pub trait RoutingPolicy: Send + Sync {
    fn rank(
        &self,
        ctx: &RouteContext,
        candidates: &[RouteCandidate],
    ) -> Result<RouteDecision, RouteError>;
}

pub trait ErrorClassifier: Send + Sync {
    fn classify(&self, failure: &UpstreamFailure) -> ClassifiedFailure;
}
```

最低实体：`ProviderAccount`、`Credential`（仅 `secret_ref`）、`CredentialLease`、`HealthState`、`RoutePolicy`、`SessionBinding`。所有持久化记录带 `schema_version` 与 `tenant_id`；active lease 的计数和回收必须在 SQLite Actor/运行时状态机中原子更新。

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
