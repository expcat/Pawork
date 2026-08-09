# P19-13：Multi-Agent / Teams / Task Graph

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、Phase 12、P17-5、P17-6、P18-4、P18-8、P18-9

**最终目的**：为 Supervisor/Worker、TaskGraph、独立 Worktree、预算与 Agent Teams 提供可理解的编排视图，使用户能看到依赖、所有权、资源与合并冲突，而不在 GUI 复制 scheduler。

**涉及范围**：TaskGraph view、Worker inspector、budget/concurrency controls、worktree/patch merge UI、team board/mailbox/presence

## 细分步骤

1. **TaskGraph** —— DAG/列表双视图，状态、dependency/blocker、critical path 与 event history。目的：复杂编排可理解且可访问。
2. **Worker** —— profile/model/effort/tenant/account lease status、session/worktree owner、budget/usage/cancel tree。目的：资源与归属透明。
3. **控制动作** —— spawn/pause/resume/cancel/retry/reassign 发送 canonical command；GUI 不做调度选择。目的：单一 scheduler。
4. **结果与合并** —— patch/artifact/evidence、conflict、review、approve merge/reject；变更继续走 Checkpoint/Policy。目的：并行写入可审查。
5. **Teams** —— shared board、mailbox、presence、mention、unread 与 ownership；消息版本化可回放。目的：协作状态可恢复。
6. **大图降级** —— 大 DAG 聚合/分页/列表 fallback、键盘导航与文字摘要。目的：性能和 accessibility 不依赖 Canvas 视觉。
7. **并发测试** —— worker crash/cancel tree、budget exhausted、lease reclaim、worktree conflict、stale merge、team disconnect。目的：高风险编排验证。

## 主要产出物

- TaskGraph/Worker/Team UI 与可访问 fallback
- Budget/ownership/worktree/result merge inspectors
- concurrency/recovery/conflict/performance fixtures

## 验收标准

- [ ] UI 展示 Core scheduler 结果，不自行选择 worker/account/model
- [ ] Worker session/worktree/lease/budget/tenant ownership 可追溯且脱敏
- [ ] parent cancel、worker crash、budget/lease 回收状态最终与 Event 一致
- [ ] merge/reject 基于最新 revision，冲突不能 optimistic 覆盖
- [ ] 大 DAG 有虚拟化列表和读屏摘要，不要求视觉图才能操作

**相关文档**：[multi-agent](../docs/features/multi-agent.md) · [provider-control-plane](../docs/features/provider-control-plane.md) · [Desktop GUI](../docs/features/desktop-gui.md) · [Phase 12](../ROADMAP.md)
