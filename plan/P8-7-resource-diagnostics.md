# P8-7：Resource Diagnostics

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟡未开始 · 依赖：P8-6

**最终目的**：实现 Resource Diagnostics，显示所有生效来源，让用户与 Agent 可诊断「为什么这条指令生效」。

**涉及范围**：`resource-loader`、`diagnostics`

## 细分步骤

1. **来源追踪记录** —— 目的：每条指令可溯源。
2. **诊断视图** —— 目的：展示生效来源。
3. **与脱敏日志协作** —— 目的：安全。
4. **测试** —— 目的：来源正确。

## 主要产出物

- Resource Diagnostics

## 验收标准

- [ ] 可显示所有生效来源

**相关文档**：[skills](../docs/features/skills.md) · [observability](../docs/features/observability.md) · [ROADMAP](../ROADMAP.md)
