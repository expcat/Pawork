# P4-3：edit_file

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 依赖：P4-1

**最终目的**：实现精确的 edit_file（精确替换、多段、unified patch、上下文校验、模糊匹配、冲突报告），让 Agent 能可靠地局部修改代码。

**涉及范围**：`builtin-tools`

## 细分步骤

1. **精确替换与多段编辑** —— 目的：批量局部改。
2. **unified patch 输入** —— 目的：兼容 diff 语义。
3. **上下文校验 + 模糊匹配** —— 目的：降低失败率。
4. **冲突报告（结构化 diff）** —— 目的：失败可诊断。

## 主要产出物

- edit_file 工具

## 验收标准

- [x] 精确替换与多段编辑正确
- [x] 冲突产生结构化 diff 报告

**相关文档**：[tools](../docs/features/tools.md) · [git-diff](../docs/features/git-diff.md) · [ROADMAP](../ROADMAP.md)
