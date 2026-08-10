# P12-2：任务分解 / 任务图

> Phase 12 · Multi-Agent · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P12-1、P18-2、P18-9（消费 `tenant-service` TenantPolicy 做 tenant 隔离）

**最终目的**：实现任务分解与依赖图，让 Parent 能把复杂任务拆分给多个 Worker 并按依赖调度。

**涉及范围**：`orchestration`

## 细分步骤

1. **任务分解抽象** —— AgentTask 携带 tenant/owner/budget/workspace/version；目的：可拆分且不可跨 tenant 漏权。
2. **依赖图与调度** —— Created/Ready/Assigned/Running/Blocked/Completed/Failed/Cancelled 事件化；目的：按依赖执行并可重放。
3. **失败与重试策略** —— 区分 Agent/task failure 与 Provider/account failure；目的：账号 failover 不误改 TaskGraph。
4. **一致性测试** —— DAG、循环拒绝、重复事件幂等、cancel/retry/replay；目的：依赖正确。

## 主要产出物

- 任务图与调度

## 验收标准

- [ ] 任务图按依赖正确调度
- [ ] TaskGraph 事件可重放且跨 tenant edge 被拒绝
- [ ] Provider/account failover 不直接改变 task ownership 或生命周期

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [ROADMAP](../ROADMAP.md)
