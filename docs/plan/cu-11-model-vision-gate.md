# CU-11 · 图像能力的前置工具门控

> 状态：待开发（P3）。前置：[CU-02](cu-02-target-contracts.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在工具暴露和执行前拒绝不支持图像的模型，避免截屏后才失败。

## 当前依据与读取范围

现有协商会扫描工具消息中的图像；文本模型可能先看到 computer 工具，截图进入下一轮请求后才被拒绝。

[models Spec](../spec/crates/models.md)、[app Spec](../spec/crates/app.md)、[tools Spec](../spec/crates/tools.md)、[providers Spec](../spec/crates/providers.md)。

## 实施范围

- 复用现有模型能力和工具 registry/scheduler，在暴露或派发视觉操作前检查 image input 能力。
- 兼顾切模型和恢复已有图像上下文，错误应指出实际能力缺失，而非报告后端断开。
- 能力裁决留在现有 app/models 责任范围；Engine 不按 Provider 名称分支，不增加具体 Provider 依赖。
- 与 [B5](b5-models.md) 协调公开 API 裁决，不新造一套能力目录。

## 完成条件与验证

- 文本模型在截图、输入或连接后端前被正确门控，后端访问次数为零。
- 支持图像的模型仍走现有三协议图像编码，嵌套工具图像的既有回归保留。
- 使用真实功能模型时按验证规格临时覆盖，缺能力或连接失败如实记录。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

