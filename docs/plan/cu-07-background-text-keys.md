# CU-07 · 后台定向文本与按键输入

> 状态：待开发（P2）。前置：[CU-01](cu-01-background-feasibility.md)、[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

补齐语义赋值不能覆盖但已证实支持后台输入的文本与按键动作。

## 当前依据与读取范围

虚拟桌面有 Unicode 文本、组合键和取消释放；本机定向输入的无干扰能力尚未证明。已弃用的 AXUIElementPostKeyboardEvent 不能作为默认现代路径。

[computer-use Spec](../spec/crates/computer-use.md)、[tools Spec](../spec/crates/tools.md)、[现行设计](../design.md#computer-use-首版)。

## 实施范围

- 只采用 CU-01 通过的公开系统定向路线，严格绑定目标应用/窗口及输入状态。
- 保留文本、键名、修饰键和取消预算；不采用系统剪贴板粘贴桥或全局事件注入。
- 拒绝不支持的控件/应用，不能临时前置目标窗口或输入后切回焦点。
- 多步按键在取消、错误和目标失效时释放本次按下状态，清楚报告可能已发送的部分。

## 完成条件与验证

- 用户同时在另一应用连续输入时，目标文本/快捷键正确，用户文本不丢失、不混入。
- 取消和超时不会遗留按下键；不自动重发非幂等输入。
- 真实目标结果与用户侧文本记录共同证明无干扰。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。
