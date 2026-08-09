# P8-2：AGENTS.md（根 + 路径层级）

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟢已完成 · 依赖：P8-1

**最终目的**：实现 `AGENTS.md` 的根 + 路径层级聚合，按当前文件路径层级确定生效指令，保证确定性优先级。

**涉及范围**：`resource-loader`、`context-engine`

## 细分步骤

1. **根与子目录 AGENTS.md 发现** —— 目的：层级覆盖。
2. **按路径层级聚合** —— 目的：靠近当前文件的优先。
3. **确定性优先级** —— 目的：不依赖扫描顺序。
4. **测试** —— 目的：层级正确。

## 主要产出物

- AGENTS.md 层级聚合

## 验收标准

- [x] 按路径层级确定性聚合

**实现**：`resource-loader::AgentsHierarchy` 从 Workspace Root 到当前路径逐层发现并稳定排序 `AGENTS.md`，越界 symlink、非 UTF-8 与超限文件均按单文件隔离。

**相关文档**：[skills](../docs/features/skills.md) · [context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)
