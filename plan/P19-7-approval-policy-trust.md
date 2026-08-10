# P19-7：Approval / Policy / Workspace Trust

> Phase 19 · Desktop GUI · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P19-2、P19-3、P4-9、P4-10、P13-1、P13-5、P18-2、P18-9

**最终目的**：为文件写入、命令、网络、插件与远程操作提供 fail-closed 的审批与信任界面，让用户在多 GUI 竞争中看清 actor、能力、作用域、理由和最新 revision 后再决策。

**涉及范围**：Approval Center controller、inline approval card、Policy explanation、Workspace Trust/Tenant policy UI、revision conflict handling

## 细分步骤

1. **审批模型** —— controller 展示 tool/command、capability、workspace/tenant/account scope、requested by、风险理由与过期时间。目的：决策有上下文。
2. **安全预览** —— 文件变更显示 diff 摘要，shell 显示 argv/cwd/env redaction，网络显示目标，插件显示 capability。目的：不只给“允许/拒绝”黑盒按钮。
3. **竞争与 revision** —— approve/deny 携带 expected revision；其他客户端先处理后立即只读并显示 actor/result。目的：杜绝双批准和陈旧决策。
4. **Workspace Trust** —— 明确 global/workspace 来源、不可由 untrusted workspace 自提权；变更需二次确认。目的：守住信任边界。
5. **策略解释** —— 展示 allow/ask/deny 的 canonical reason 与生效来源，不在 GUI 重算 Policy。目的：Core 仍是唯一裁决者。
6. **紧急/批量交互** —— 批量仅允许同 scope、同 revision 的安全集合；破坏性动作不提供“一律允许”。目的：效率不扩权。
7. **a11y/security tests** —— 焦点管理与全键盘、倒计时、陈旧审批、恶意长文本、Secret redaction、多窗口 race。目的：高风险路径可复核。

## 主要产出物

- Approval Center / inline cards / Policy explanation
- Workspace Trust 与 tenant policy presentation
- 多客户端竞争、redaction、keyboard/a11y tests

## 验收标准

- [ ] 审批展示 actor/source、scope、capability、风险解释与最新 revision
- [ ] 陈旧/已处理/身份变化的审批 fail-closed，不允许 optimistic grant
- [ ] Workspace Trust 来源可见，workspace 配置不能自我提权
- [ ] shell/env/URL/tool args 的 Secret 在 GUI 状态、日志与截图 fixture 中脱敏
- [ ] CLI/GUI A/GUI B 同时处理时最终结果与 Core Event 一致

**相关文档**：[policy](../docs/features/policy.md) · [security acceptance](../docs/quality/security-acceptance.md) · [ADR-009](../docs/adr/ADR-009-default-workspace-trust.md) · [Desktop GUI](../docs/features/desktop-gui.md)
