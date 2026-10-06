# CU-23 · Wayland 隔离后台路线可行性

> 状态：条件性待开发（后续平台）。前置：[CU-02](cu-02-target-contracts.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

确认具体 compositor / portal 与独立虚拟桌面环境后开展。

## 目标

验证 Wayland 是否存在满足无干扰目标的独立桌面截图与输入路线。

## 当前依据与读取范围

LCU 不支持原生 Wayland，Pawork 当前只提供专用 Xvnc。不能把 X11 验收推广为所有 Wayland 会话支持。

[computer-use Spec](../spec/crates/computer-use.md)、[安全规格](../spec/security.md)、[LCU 平台限制](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)。

## 实施范围

- 检索目标平台当前系统 API，验证 portal/compositor 的屏幕与输入授权、目标范围及独立虚拟桌面。
- 比较最小 Rust 路线，优先证明不碰用户主 seat / 焦点 / 剪贴板，不能自动回退 XTEST 主桌面输入。
- 交付可行性结论、必要系统条件与下一实现切片，不提前建设所有 compositor 的抽象层。

## 完成条件与验证

- 可行路线有真实环境证据，并清楚列出支持的 compositor / 版本和授权条件。
- 无安全后台路线时明确不支持并记录原因；不宣称平台能力已实现。
- 本项只闭合可行性结论，后续生产实现另按实际差距拆任务，不靠本文完成代表支持 Wayland。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

