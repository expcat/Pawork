# P4-7：find_files

> Phase 4 · 核心工具与权限 · 状态：🟡未开始 · 依赖：P4-1

**最终目的**：实现 find_files（glob、类型过滤、ignore、最大深度/结果、排序），让 Agent 能按模式定位文件。

**涉及范围**：`builtin-tools`

## 细分步骤

1. **glob 匹配 + 类型过滤** —— 目的：按模式/类型筛选。
2. **ignore + 最大深度/结果** —— 目的：性能与噪声控制。
3. **稳定排序** —— 目的：结果可复现。
4. **路径安全** —— 目的：基于 workspace 相对路径。

## 主要产出物

- find_files 工具

## 验收标准

- [ ] glob 与 ignore 正确
- [ ] 结果受限且稳定排序

**相关文档**：[tools](../docs/features/tools.md) · [ROADMAP](../ROADMAP.md)
