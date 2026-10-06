# CU-17 · GUI 观测查询与协议版本接入

> 状态：待开发（P5）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

为 GUI 提供有界目标状态与截图证据查询，保留 Host 作为唯一入口。

## 当前依据与读取范围

TimelineItem 没有图片字段，join_text 只保留文本/视频说明；持久消息中的图片不能直接在 Timeline 展示。现有 ArtifactRead 提供有界读取，但模型 Artifact 图片解析并不完整。

[protocol Spec](../spec/crates/protocol.md)、[client Spec](../spec/crates/client.md)、[app Spec](../spec/crates/app.md)、[storage Spec](../spec/crates/storage.md)、[架构](../architecture.md)。

## 实施范围

- 先复用现有查询/图像组件能力，补最小观测引用与受限读取；不把巨幅 Base64 直接塞进每个 Timeline 帧。
- 查询绑定当前授权的 workspace/session 与目标证据，包含实际时间、尺寸和失效/历史状态；不得跨任务取图。
- 目标列表、审批与停止控制只按 CU-02 已裁决契约接入 registry，使用必要的最小新增消息。
- 冻结 wire 演进先完成具体裁决、版本协商、golden 与生成物；旧版本禁用新入口或明确拒绝。
- 模型图片仍走现有已支持编码，GUI 查询不迫使 Provider 改为尚未解析的 Artifact 来源。

## 完成条件与验证

- 有界查询能读到对应持久图片和真实状态，分页/重连不会错配身份。
- 跨 scope、超限、过期控制和旧版本被明确拒绝，历史读取不触发新截图或输入。
- protocol/client/app/storage 的受影响既有协议与持久化回归通过。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

