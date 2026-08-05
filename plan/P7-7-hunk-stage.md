# P7-7：Hunk / Line stage（P1）

> Phase 7 · Git、Diff 与 Worktree · 状态：⚪P1（可推迟）· 依赖：P7-3

**最终目的**：实现按 hunk / 按行暂存，让用户能精细控制提交内容。标记为 P1，可在 MVP 后交付。

**涉及范围**：`git-service`、`diff-service`

## 细分步骤

1. **按 hunk 暂存** —— 目的：块级暂存。
2. **按行暂存** —— 目的：行级暂存。
3. **与结构化 Diff 协作** —— 目的：基于 DiffLine。
4. **测试** —— 目的：暂存结果正确。

## 主要产出物

- Hunk / Line stage

## 验收标准

- [ ] 按块/按行暂存结果正确

**相关文档**：[git-diff](../docs/features/git-diff.md) · [ROADMAP](../ROADMAP.md)
