# P18-9：Tenant Policy / RBAC

> Phase 18 · Account Control Plane & Client Adapters · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P18-2、P18-3、P18-8、P4-9

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

- [x] Tenant A 无法使用或观察 Tenant B 的 account/session/agent/usage/audit
- [x] deny 优先，adapter/GUI/plugin 不能覆盖 Core policy
- [x] 并发与日 token/cost budget 在租约/Agent 准入前执行（orchestration 与 app-service `RunSupervisor` 均执行 `max_concurrent_agents` + `max_concurrent_requests`）
- [x] 未配置用户继续使用 `local/default` 默认 policy

**相关文档**：[tenant-audit](../docs/features/tenant-audit.md) · [policy](../docs/features/policy.md) · [ADR-033](../docs/adr/ADR-033-control-plane-separation.md) · [ROADMAP](../ROADMAP.md)

## 当前进度（2026-08-13）

- 已接入 provider/model/account allowlist、RBAC、run/agent 并发准入、retry 重新检查、tenant-scoped GUI/query、Lease 返回作用域校验与 canonical policy audit。
- 专项回归已补：并发 spawn 压力（`max_concurrent_agents` + pool 并发，JoinSet，无超配、预约可回收）与恶意/故障 pool 返回错配 lease（tenant/principal/session/agent/provider/account；fail-closed `PolicyDenied`、`LeaseOutcome::Released`、无活动 worker / 悬挂预约）。
- 生产最小修复：lease 作用域校验失败由 `SupervisorError::LeaseError` 改为 `PolicyDenied`（与 deny-first 闸口一致，不惩罚账号健康）。
- app-service 缺口已补：`RunSupervisor::enforce_run_admission` 在同一 `inner` 锁内对 `active_for_tenant` 同时执行 `decide_agent_concurrency` 与 `decide_request_concurrency`（`None` 不限制；`current >= max` deny-first；agent 拒绝原因含 `agent 并发`，router 记 `PolicyGate::AgentSpawn`）。
- L1 证据（2026-08-13）：`cargo test -p app-service --test tenant_policy`：16 passed（0 failed / 0 ignored），含 `run_start_enforces_agent_concurrency_limit_at_boundary` 与 `concurrent_run_start_has_exactly_two_winners_at_agent_limit_two`；`cargo test -p app-service --lib`：103 passed。先前：`cargo test -p orchestration --lib`：81 passed；`cargo test -p gui-server --lib`：22 passed。`Full workspace gate: NOT RUN`。clippy `-p app-service --all-targets` 因 `provider-control` 既有失败未作为本任务门禁。
- GLM 审查交由后续审查者。
- 剩余非阻塞项：app-service 层恶意 pool 错配 lease 回归依赖 `LeaseGuard` 公开构造器，待 P18-14 释放 `provider-control` 写入后再补。
