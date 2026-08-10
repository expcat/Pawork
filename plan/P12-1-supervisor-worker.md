# P12-1：Supervisor / Worker

> Phase 12 · Multi-Agent · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：Phase 3（P3-1~P3-10）、Phase 7（P7-1~P7-6）、P18-2、P18-4（本任务交付了 P12 实际消费的最小 P18 契约：`agent-domain` 的 `TenantId`/`PrincipalId`/`AgentId` 与 `provider-control` CredentialLease）
> **边界与扩展**：本任务交付 **Supervisor/Worker** 编排抽象（parent/worker 角色、worker 生命周期、复用 Agent Engine 循环）。**团队协作语义**（shared task board、mailbox、presence、worker 互联、plan approval）**不在本任务范围**，由 [P17-6](P17-6-agent-teams.md) 在本任务之上叠加，复用 P12 编排、不重写 run loop。勿在本任务实现团队协作层，也勿把 supervisor/worker 完成误判为多 Agent 协作完整。

**最终目的**：实现 Parent/Worker 编排抽象，为多 Agent 协作提供基础角色模型。在核心 Coding Agent 可靠完成真实任务后才进入此阶段。

**涉及范围**：`orchestration`

## 细分步骤

1. **`AgentSupervisor` / parent/worker 抽象** —— 目的：集中拥有 spawn/assign/cancel_tree，禁止 tool 直接 `tokio::spawn` 出脱离监督的 worker。
2. **worker 生命周期管理** —— Created → Admitted → Starting → Running/Waiting → Completed；任何阶段可进入 Cancelling/Cancelled/Failed，全部事件化、可重放。目的：创建/监控/恢复/结束可解释。
3. **与 Agent Engine 复用** —— 目的：不重复实现循环。
4. **Tenant 与 Provider 资源边界** —— AgentInstance 带 tenant/session/parent identity；Worker 只经 `AcquireRequest` / lease 使用 Provider，不拿 API key。目的：隔离 Agent 与账号状态机。
5. **测试** —— 目的：基本编排可用且 crash replay 后无孤儿 worker。

## 主要产出物

- `orchestration` 的 Supervisor/Worker

## 验收标准

- [ ] parent 可创建并管理 worker，所有 worker 都有唯一 Supervisor owner
- [ ] lifecycle 可持久化/重放，崩溃恢复后无未归属 worker
- [ ] Worker 不直接获取 credential，cancel 不降低 account health

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
