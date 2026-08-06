# P4-8：list_directory

> Phase 4 · 核心工具与权限 · 状态：🟢已完成 · 依赖：P4-1

**最终目的**：实现 list_directory（类型/大小/mtime/symlink/分页），让 Agent 能浏览目录结构并识别 symlink。

**涉及范围**：`builtin-tools`

## 细分步骤

1. **类型/大小/mtime 输出** —— 目的：信息完整。
2. **symlink 信息** —— 目的：识别链接。
3. **分页** —— 目的：大目录可控。
4. **路径安全** —— 目的：基于 workspace 相对路径。

## 主要产出物

- list_directory 工具

## 验收标准

- [x] symlink 信息正确
- [x] 分页可用

**相关文档**：[tools](../docs/features/tools.md) · [ROADMAP](../ROADMAP.md)
