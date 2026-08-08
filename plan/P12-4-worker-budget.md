# P12-4：Worker 预算 / 模型 / 并发上限

> Phase 12 · Multi-Agent · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P3-6、P12-1、P18-4、P18-6、P18-8、P18-9

**最终目的**：为 Worker 设定 token/模型/并发预算，让多 Agent 并发开销可控。

**涉及范围**：`orchestration`

## 细分步骤

1. **worker token/模型/费用预算** —— 消费 Usage/Cost Ledger 与 TenantPolicy；目的：成本归属可控。
2. **双层并发上限** —— Agent 并发由 Supervisor/TenantPolicy 控制，请求并发由 CredentialLease 控制；目的：不混用两类 scheduler。
3. **达预算行为** —— pause/cancel/reassign/fallback 均产生显式事件；目的：优雅降级且可审计。
4. **测试** —— 覆盖 parent→child 预算传播、tenant 总额、lease concurrency 与 cancel；目的：预算生效。

## 主要产出物

- worker 预算控制

## 验收标准

- [ ] 并发与预算可控
- [ ] Agent 并发与 account request concurrency 使用独立计数器/状态机
- [ ] usage 可归属 tenant/session/agent/account，达预算动作有 audit event

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [agent-engine](../docs/features/agent-engine.md) · [ROADMAP](../ROADMAP.md)
