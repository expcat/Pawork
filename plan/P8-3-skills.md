# P8-3：Skills

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟡未开始 · 依赖：P8-1

**最终目的**：实现 Skills（manifest、激活、参数、脚本、权限、版本、依赖、冲突检测、热重载），作为声明式扩展的主入口。

**涉及范围**：`resource-loader`

## 细分步骤

1. **manifest 解析与激活** —— 目的：技能可被加载激活。
2. **参数 / 脚本 / 权限** —— 目的：可控执行。
3. **版本 / 依赖 / 冲突检测** —— 目的：多技能共存。
4. **热重载** —— 目的：开发期可迭代。

## 主要产出物

- Skills 加载与管理

## 验收标准

- [ ] manifest 可激活、冲突可检测、支持热重载

**相关文档**：[skills](../docs/features/skills.md) · [ROADMAP](../ROADMAP.md)
