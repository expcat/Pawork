# P18-4：CredentialPool / Lease 与并发准入

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-3、P2-10、P1-4

**最终目的**：用可释放、可过期、可幂等回收的 `CredentialLease` 代替“每次从 credential 数组挑一个”，确保 per-account concurrency 和取消语义正确。

**涉及范围**：`provider-control`；`app-database` lease projection；`core-runtime` 装配；Mock Provider 测试

## 细分步骤

1. **Pool contract** —— 实现对象安全 `acquire(AcquireRequest)` / `release(lease, LeaseOutcome)`；目的：统一账号资源入口。
2. **Lease 状态机** —— Requested/Acquired/Released/Expired/Reclaimed 全部 versioned/evented；目的：支持崩溃恢复与审计。
3. **并发准入** —— 以原子事务维护 active lease，超过 account/tenant limit 返回可解释 admission error；目的：永不超配。
4. **取消与 Drop** —— Cancelled 不记 health failure；进程恢复扫描过期 lease 并幂等 reclaim；目的：不泄漏并发额度。
5. **并发测试** —— tokio 多任务 + proptest 覆盖 acquire/release/cancel/drop/restart；目的：证明 invariant。

## 主要产出物

- `CredentialPool` / `CredentialLease` / `LeaseOutcome`
- 并发 admission 与 lease recovery projection
- 并发、取消、重启回收测试

## 验收标准

- [ ] active lease 永不超过 configured account concurrency
- [ ] release/reclaim 幂等，cancel/drop/restart 后无永久泄漏
- [ ] `LeaseOutcome::Cancelled` 不降低 account health
- [ ] Agent/Client 只能获得 lease，不能读取持久 Secret

**相关文档**：[provider-control-plane](../docs/features/provider-control-plane.md) · [multi-agent](../docs/features/multi-agent.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)

