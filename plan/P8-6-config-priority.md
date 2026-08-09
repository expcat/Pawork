# P8-6：配置优先级（确定性）

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟢已完成 · 依赖：P8-2、P8-3、P8-4、P8-5

**最终目的**：固化配置优先级（内置 < 用户全局 < profile < 工作区 < session < run）为确定性规则，保证相同配置始终产生相同上下文。

**涉及范围**：`context-engine`

## 细分步骤

1. **优先级规则实现** —— 目的：确定性合并。
2. **跨来源冲突解决** —— 目的：高优先级覆盖。
3. **不依赖扫描顺序** —— 目的：可复现。
4. **确定性测试** —— 目的：同输入同输出。

## 主要产出物

- 配置优先级合并

## 验收标准

- [x] 相同配置产生确定性上下文

**实现**：复用 `config-service::ConfigTier` 六级优先级；Resource 候选与 Context 贡献均使用 tier + 稳定 source key 排序，并以反向输入回归测试守护确定性。

**相关文档**：[context](../docs/features/context.md) · [skills](../docs/features/skills.md) · [ROADMAP](../ROADMAP.md)
