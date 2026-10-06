# CU-21 · 隔离 Linux X11 / AT-SPI 应用控制

> 状态：条件性待开发（后续环境）。前置：[CU-02](cu-02-target-contracts.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

作为明确选择的隔离桌面扩展；不作为本机后台能力的替代验收。

## 目标

为现有专用 Linux 桌面增加 Rust 应用/窗口和元素控制通道。

## 当前依据与读取范围

现有 Alpine / Fluxbox / Xvnc 有 RFB 图像与输入，不具备 Rust guest AX 服务。RFB 协议不能直接返回 AT-SPI 树；LCU 的 glibc 原 runtime 要求不等于独立 Rust 实现的平台限制。

[computer-use Spec](../spec/crates/computer-use.md)、[隔离桌面部署](../../crates/computer-use/README.md)、[安全规格](../spec/security.md)。

## 实施范围

- 在现有包边界优先实现最小 Rust guest 入口，通过 X11 / D-Bus / AT-SPI 提供应用、窗口和元素；RFB 保留图像/输入职责。
- guest 状态通道受控且有身份校验，模型不能指定任意端口/路径；不挂宿主目录、设备、屏幕或剪贴板。
- 沿用观测、目标归属、审批和取消契约，不同时新建另一套 Agent 引擎或容器池。
- 容器仍显式启动；GUI/模型明确显示目标属于隔离环境，不静默把本机请求转到容器。

## 完成条件与验证

- 容器内真实应用能被定位、读树和操作，状态与图像一致。
- 用户本机键鼠、焦点和剪贴板不受影响，guest 断线和租约拒绝符合共享契约。
- 只声明此隔离环境的能力，不能记为本机既有应用后台验收通过。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

