# P18-18：Route / Health / Binding Runtime Control Loop

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P18-5～P18-7、P18-14、P18-17

**最终目的**：把已验证的 Health、Route、Session Binding 与 Reconciler 从库层接成正式 run 控制环：候选使用真实 model capability、健康与活跃 lease，路由选中的 credential 只被消费一次，失败反馈改变后续选择，Session affinity 与 lease 生命周期保持一致。

**涉及范围**：`app-service` run supervisor、`provider-control` routing/health/binding/reconciler、`core-runtime` 周期任务、model registry、credential pool、quota refresh scheduler。

## 细分步骤

1. **完整 Route 输入** —— 从 model registry、pool 与 account repository 提供 capability/token、active lease、并发和策略；不以 `u64::MAX` / 固定 0 伪造未知能力。目的：过滤与策略选择基于真实事实。
2. **Health feedback** —— 正式宿主持有共享 `HealthRuntime`，Provider 分类结果更新 account/credential/model/provider scope，route 使用同一 HealthView。目的：失败可解释地影响下一次准入与 failover。
3. **单次 credential 选择** —— route winner 的 account + credential 进入 `AcquireRequest` / lease，pool 校验绑定后直接授予，不再二次独立挑选。目的：决策、审计与实际使用一致。
4. **Binding / Reconciler 生命周期** —— 构造 `SessionBindingService`，复用或 rebind 时按 ownership/revision 释放旧 lease并写 `LeaseRebound`；宿主启动/停止 Reconciler、Probe 与 quota refresh scheduler。目的：长驻状态收敛且可关停。

## 主要产出物

- Route → Health → Binding → Lease 正式主链
- Provider failure → health → subsequent route 的端到端回归
- selected credential 一次性透传与多账号策略回归
- Reconciler/Probe/Quota scheduler 生命周期与 shutdown tests

## 验收标准

- [ ] RouteCandidate 不使用伪造的 token 上限或固定 active lease；缺 catalog 时显式 fail-closed/unsupported
- [ ] HealthRuntime 被正式 route 消费，account/credential/model/provider 四 scope 的失败反馈可改变下一次选择
- [ ] route 选中的 credential_id 与最终 lease 完全一致，repository 只做一次选择且审计可对账
- [ ] Priority/Weighted/FillFirst 与策略冲突、多账号轮换、并发满、健康拒绝有生产链定向回归
- [ ] Session affinity/rebind、`LeaseRebound`、旧 lease 释放与 restart/reconcile 可证
- [ ] Reconciler/Probe/Quota scheduler 由唯一宿主持有，start/cancel/shutdown 无悬挂任务

**相关文档**：[P18 Review](../docs/review/p18-review.md) · [routing](P18-6-routing-policy.md) · [session affinity](P18-7-session-affinity.md) · [pool reconciliation](P18-14-pool-reconciliation.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md)
