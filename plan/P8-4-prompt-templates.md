# P8-4：Prompt Templates

> Phase 8 · Skills、Prompts 与 Instructions · 状态：🟢已完成 · 依赖：P8-1

**最终目的**：实现 Prompt Templates（markdown、参数、文件引用、默认 model/thinking/tools/budget、工作区覆盖），让常用提示可复用。

**涉及范围**：`resource-loader`

## 细分步骤

1. **markdown 模板 + 参数** —— 目的：可参数化复用。
2. **文件引用** —— 目的：引用资源文件。
3. **默认 model/thinking/tools/budget** —— 目的：模板自带配置。
4. **工作区覆盖** —— 目的：可定制。

## 主要产出物

- Prompt Templates

## 验收标准

- [x] 模板可参数化、可带默认配置

**实现**：Prompt Template 使用 TOML `+++` frontmatter + Markdown body，支持参数/default、workspace-relative 文件引用、model/thinking/tools/budget 默认值与 Workspace 覆盖。

**相关文档**：[skills](../docs/features/skills.md) · [context](../docs/features/context.md) · [ROADMAP](../ROADMAP.md)
