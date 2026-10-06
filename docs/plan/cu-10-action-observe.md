# CU-10 · 动作与新观测的 Host 闭环

> 状态：待开发（P3）。前置：[CU-04](cu-04-window-observation.md)、[CU-06](cu-06-semantic-actions.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

减少单独截图/动作消耗的回合，同时诚实区分输入派发和 UI 接受。

## 当前依据与读取范围

当前输入后由模型再调用 screenshot；输入成功只证明派发。ToolExecutionCompleted / MessageCommitted 与图片已经能进入持久消息并重放。

[tools Spec](../spec/crates/tools.md)、[engine Spec](../spec/crates/engine.md)、[app Spec](../spec/crates/app.md)、[持久化流程](../spec/flows.md)。

## 实施范围

- 一个有界工具调用执行一个动作并返回新观测，保留动作前观测的单次消费与 scope 校验。
- 结果记录输入是否派发、操作状态、新观测或观测失败；后置捕获失败不能抹掉已经发生的输入。
- 沿用现有工具消息、事件和 Provider 图片编码，不另建 Agent loop 或持久事件旁路。
- 取消/恢复和出错时不重执行动作，不引入无界批量命令或 JS REPL。

## 完成条件与验证

- 用户可通过新观测核对效果，动作状态不把派发成功等同任务完成。
- 输入已发生但截图失败时，结果仍保留真实状态且不自动重试。
- 工具结果和图像能持久化、重放，既有审批/恢复回归仍覆盖真实链路。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。
