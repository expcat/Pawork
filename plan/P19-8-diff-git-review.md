# P19-8：Diff / Git / Checkpoint / Review

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、P4-11、P7-1～P7-9、P13-8、P16-8

**最终目的**：提供面向大规模变更的可审阅界面，把 Diff、Git 暂存、Checkpoint/Rollback 与 Review Finding 串成一个受 Policy/revision 约束的闭环。

**涉及范围**：Changes tree、GPUI 虚拟化 diff viewer、Git action bar、Checkpoint/Rollback UI、Review inspector

## 细分步骤

1. **Changes tree** —— staged/unstaged/untracked、rename/binary/submodule、filter/search 与统计。目的：先建立变更全貌。
2. **Diff viewer** —— GPUI 虚拟列表分页渲染 file/hunk/line，side-by-side/unified、no-newline、CRLF、Unicode 与 large/binary fallback。目的：100k 行仍可交互。
3. **Git actions** —— stage/unstage/hunk/line/discard/commit 等操作经 AppCommand，展示 actor/revision/conflict。目的：不由 GUI 执行 Git。
4. **Checkpoint/Rollback** —— 写前 checkpoint、变更预览、冲突/部分失败与恢复路径。目的：破坏性动作可撤销且 fail-closed。
5. **Review Finding** —— 行锚点、stale/re-anchor、severity、suggested patch、resolve/reopen。目的：连接 P16-8 审查语义。
6. **并发与刷新** —— Git index/file change 导致 revision 变化时保留阅读位置，禁用陈旧动作并刷新。目的：避免误操作旧 diff。
7. **性能/visual/a11y tests** —— 100k lines、长路径、主题、键盘逐 hunk、AccessKit 读屏摘要。目的：高密度界面可用。

## 主要产出物

- Changes tree 与 GPUI 虚拟化 Diff viewer
- Git/Checkpoint/Review command flows
- 大 diff、冲突、revision、visual/a11y/performance fixtures

## 验收标准

- [ ] 100,000 行 Diff 首屏与渲染内存预算达到性能目标，不一次复制全部文本
- [ ] stage/unstage/discard/rollback/commit 都经 Core/Policy，陈旧 revision 被拒后刷新
- [ ] binary/rename/submodule/no-newline/CRLF/Unicode 有明确安全显示
- [ ] Review anchor 漂移显示 stale，不静默指向错误行
- [ ] 破坏性动作有最新预览、确认与可恢复结果
- [ ] **（P16-10 延期接线）Review UI + 真实 Forge host**：finding / line anchor / stale re-anchor / severity / suggested patch 经 `core-api` 查询并在 UI 呈现；Review 富字段（evidence/assignee/patch/fingerprint）进 canonical event 可重放（修复 P16-8 事件外内存补写、fingerprint=None 致 stale）；Forge 副作用移到 host 显式命令/connector 并持久化远端 comment ID（修复 Generic Forge 假副作用、本地合成 ID 冒充 `published`）；SuggestedPatch 在 checkpoint/policy 真接入前仅显示为 dry-run。见 [p16-review §1/§3.5/§4.3](../docs/review/p16-review.md) 与 [plan/README Phase 16 登记](README.md)。

**相关文档**：[git-diff](../docs/features/git-diff.md) · [checkpoint](../docs/features/checkpoint.md) · [Desktop GUI](../docs/features/desktop-gui.md) · [性能目标](../docs/quality/performance-targets.md)

**依赖建议（2026-08）**：Diff/Changes 长列表使用 GPUI 虚拟列表原语或自实现；行文本渲染自实现；不引入浏览器端 Git 或完整代码编辑器。
