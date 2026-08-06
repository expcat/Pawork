# P7-8：commit / branch / checkout / stash / log / show（P1）

> Phase 7 · Git、Diff 与 Worktree · 状态：🟢已完成 · 依赖：P7-2

**最终目的**：实现 P1 级 Git 操作（commit/branch/checkout/stash/log/show），含 merge-base 与 conflict 处理。MVP 可推迟。

**涉及范围**：`git-service`

## 细分步骤

1. **commit / branch / checkout / stash / log / show** —— 目的：常用 Git 操作。
2. **merge-base / conflict 处理** —— 目的：分支操作安全。
3. **错误归一** —— 目的：可处理。
4. **测试** —— 目的：操作正确。

## 主要产出物

- P1 Git 操作集

## 验收标准

- [x] 操作正确，conflict 可识别（commit/branch/stash/log/show/merge-base/unmerged，51 项真实 git 测试通过）

**相关文档**：[git-diff](../docs/features/git-diff.md) · [ROADMAP](../ROADMAP.md)
