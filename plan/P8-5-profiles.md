# P8-5：Profiles / Agent Profile

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟡未开始 · 依赖：P8-1

**最终目的**：实现 Agent Profile（agent profile、单次运行 instructions），让单次运行可注入专属指令与配置。

**涉及范围**：`resource-loader`

## 细分步骤

1. **agent profile 定义** —— 目的：可命名配置集。
2. **单次运行 instructions** —— 目的：运行期注入。
3. **与配置优先级协作** —— 目的：确定性。
4. **测试** —— 目的：生效正确。

## 主要产出物

- Profiles / Agent Profile

## 验收标准

- [ ] 单次运行可注入 instructions

**相关文档**：[skills](../docs/features/skills.md) · [context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)
