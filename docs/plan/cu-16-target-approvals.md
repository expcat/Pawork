# CU-16 · 应用与网站范围的 Host 审批

> 状态：待开发（P1）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在现有 Policy 和显式审批上补目标范围，生产原生操作在本任务落地后才接入。

## 当前依据与读取范围

当前 computer 工具是 requires_approval 的 ExternalPlugin，拒绝 untrusted / ReadOnly，Approve for run 已接线；尚无本机应用或网站授权范围。

[policy Spec](../spec/crates/policy.md)、[tools Spec](../spec/crates/tools.md)、[app Spec](../spec/crates/app.md)、[安全规格](../spec/security.md)、[Settings Spec](../spec/settings.md)。

## 实施范围

- 将应用/网站身份与 workspace/run 审批绑定，捕获、读树、启动和输入均在实际后端访问前裁决。
- 区分 OS Screen Recording / Accessibility / Automation 权限与 Pawork 目标授权；不能代用户批准系统权限。
- 复用现有一次/本 Run 审批与拒绝语义，自动 resolver 不能冒充显式用户批准；持久允许仅按已裁决设置口径实现，并可撤销。
- 授权变化与 CU-09 占用、观测和取消联动，跨目标和失效授权不能继续派发。
- 防止 Agent 通过自身审批界面或系统权限界面构成自我批准；必要安全/持久契约变化先裁决并做 golden。

## 完成条件与验证

- 拒绝、未授权、ReadOnly 和不可信 workspace 在连接、捕获或输入前零后端访问。
- 授权只覆盖用户选定应用/网站，撤销影响后续动作且已执行历史诚实保留。
- 对应 Policy / 显式审批 / Secret 定向回归通过，不另造审批框架。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

