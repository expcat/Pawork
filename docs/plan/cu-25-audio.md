# CU-25 · 显式授权的目标音频采集

> 状态：条件性待开发（后续能力）。前置：[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)、[CU-20](cu-20-native-acceptance.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

用户选择音频使用场景并显式开启后开展，不默认采集。

## 目标

补齐目标音频录制或模型使用链路，并明确二者的能力边界。

## 当前依据与读取范围

LCU 音频是原 runtime 的可选录制 API；保存录音不等于把音频送入模型。Pawork 当前没有此 computer-use 音频链路。

[computer-use Spec](../spec/crates/computer-use.md)、[providers Spec](../spec/crates/providers.md)、[models Spec](../spec/crates/models.md)、[安全规格](../spec/security.md)、[LCU 音频说明](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)。

## 实施范围

- 先确认录制给用户还是作为模型输入、目标范围、格式/时长/字节预算及权限；不顺势录制其它用户应用的系统音频。
- 以 Rust 系统采集 API 实现实际支持的目标范围；若无法隔离范围，明确不支持而非默认录全桌面。
- 停止、取消、撤销和任务结束释放采集。若交给模型，先验证模型/Provider 音频能力与完整编码通路。
- 需要演进媒体、持久化或 wire 时先做对应裁决和 golden；不把仅有文件保存记为模型听到音频。

## 完成条件与验证

- 真实音频来源、权限、预算和停止行为可核对，用户正常操作不受影响。
- 录制产物与模型实际收到的内容分开验收；未支持模型输入时保持明确限制。
- 未启用时零采集，取消或撤销后不继续录制，不含未授权应用内容。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

