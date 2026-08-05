# P7-4：stage / unstage / discard

> Phase 7 · Git、Diff 与 Worktree · 状态：🟡未开始 · 依赖：P7-2

**最终目的**：实现 stage / unstage / discard 操作，让用户与 Agent 可管理工作区与索引区变更。

**涉及范围**：`git-service`

## 细分步骤

1. **stage / unstage / discard** —— 目的：基本暂存操作。
2. **与 Policy 协作（写操作审批）** —— 目的：安全。
3. **错误归一** —— 目的：可处理。
4. **测试** —— 目的：状态正确。

## 主要产出物

- stage / unstage / discard

## 验收标准

- [ ] 操作后 index 状态正确

**相关文档**：[git-diff](../docs/features/git-diff.md) · [policy](../docs/features/policy.md) · [ROADMAP](../ROADMAP.md)
