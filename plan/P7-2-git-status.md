# P7-2：status / changed files

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 依赖：P7-1

**最终目的**：解析 status（staged/unstaged/untracked），为 Diff 与暂存提供稳定的变更文件列表。

**涉及范围**：`git-service`

## 细分步骤

1. **status 解析** —— staged/unstaged/untracked。目的：稳定结构化输出。
2. **changed files 列表** —— 目的：供 Diff 与 UI。
3. **解析稳定性测试** —— 目的：跨版本稳定。
4. **性能** —— 目的：大仓库可用。

## 主要产出物

- status / changed files

## 验收标准

- [ ] 解析稳定（含多版本 Git）

**相关文档**：[git-diff](../docs/features/git-diff.md) · [ROADMAP](../ROADMAP.md)
