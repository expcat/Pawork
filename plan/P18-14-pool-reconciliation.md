# P18-14：Provider Registry / Pool Reconciliation / Hot Reload

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢库层有界完成 · 交付成熟度：LibraryBuilt（Reconciler / Probe / Quota target 已验证，生产 composition 未装配） · 依赖：P18-4～P18-7、P18-10、P8-8

**最终目的**：以 Provider factory registry 消除组合层硬编码，并在被动 cooldown 之外补齐过期 lease 回收、主动健康探测和事务式配置热切换，使 Provider/账号/capability 变化不会留下悬挂 binding 或半应用配置。

**涉及范围**：`provider-control::reconciler`；`config-service` / resource watch；`session-store` binding migration；Mock health probes

## 细分步骤

1. **Provider factory registry** —— Provider adapter 通过 descriptor/factory 注册，组合层按 capability/config 创建实例；目的：禁止在 core 用 Provider 名分支装配。
2. **Reconciler loop** —— 扫描过期 lease、stale health、disabled account 与 binding；目的：长期运行状态自愈。
3. **主动健康探测** —— 按 Provider capability 配置 synthetic probe、频率/预算/退避，默认关闭高成本 probe；目的：识别恢复而不烧额度。
4. **事务式热切换** —— parse/validate/stage/commit，失败回滚旧 registry/pool 配置；目的：避免部分切换。
5. **Binding migration** —— policy/account/capability hash 变化后安全 rebind，运行中请求保留旧 lease 到 release；目的：不中断或串错会话。
6. **故障注入测试** —— duplicate/unknown factory、invalid config、probe storm、restart、lease expiry、并发 reload/rebind；目的：证明可恢复。

## 主要产出物

- Provider factory registry + pool reconciler + 可配置 health probe
- transactional config reload / rollback
- session binding migration 与故障注入测试

## P14 现状与登记（2026-08-11）

P14-5/9 的六家远端 quota adapter factory 与 `RefreshScheduler` / `AuditSink` / `AlertSink` 只在 quota-service 测试中闭环；`QuotaRuntime::production` 仅注册 `LocalLedger` 适配器，无 scheduler 生命周期与 target 注册（见 [usage-quota](../docs/features/usage-quota.md)）。target 装配与对账由本任务完成。

## 验收标准

- [x] stale lease 可幂等回收，运行中 lease 不被热切换误杀（`PoolReconciler` + `CredentialPool::reclaim_expired`；热切换只换 registry snapshot）
- [x] Provider factory 可动态注册/禁用且 core 不出现 Provider 名称分支
- [x] probe 有独立并发/频率/预算且失败不形成雪崩（`ProbeRuntime` / `ProbeBudget`；昂贵 probe 默认 OFF）
- [x] 无效配置保持旧配置原子有效，不出现半应用状态
- [x] account/capability 变化后 binding 安全迁移并有 binding event（复用既有 affinity 状态机；in-flight 保留旧 lease 至 commit）
- [ ] 六家远端 quota adapter factory 注册为 `RefreshScheduler` **生产** targets；scheduler 在生产 composition root 启动、取消与关闭（复用 QuotaRuntime 为生命周期 owner）——**库层已具备** `QuotaTargetRegistry` + `RefreshScheduler::start/cancel/shutdown`；**生产接线留给编排器**（不改 `app-service` / `apps/pawork`）
- [x] quota target registry 与 account/binding 集合对账：library-level `QuotaTargetRegistry::reconcile` 去掉悬挂 targets（生产侧调用点仍待 composition）
- [~] provider factory `builtin_models()` v2 catalog 并入 model-registry：已加窄钩子 `ProviderCapabilitySource` / `ModelRegistry::merge_provider_models`（无 Provider 名分支）；**composition root 把 factory descriptor 喂进 registry 仍待编排器**
- [x] Provider factory / pool 的实例生命周期与 reasoning scope 对齐：`ProtectorFactory` 按 `(provider_id, session_id)` 构造，禁止跨 Session 共享（P18-14 前期已交付）

**相关文档**：[provider-control-plane](../docs/features/provider-control-plane.md) · [client-adapters](../docs/features/client-adapters.md) · [P8-8](P8-8-hot-reload.md) · [ROADMAP](../ROADMAP.md)

## 当前进度（2026-08-13，P18-14 remaining）

### 已落地（库层）

- 动态 `ProviderRegistry` / immutable snapshot / staged registry，串行化 parse→validate→stage→commit；失败保持旧 snapshot 原子有效。
- **Reconciler loop**（`crates/provider-control/src/reconciler.rs`，feature `account-control-v1`）：host-driven `PoolReconciler::tick` 扫描过期 lease、stale binding、disabled account、stale health，并走既有 `reclaim_expired` / `acquire_binding` / `release_binding`。运行中 `Acquired` lease 不被热切换误杀；stale 对账幂等。
- **主动健康探测**：`ProbeRuntime` + `ProbeBudget`（cheap/expensive 独立并发、频率、每 tick 预算与失败上限）；昂贵 probe 默认 OFF；失败经既有 `HealthRuntime`/`CircuitBreaker`，`safe_to_failover: false`，不雪崩。Probe 只经 `ProviderFactory::health_probe()` 扩展点，Core 无 Provider 名分支。
- **Binding migration**：fingerprint / target 变化走既有 `safe_rebind`（`RebindReason::PolicyChanged`）；in-flight 保留旧 lease 至 commit；产生 `BindingEvent`，不另造 affinity 状态机。
- **故障注入测试**：duplicate/unknown factory、invalid reload rollback、probe storm budget、restart/reclaim、lease expiry vs running lease across reload、concurrent reload/rebind。
- **Quota target registry（library only）**：`crates/quota-service/src/targets.rs` 将六家远端 adapter factory 按 `ProviderId` 登记为 `RefreshScheduler` targets；`reconcile` 去掉悬挂 targets。`RefreshScheduler::start` → `SchedulerHandle::{cancel,shutdown}`；`unregister` / `registered_ids` / `reconcile`。
- **model-registry 窄钩子**：`ProviderCapabilitySource` + `ModelRegistry::merge_provider_models`，同 provider 覆盖 v2 caps，跨 provider skip。不改 `caps()` 签名，不在 Core 做 Provider 名 match。

### 明确延期（非本任务写入集）

- `app-service::QuotaRuntime::production()` 与 `apps/pawork` / `core-runtime` 生产 composition：不把六家 factory 接到生产 scheduler，不启动生产 refresh loop。
- OTel / 指标导出。
- composition root 把 `ProviderFactory::descriptors().builtin_models()` 喂进 `ModelRegistry::merge_provider_source`。
- session-store 未新增 durable scan API（reconciler 使用 `SessionBindingService::load_outstanding()` 内存投影即可）。

### 验证（L1）

- `cargo test -p provider-control`：180 + 10 error_matrix passed。
- `cargo test -p quota-service --lib`：268 passed。
- `cargo test -p model-registry --lib`：新测 `merge_provider_catalog_feeds_v2_caps_into_evidence_without_name_match` passed；既有 `wait_for_probe_dedups_repeated_poll_of_same_waker` 失败（DEBUG eprintln / `Waker::noop`，非本任务引入）。
- `cargo clippy -p quota-service --all-targets -- -D warnings`：通过。
- `cargo clippy -p provider-control --all-targets -- -D warnings`：新代码干净；既有 `commit_rebind` / `rebind_after_release` 触发 `clippy::too_many_arguments`（非本任务引入，未改签名）。
- Full workspace gate: NOT RUN。
