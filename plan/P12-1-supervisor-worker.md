# P12-1：Supervisor / Worker

> Phase 12 · Multi-Agent · 状态：🟡未开始 · 依赖：Phase 3（P3-1~P3-10）、Phase 7（P7-1~P7-6）
> **边界与扩展**：本任务交付 **Supervisor/Worker** 编排抽象（parent/worker 角色、worker 生命周期、复用 Agent Engine 循环）。**团队协作语义**（shared task board、mailbox、presence、worker 互联、plan approval）**不在本任务范围**，由 [P17-6](P17-6-agent-teams.md) 在本任务之上叠加，复用 P12 编排、不重写 run loop。勿在本任务实现团队协作层，也勿把 supervisor/worker 完成误判为多 Agent 协作完整。

**最终目的**：实现 Parent/Worker 编排抽象，为多 Agent 协作提供基础角色模型。在核心 Coding Agent 可靠完成真实任务后才进入此阶段。

**涉及范围**：`orchestration`

## 细分步骤

1. **parent/worker 抽象** —— 目的：角色与职责定义。
2. **worker 生命周期管理** —— 目的：创建/监控/结束。
3. **与 Agent Engine 复用** —— 目的：不重复实现循环。
4. **测试** —— 目的：基本编排可用。

## 主要产出物

- `orchestration` 的 Supervisor/Worker

## 验收标准

- [ ] parent 可创建并管理 worker

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
