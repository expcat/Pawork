# CU-12 · 请求侧截图历史与上下文预算

> 状态：待开发（P3）。前置：[CU-10](cu-10-action-observe.md)、[CU-11](cu-11-model-vision-gate.md)、[B10](b10-engine.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

限制发给模型的旧截图，保留完整持久化证据和历史重放。

## 当前依据与读取范围

旧截图持续留在工具历史中，`tool_result_trim` 未接生产循环；Chat / Responses 当前会丢弃 Artifact 类型的图像来源，不能只替换 ImageSource。

[engine Spec](../spec/crates/engine.md)、[models Spec](../spec/crates/models.md)、[storage Spec](../spec/crates/storage.md)、[providers Spec](../spec/crates/providers.md)。

## 实施范围

- 先与 [B10](b10-engine.md) 完成工具结果裁剪归属裁决，可同批处理；只接一条必要生产路径。
- 在请求副本按现有预算保留最新或当前任务必需的观测，保留旧动作文本与图像存在的明确说明。
- 数据库消息、事件和可审阅图片不裁剪；持久重放不变成再次截屏或输入。
- 维持已支持的 Base64 图像编码，确需 Artifact 时另证实完整解析链路，不用引用替代实际图像。

## 完成条件与验证

- 代表性多步任务的模型请求图像数量/字节受控，最新观测仍可用。
- 同一任务持久记录与重放证据完整，模型上下文裁剪不改变历史。
- 现有工具图像、Provider 编码与持久化回归通过，不新增完整上下文框架。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

