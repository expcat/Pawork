# CU-20 · macOS 后台能力端到端验收与文档收口

> 状态：待开发（P6）。前置：[CU-03](cu-03-app-window-discovery.md)、[CU-04](cu-04-window-observation.md)、[CU-05](cu-05-accessibility-tree.md)、[CU-06](cu-06-semantic-actions.md)、[CU-07](cu-07-background-text-keys.md)、[CU-08](cu-08-background-pointer.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-10](cu-10-action-observe.md)、[CU-11](cu-11-model-vision-gate.md)、[CU-12](cu-12-screenshot-context.md)、[CU-13](cu-13-browser-observation.md)、[CU-14](cu-14-browser-tabs.md)、[CU-15](cu-15-external-browser.md)、[CU-16](cu-16-target-approvals.md)、[CU-17](cu-17-gui-observation-protocol.md)、[CU-18](cu-18-gui-target-approvals.md)、[CU-19](cu-19-gui-evidence-stop.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

闭合首阶段本机后台核心能力，证明用户可持续工作且操作证据完整。

## 当前依据与读取范围

本轮只有静态研究和规划，没有新引擎实现、真实窗口或 Provider 验收。旧隔离桌面和历史 macOS 原型结论不能替代本轮本机后台证据。

[验证规格](../spec/verification.md)、[computer-use Spec](../spec/crates/computer-use.md)、[browser Spec](../spec/crates/browser.md)、[架构](../architecture.md)、[GUI 设计](../gui-design.md)。

## 实施范围

- 先跑实际写入集的既有定向测试，再在真实 AppKit / Electron / 浏览器完成应用定位、截图、元素/键鼠和多应用任务。
- 用户同时在其它应用连续输入，核对目标 UI/文件/DOM 与用户侧文本及焦点/指针；纳入遮挡、接管、取消、过期、拒绝和恢复。
- 真实模型固定临时参数 `--provider opencode-go --model glm-5.3-flash`，不写默认；连接或图像能力失败如实记录，不换模型凑通过。
- 回放和 GUI 同时核对原始证据，不重执行动作。只有出现新变化或未解决问题才扩大测试，不跑无关全 workspace。
- 按实际改动同步包级 Spec、architecture/design/gui-design 与本任务的能力矩阵，分别写实现、自动验证、真实/人工验收；发布仍非本任务。

## 完成条件与验证

- 核心场景满足不抢键鼠/焦点/剪贴板、持续后台执行和用户接管让出，所有通过结论有真实证据。
- 未支持动作列为缺口；若核心目标未达到，不得仅凭子任务或隔离环境通过把本项标 ✅。
- GUI 证据、Host 状态、持久化/恢复与实际目标结果一致，用户验收结果明确。
- 后续平台/锁屏/音频任务仍独立跟踪，本项完成不表示全平台或发布完成。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

