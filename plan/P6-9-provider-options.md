# P6-9：Provider-specific options

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P6-1、P6-2、P6-3

**最终目的**：实现 provider 特有选项透传与原始 metadata 保存，让用户可访问 provider 专属能力，同时核心 Agent 不含特例（ADR-002）。

> 范围澄清（2026-08 收敛）：`provider_options` 是「Provider 特有、canonical 未建模的剩余参数」的逃逸通道，**不再是 reasoning effort 的归属**。reasoning effort / 强度已提升为 canonical 一等字段（`ReasoningEffort`），经 [P15-7](P15-7-reasoning-state.md) / [P15-8](P15-8-capability-discovery.md) 协商翻译；[P17-5](P17-5-agent-profile-v2.md) 的 `AgentProfile.effort` 走 canonical 路径，不再经 `provider_options`。本任务只承接真正 Provider-specific 且无 canonical 表达的杂项配置，并保持 raw metadata 审计。

**涉及范围**：`provider-api`、`provider-*`

## 细分步骤

1. **透传选项通道** —— 目的：provider 专属参数可达。
2. **原始 metadata 保存** —— 目的：不丢信息、可审计。
3. **核心不按 provider 名分支** —— 目的：守住解耦红线。
4. **回归测试** —— 目的：核心无 provider 特例。

## 主要产出物

- 透传选项 + raw metadata

## 验收标准

- [ ] agent core 无 provider 特例判断

**相关文档**：[providers](../docs/features/providers.md) · [P15-7 Reasoning State](P15-7-reasoning-state.md) · [P15-8 Capability Discovery](P15-8-capability-discovery.md) · [P17-5 Agent Profile v2](P17-5-agent-profile-v2.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ROADMAP](../ROADMAP.md)
