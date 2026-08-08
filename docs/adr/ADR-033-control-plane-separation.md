# ADR-033：Provider、Account、Agent 与 Client Protocol 控制面分离

- **状态**：Accepted
- **日期**：2026-08-09

## 背景

Pawork 已有 canonical `ModelProvider`、Provider Runtime、Session/Event Store、Policy 与 Multi-Agent 规划，但尚未形成多账号租约、确定性路由、多租户隔离和外部 Agent Client 协议适配的统一边界。如果把 provider 选择、credential 轮换、Agent 调度和客户端会话同步塞入同一个 router，会把不同失败域和状态机耦合：客户端取消可能错误惩罚账号，协议不兼容可能轮询全部 credential，账号 429 也可能污染 Agent 生命周期。

## 决策

采用以下单向分层：

```text
ClientAdapter
    ↓
SessionRegistry / AgentSupervisor
    ↓
RoutePlanner + TenantPolicy
    ↓
RoutingPolicy
    ↓
CredentialPool
    ↓ CredentialLease
ModelProvider（保持现有 contract）
```

交叉能力由 `UsageLedger`、`AuditEvent`、`SecretBackend` 与 `CapabilityRegistry` 提供，不在任一 adapter 内私有实现。

明确保持和新增的契约：

- `provider-api::ModelProvider` 与 `EmbeddingProvider` 保持不变，只负责 canonical 模型能力和协议调用，不承担账号选择、租户、计费或 Agent 调度。
- 新增 `CredentialPool`：按 `AcquireRequest` 获取租约，并在 `release(LeaseOutcome)` 时更新并发和健康状态；Agent 与 Client 不接触 API key。
- 新增 `RoutingPolicy`：只对已通过 capability、tenant policy 与 health 过滤的候选排序，支持 `SingleCandidate`、priority、weighted round robin、fill-first 和 session affinity。
- 新增 `ErrorClassifier`：把 HTTP/协议/Provider 特有错误归一成 `FailureClass + FailureScope + Retryability + HealthImpact + safe_to_failover`；HTTP status 只是输入，不是最终动作。
- `AgentSupervisor` / `TaskGraph` 继续由 Phase 12 的 `orchestration` 承载，不在 Provider control plane 重建。
- 新增 `ClientAdapter` / `ClientAdapterFactory`：只负责协议解码、编码、版本协商、能力映射、客户端身份提取和事件翻译；不持有 Provider credential，不把客户端专有 JSON 泄漏进 `agent-engine`。
- `SessionRegistry` 显式保存 client/core session 映射、`ownership_epoch`、`last_seen_revision` 与 capability snapshot；共享磁盘或相同 session id 不等于跨进程 ownership。

所有新 schema 与 canonical event 带版本。持久化实体必须包含 `tenant_id` 或有明确的 tenant side table；旧数据迁移到 `tenant_id = local/default`、`principal_id = local/user`。旧单 credential 配置自动包装为 synthetic `ProviderAccount(default)` / `Credential(default)`，默认策略为 `SingleCandidate`。

## 错误与回退边界

回退动作必须区分：retry same credential、failover credential、fallback model、fallback provider、fallback protocol。`ClientCancelled`、`InvalidRequest`、`ContextTooLarge` 与 `ProtocolIncompatible` 不得默认触发 credential rotation；`Cancelled` 不降低账号健康度。

## 后果

- 新增 `provider-control`、`tenant-service`、`client-adapter-api`、`client-codex-app-server`、`client-claude-gateway` 等计划 crate；现有 `acp-host` 改为实现统一 ClientAdapter 契约。
- Phase 14 的 quota 适配器继续负责额度观测，P18 Usage/Cost Ledger 负责 tenant/account/session/agent 多维持久账本；两者互补，不重复。
- Phase 12 的 Agent 生命周期只通过 `AcquireRequest` 消费 Provider 资源；账号失败不直接改变 Agent lifecycle，Agent cancel 不直接改变账号 health。
- 分布式 scheduler、自动账号 provisioning、完整支付/开票与全功能 Web Control Plane 明确延后，不进入首轮实现。

## 相关

- [Provider Control Plane](../features/provider-control-plane.md) · [Client Adapters](../features/client-adapters.md) · [Tenant、Usage 与 Audit](../features/tenant-audit.md)
- [ADR-002 Agent Engine 与 Provider 解耦](ADR-002-agent-engine-provider-decoupled.md) · [ADR-014 Secret 存 OS Keychain](ADR-014-secret-os-keychain.md) · [ADR-016 事件持久化重放](ADR-016-core-event-persist-replay.md) · [ADR-030 Core 单一事实源](ADR-030-core-sole-source-of-truth.md)
- [ROADMAP Phase 18](../../ROADMAP.md)
