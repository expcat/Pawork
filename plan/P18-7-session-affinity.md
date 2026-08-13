# P18-7：Session Affinity / Binding

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P18-4、P18-6、P5-3、P18-2

**最终目的**：让健康 session 在请求间稳定复用 account/model，同时在 cooldown、禁用或能力变化后安全 rebind，且不跨 tenant 复用粘性。

**涉及范围**：`provider-control::binding`；`session-store` projection；route/lease events

## 细分步骤

1. **Binding 模型** —— 定义 session/agent → provider/model/account、tenant、TTL、capability hash、revision、ownership epoch；目的：粘性有显式状态。
2. **稳定命中** —— 健康且能力未变时 deterministic reuse；目的：状态型协议不被请求级轮转破坏。
3. **安全 rebind** —— account unavailable、policy/capability 改变或 TTL 到期时先 CAS 更新 revision，再获取新 lease；目的：避免并发双绑定。
4. **生命周期与迁移** —— Unbound/Bound/Rebinding/Released 事件化，支持配置热切换与 crash replay；目的：可恢复。
5. **隔离/并发测试** —— 覆盖 affinity stickiness、cooldown rebind、跨 tenant 禁止复用、并发 ownership 冲突；目的：锁定一致性。

## 主要产出物

- `SessionBinding` projection 与 affinity policy
- revision/ownership epoch 的原子 rebind
- stickiness/rebind/isolation/concurrency tests

## 验收标准

- [x] healthy affinity 在重复请求间稳定
- [x] account unavailable 后只发生一次安全 rebind，不重复占用 lease
- [x] capability/policy hash 改变会使旧 binding 失效
- [x] Tenant A 永不复用 Tenant B 的 affinity binding

## 验证记录（2026-08-13）

- `provider-control::binding` 已交付 tenant-scoped affinity、CAS revision / ownership epoch、TTL/capability/policy 失效、rebind/release/GC 与恢复语义。
- `session-store::binding` 已交付 schema v9 持久化与严格事件 replay；重复同版本同内容幂等跳过，同版本冲突、版本跳跃与缺失版本 fail-closed。
- `cargo test -p provider-control binding`：34 passed；`cargo test -p session-store binding`：9 passed。
- DeepSeek 审查发现的 repair replay 重复事件问题已修复并回归通过。

**相关文档**：[sessions](../docs/features/sessions.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [client-adapters](../docs/features/client-adapters.md) · [ROADMAP](../ROADMAP.md)
