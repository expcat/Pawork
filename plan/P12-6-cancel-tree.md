# P12-6：取消树

> Phase 12 · Multi-Agent · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P3-8、P12-1、P18-4

**最终目的**：实现取消树（cancel parent → cancel all workers），保证多 Agent 取消可可靠传播。

**涉及范围**：`orchestration`

## 细分步骤

1. **取消传播树** —— 目的：parent 取消联动 workers。
2. **与 Agent 取消协作** —— 目的：复用 P3-8。
3. **无悬挂 worker** —— 目的：彻底取消。
4. **Lease 释放语义** —— 正在执行的 Provider request 取消后以 `LeaseOutcome::Cancelled` 幂等释放，不降低账号 health；目的：取消不污染账号池。
5. **测试** —— 覆盖深树、重复取消、启动中取消、Provider stream cancel 与 crash replay；目的：传播可靠。

## 主要产出物

- 取消树

## 验收标准

- [ ] 取消 parent 会取消所有 worker
- [ ] 相关 lease 被幂等释放，active count 不泄漏且 account health 不受惩罚

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
