# CU-18 · GUI 目标选择与权限交互

> 状态：待开发（P5）。前置：[CU-03](cu-03-app-window-discovery.md)、[CU-16](cu-16-target-approvals.md)、[CU-17](cu-17-gui-observation-protocol.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

让用户选择后台操作目标并管理应用/网站授权。

## 当前依据与读取范围

当前 GUI 有工具审批，但没有本机 computer-use 目标选择和应用范围权限产品面。

[Desktop Spec](../spec/desktop.md)、[desktop crate Spec](../spec/crates/desktop.md)、[GUI 设计](../gui-design.md)、[client Spec](../spec/crates/client.md)。

## 实施范围

- 通过 client/协议展示真实应用、窗口或标签目标及支持的后台能力；选择不激活目标应用。
- 明确展示当前任务授权、持久授权与撤销状态，系统权限缺失给用户可执行指引。
- UI 不直接依赖 computer-use、数据库或 Provider；Desktop 直接 pawork-* 依赖仍符合当前红线。
- 键盘、AX、错误与旧 Host 禁用状态同源，界面不暴露不必要的内部协议细节。

## 完成条件与验证

- 用户能用鼠标/键盘选对目标，授权/拒绝/撤销与 Host 的真实决定一致。
- 选择目标或展示权限不会抢目标应用前台，也不能通过模型点击形成自动批准。
- 真实窗口检查并同步 GUI 设计与 Desktop Spec，自动测试不能替代像素验收。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

