# P18-9：Tenant Policy / RBAC

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟠核心接线已实现，专项回归待补 · 交付成熟度：Built · 依赖：P18-2、P18-3、P18-8、P4-9

**最终目的**：在共享账号池、Agent 与查询面前建立 deny-first tenant policy，限制可用 provider/model/account、并发、预算、保留与审计导出。

**涉及范围**：`tenant-service`；`policy-engine` 组合接口；`core-api` 管理/查询；route/lease/agent/session/audit enforcement

## 细分步骤

1. **PolicySet** —— 定义 allowed providers/models/accounts、max concurrent agents/requests、daily token/cost budget、permission profile、retention、audit export；目的：首轮最小 RBAC。
2. **Principal role** —— local user/service/admin/viewer 等最小角色与 deny-first 合并规则；目的：操作人与执行 Agent 分离。
3. **强制入口** —— route candidate、lease acquire、Agent spawn、Session/Usage/Audit query 全部执行 policy；目的：无旁路。
4. **决策事件** —— allow/deny/limit/fallback 记录 versioned、脱敏 decision reason；目的：可解释审计。
5. **隔离/预算测试** —— cross-tenant access、并发和 token/cost budget、adapter/GUI 无法覆盖 deny；目的：安全闭环。

## 主要产出物

- TenantPolicy / PrincipalRole / policy repository
- 五类入口的 enforcement 接线
- RBAC、预算、跨租户隔离测试

## 验收标准

- [ ] Tenant A 无法使用或观察 Tenant B 的 account/session/agent/usage/audit
- [ ] deny 优先，adapter/GUI/plugin 不能覆盖 Core policy
- [ ] 并发与日 token/cost budget 在租约/Agent 准入前执行
- [ ] 未配置用户继续使用 `local/default` 默认 policy

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [policy](../docs/features/policy.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)

## 当前进度（2026-08-13）

- 已接入 provider/model/account allowlist、RBAC、run/agent 并发准入、retry 重新检查、tenant-scoped GUI/query、Lease 返回作用域校验与 canonical policy audit。
- `cargo test -p app-service --test tenant_policy`：14 passed；`cargo test -p app-service --lib`：99 passed；`cargo test -p gui-server --lib`：22 passed；`cargo test -p orchestration --lib`：78 passed。
- 待补：并发 spawn 压力回归、恶意/故障 pool 返回错配 lease 的专项回归，以及完成后的 GLM 审查。
