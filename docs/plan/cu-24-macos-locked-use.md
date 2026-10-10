# CU-24 · macOS 锁屏使用独立设计与验证

> 状态：设计与契约拒绝模拟完成；独立 review / 复审 PASS；特权组件未实施，系统安装、系统授权与真实锁屏验收待环境。前置：[CU-20](cu-20-native-acceptance.md)。更新：2026-10-10。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

用户明确启用锁屏功能后，单独裁决特权组件与系统授权方案。

## 目标

研究并验证锁屏下的受限后台操作，不把普通 AX 后台操作等同锁屏支持。

## 当前依据与读取范围

官方锁屏使用需要单独启用的 macOS authorization plug-in，授权窗口与可信活动 turn 绑定；当前 Pawork 无此实现，LCU 包装器不提供独立实现。

[架构](../architecture.md)、[安全规格](../spec/security.md)、[operations Spec](../spec/operations.md)、[官方锁屏使用](https://learn.chatgpt.com/docs/computer-use#locked-use)。

## 实施范围

- 先独立设计显式开启、系统授权、短期 turn 身份、安装/卸载与失败处理；不复制官方私有组件，也不作为普通辅助进程顺带安装。
- 验证只有有效任务可使用授权窗口，过期/取消/停用后立即失效；不成为通用远程解锁路径。
- 用户本机输入或接管时停止自动操作并恢复锁定保护，不让 Agent 与用户竞争。
- 先交付设计和可行性证据；特权、安全或冻结契约变化按仓库规则完成具体裁决后再实施。

## 完成条件与验证

- 设计的系统授权和输入保护有真实 macOS 验证，不能仅凭后台截图通过。
- 启停、任务取消、授权过期和用户输入均正确终止相应权限。
- 记录设计/实现/真实验收各自状态；未实施不标锁屏能力已完成，安装发布另行处理。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

### 2026-10-10 设计与模拟边界

按用户限定，本轮只推进设计与可模拟拒绝边界：不启用锁屏功能、不安装 Apple authorization plug-in、不申请或修改系统授权，不把普通 AX 后台能力写成锁屏支持。设计证据与原始命令在 `/tmp/pawork-cu24-locked-design/evidence/`（design.md、CodeGraph/rg 记录、测试日志、final.diff 与 SHA256SUMS）。

设计结论：锁屏使用必须由独立 Apple authorization plug-in 显式开启，不并入普通辅助进程、不随普通安装顺带安装；安装失败回退未启用，卸载以系统侧 mechanism 移除为准。OS 层 Screen Recording、Accessibility 与 plug-in 安装全部由用户系统手势完成；Pawork 层复用 CU-16 授权台账与门序。短期解锁尝试由宿主侧当前活跃可信 turn 签发与校验，模型不能签发；plug-in 参与前核对尝试仍对应当前活跃 turn/scope、尚未消费且 run 未终态，窗口仅覆盖这一次尝试，成功、失败、本地重锁或终态即作废，一次尝试最多一次解锁，普通 Once / ForRun 批准不能当作解锁许可。本地键鼠输入立即重锁并暂停自动解锁；plug-in 状态、安装校验或授权不可判定时 fail-closed 拒绝。停用请求先撤销未消费尝试、停止自动解锁并恢复锁定；mechanism 移除失败保持拒绝态并如实报告残留，不恢复已启用或自动解锁。审计复用现有日志通道，不新增第二台账或日志体系。

实现与验证：生产代码无锁屏路径。`Environment` 契约仅允许 `isolated_desktop` 与 `native_background`，新增 `locked_use_environment_is_rejected_by_the_contract` 钉住两值序列化，并证明完整 `TargetObservation` JSON 在 `environment: "locked_use"` 时反序列化失败。`cargo test -p pawork-computer-use --offline --lib` 100 passed / 0 failed、exit 0；该测试是契约模拟，不是 macOS 授权或真实锁屏验收。

待真实环境：真机锁定 → 临时解锁 → 本地输入重锁 → 暂停自动解锁序列；plug-in 安装、卸载与失败回退实测（含系统侧 mechanism 移除验证）；Screen Recording / Accessibility 系统授权的真实授予与撤销行为。上述未验项未完成前本任务不标 ✅。

独立 review（2026-10-10，Sol max，thread `01a12534-7903-7ab1-8abe-80b8e328256c`）：R1 初审 turn `01a12534-7d50-7a70-b61c-c87a2dadc768` 报两个 P2——短期解锁许可签发 / 到期 / 单次消费未闭合、卸载失败后的停用状态未定义。Grok xhigh thread `01a12523-4acb-7ff2-ad71-5eb2b8cbdbbd` 补设计后，design.md SHA256 `68e3b097a438a63a413d6029f48eec97ef3534e670a1b46dcc5a0dfa47c03144`；R2 快照 8,517 字节、SHA256 `83691c5127ed942f177820b1766f3d8d399e0a895c05c5b688a5ea6aa3bf07b7`，复审 turn `01a1253c-49ac-7e11-8bd1-aeb9e5f6788d` PASS。结论限定本轮设计与契约拒绝模拟，不代表锁屏能力已实施或验收。
