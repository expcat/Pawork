# P18-14：Provider Registry / Pool Reconciliation / Hot Reload

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-4～P18-7、P18-10、P8-8

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

- [ ] stale lease 可幂等回收，运行中 lease 不被热切换误杀
- [ ] Provider factory 可动态注册/禁用且 core 不出现 Provider 名称分支
- [ ] probe 有独立并发/频率/预算且失败不形成雪崩
- [ ] 无效配置保持旧配置原子有效，不出现半应用状态
- [ ] account/capability 变化后 binding 安全迁移并有 audit event
- [ ] 六家远端 quota adapter factory 注册为 `RefreshScheduler` 生产 targets；scheduler 在生产 composition root 启动、取消与关闭（复用 QuotaRuntime 为生命周期 owner，不新增 manager/daemon）
- [ ] quota target registry 与 provider/account registry 对账：binding/account 变化时 reconcile target 不悬挂，热切换不误杀运行中 lease

**相关文档**：[provider-control-plane](../docs/features/provider-control-plane.md) · [client-adapters](../docs/features/client-adapters.md) · [P8-8](P8-8-hot-reload.md) · [ROADMAP](../ROADMAP.md)
