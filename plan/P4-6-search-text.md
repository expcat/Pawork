# P4-6：search_text

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 依赖：P4-1

**最终目的**：实现 search_text（固定串/正则、文件过滤、ignore、结果限制、上下文行、Unicode），让 Agent 能在代码库中精确检索。

**涉及范围**：`builtin-tools`

## 细分步骤

1. **固定串 / 正则匹配** —— 目的：灵活检索。
2. **文件过滤 + ignore 规则** —— 目的：聚焦相关文件。
3. **结果限制 + 上下文行** —— 目的：可控输出量。
4. **Unicode 与安全** —— 目的：多语言、无 ReDoS。

## 主要产出物

- search_text 工具

## 验收标准

- [x] 正则/固定串正确
- [x] ignore 生效、结果受限

**相关文档**：[tools](../docs/features/tools.md) · [ROADMAP](../ROADMAP.md)
