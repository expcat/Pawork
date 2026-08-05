# P1-8：文件索引

> Phase 1 · 基础设施 · 状态：🟢已完成 · 依赖：P1-7

**最终目的**：实现异步文件索引（增量更新 + ignore 规则），为 `@file` 搜索与上下文构建提供快速文件定位。

**涉及范围**：`file-index`

## 细分步骤

1. **异步扫描与增量更新** —— 目的：大目录可用。
2. **ignore 规则** —— `.gitignore` / 全局 / 工作区 ignore。目的：排除噪声。
3. **去抖** —— 目的：避免频繁重建。
4. **大目录排除规则** —— 目的：性能可控。

## 主要产出物

- `file-index` crate

## 验收标准

- [x] 大目录排除规则生效
- [x] 增量更新正确

**相关文档**：[workspace-index](../docs/features/workspace-index.md) · [ROADMAP](../ROADMAP.md)
