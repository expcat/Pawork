# CU-22 · Windows 独立会话 / VM 后台能力

> 状态：条件性待开发（后续平台）。前置：[CU-02](cu-02-target-contracts.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

准备独立 Windows 会话或 VM，并以不干扰用户主桌面为前提。

## 目标

通过 Rust Windows 系统 API 实现独立环境中的应用、窗口、UIA、截图和输入。

## 当前依据与读取范围

官方 Windows Computer Use 工作在前台活跃桌面，LCU Windows 在对比版本仍为候选。这不能满足用户要求的同会话无干扰目标。

[computer-use Spec](../spec/crates/computer-use.md)、[架构](../architecture.md)、[官方 Computer Use](https://learn.chatgpt.com/docs/computer-use)。

## 实施范围

- 先验证独立登录会话或专用 VM 的捕获/UI Automation/输入和宿主隔离，不把 SendInput 注入用户主桌面。
- 在已有 computer-use 契约下增加必要 Windows 实现，Core 正式宿主仍是 pawork；guest 只承载受控系统能力。
- 首个切片完成真实窗口发现、UIA 和截图，再按共享任务补输入与生命周期；复杂 VM 管理不在首个切片。
- 拒绝权限/UIPI 或目标会话不一致等真实条件，不以最小化窗口宣称后台隔离。

## 完成条件与验证

- 用户主桌面持续操作时，独立环境动作正确且不影响用户输入、焦点或剪贴板。
- 窗口、句柄、审批、停止与断线有真实 Windows 证据。
- 主桌面前台控制仅能登记为限制，不能作为本任务完成方式。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

