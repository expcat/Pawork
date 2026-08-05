# P7-3：结构化 Diff

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 依赖：P7-2

**最终目的**：实现结构化 Diff（`DiffFile`/`DiffHunk`/`DiffLine`、分页、rename/binary/untracked/submodule、CRLF、Unicode 文件名），支撑大规模 diff 的可分页浏览。

**涉及范围**：`diff-service`

## 细分步骤

1. **结构化模型** —— `DiffFile`/`DiffHunk`/`DiffLine`。目的：可渲染可分析。
2. **rename/binary/untracked/submodule 处理** —— 目的：覆盖特殊情形。
3. **分页 + CRLF + Unicode 文件名** —— 目的：大 diff 与跨平台可用。
4. **100k 行基准** —— 目的：性能达标。

## 主要产出物

- `diff-service` crate

## 验收标准

- [ ] rename/binary/untracked/submodule 测试通过
- [ ] Diff 可分页
- [ ] 100,000 行 Diff < 500ms

**相关文档**：[git-diff](../docs/features/git-diff.md) · [ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md) · [ROADMAP](../ROADMAP.md)
