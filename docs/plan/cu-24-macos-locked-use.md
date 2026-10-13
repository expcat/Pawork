# CU-24 · macOS 锁屏使用独立设计与验证

> 状态：设计与契约拒绝回归完成；特权组件未实施，系统安装、系统授权与真实锁屏验收待环境。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#未完成任务)

## 触发条件

用户明确启用锁屏功能后，单独裁决特权组件与系统授权方案。2026-10-10 用户限定只推进设计与可模拟拒绝边界，该触发尚未满足。

## 目标

研究并验证锁屏下的受限后台操作，不把普通 AX 后台操作等同锁屏支持。

## 当前依据与读取范围

官方锁屏使用需要单独启用的 macOS authorization plug-in，授权窗口与可信活动 turn 绑定；Pawork 无此实现，LCU 包装器不提供独立实现。

[架构](../architecture.md)、[安全规格](../spec/security.md)、[operations Spec](../spec/operations.md)、[computer-use Spec](../spec/crates/computer-use.md)、[官方锁屏使用](https://learn.chatgpt.com/docs/computer-use#locked-use)。

## 实施范围

- 显式开启、系统授权、短期 turn 身份、安装 / 卸载与失败处理按下方设计实施；不复制官方私有组件，也不作为普通辅助进程顺带安装。
- 只有有效任务可使用授权窗口，过期 / 取消 / 停用后立即失效；不成为通用远程解锁路径。
- 用户本机输入或接管时停止自动操作并恢复锁定保护，不让 Agent 与用户竞争。
- 特权、安全或冻结契约变化按仓库规则完成具体裁决后再实施。

## 完成条件与验证

- 设计的系统授权和输入保护有真实 macOS 验证，不能仅凭后台截图通过。
- 启停、任务取消、授权过期和用户输入均正确终止相应权限。
- 记录设计 / 实现 / 真实验收各自状态；未实施不标锁屏能力已完成，安装发布另行处理。

## 设计结论（2026-10-10，独立 review PASS）

- 锁屏使用必须由独立 Apple authorization plug-in 显式开启，不并入普通辅助进程、不随普通安装顺带安装；安装失败回退为未启用，卸载以系统侧 mechanism 移除为准。
- OS 层 Screen Recording、Accessibility 与 plug-in 安装全部由用户系统手势完成；Pawork 层复用 CU-16 授权台账与门序。
- 短期解锁尝试由宿主侧当前活跃可信 turn 签发与校验，模型不能签发。plug-in 参与前核对尝试仍对应当前活跃 turn / scope、尚未消费且 run 未终态；窗口仅覆盖这一次尝试，成功、失败、本地重锁或终态即作废，一次尝试最多一次解锁，普通 Once / ForRun 批准不能当作解锁许可。
- 本地键鼠输入立即重锁并暂停自动解锁；plug-in 状态、安装校验或授权不可判定时 fail-closed 拒绝。
- 停用请求先撤销未消费尝试、停止自动解锁并恢复锁定；mechanism 移除失败保持拒绝态并如实报告残留，不恢复已启用或自动解锁。审计复用现有日志通道，不新增第二台账。

## 当前状态

生产代码没有锁屏路径：`Environment` 只允许 `isolated_desktop` 与 `native_background`。`target::tests::locked_use_environment_is_rejected_by_the_contract` 钉住两值序列化，并证明 `environment: "locked_use"` 的完整 `TargetObservation` 反序列化失败；它是契约拒绝回归，不是锁屏验收。

## 剩余工作

1. 用户明确启用并裁决特权方案后，实施独立 authorization plug-in。
2. 真实环境验收 plug-in 安装、卸载与失败回退（含系统侧 mechanism 移除）。
3. 真机验收锁定 → 临时解锁 → 本地输入重锁 → 暂停自动解锁序列，以及 Screen Recording / Accessibility 的真实授予与撤销。
