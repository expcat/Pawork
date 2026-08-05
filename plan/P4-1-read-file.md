# P4-1：read_file

> Phase 4 · 核心工具与权限 · 状态：🟡未开始 · 依赖：P0-5

**最终目的**：实现只读的 read_file 工具，支持 offset/limit、行号、编码与二进制检测、图片 attachment 与路径安全，是 Agent 读取代码的基础能力。

**涉及范围**：`builtin-tools`

## 细分步骤

1. **offset/limit + 行号输出** —— 目的：可控范围读取。
2. **编码检测与二进制检测** —— 目的：正确处理文本/二进制。
3. **图片作为 attachment** —— 目的：多模态上下文。
4. **路径基于 workspace_id + relative_path** —— 目的：禁止任意绝对路径。

## 主要产出物

- read_file 工具

## 验收标准

- [ ] 行号/offset/limit 正确
- [ ] 二进制与编码检测正确
- [ ] 路径基于 workspace_id + relative_path

**相关文档**：[tools](../docs/features/tools.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)
