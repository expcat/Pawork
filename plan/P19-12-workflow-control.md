# P19-12：Plan / Goal / Background / Automation

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、P16-1～P16-8、P17-1、P18-13

**最终目的**：把长期运行的 Plan、Goal、Background Task、Automation、Monitor 与 Memory 状态呈现为可审阅、可恢复、可追责的控制面，使 GUI 断线、重启或多客户端操作不改变任务生命周期。

**涉及范围**：Plan editor/review、Goal/Task center、Automation/Monitor pages、Inbox、Memory inspector、activity/audit timeline

## 细分步骤

1. **Plan** —— versioned artifact、step/status、comment anchor、diff/revision、approve/reject/revise。目的：计划先审后执行。
2. **Goal** —— objective/success criteria/progress/evidence/complete/blocked，显示 source/owner。目的：持久目标可核验。
3. **Background Task** —— process/agent/monitor/automation 的统一 list/detail/cancel/resume/restart/log/Artifact。目的：后台生命周期集中。
4. **Automation** —— cron/interval/once/event trigger、timezone、next run、inbox/result、pause/trigger。目的：调度语义明确。
5. **Monitor** —— health、heartbeat、attach/detach/restart、notification policy。目的：常驻任务可观察。
6. **Memory** —— source/citation/retention/forget 与注入解释；P2 feature 可按 capability 隐藏。目的：长期记忆可控。
7. **断线/并发** —— GUI 关闭不取消，reconnect 从 Snapshot 恢复；revision conflict 显示 actor。目的：多客户端一致。
8. **时间/恢复测试** —— fake clock、DST、missed trigger、crash/restart、duplicate event、stale approval。目的：长期状态可复核。

## 主要产出物

- Plan/Goal/Task/Automation/Monitor/Memory pages
- version/revision/audit/notification interactions
- fake-clock、recovery、multi-client contract tests

## 验收标准

- [ ] Plan 未批准前执行 gate 不可由 UI 绕过；评论/修订锚点版本明确
- [ ] Background/Automation/Monitor 在 GUI 断线/退出后继续，重连恢复同一状态
- [ ] 时区/DST/错过触发/重复事件有明确展示与测试
- [ ] cancel/resume/restart/forget 等动作带 revision/Policy 并由 Event 确认
- [ ] capability 未交付时页面明确 unavailable，不用本地 mock 冒充

**相关文档**：[agent-engine](../docs/features/agent-engine.md) · [Desktop GUI](../docs/features/desktop-gui.md) · [P16-1](P16-1-plan-mode.md) · [P16-4](P16-4-background-task-manager.md) · [P16-5](P16-5-scheduled-automation.md)
