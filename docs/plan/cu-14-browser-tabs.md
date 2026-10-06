# CU-14 · 内置浏览器标签页与任务生命周期

> 状态：待开发（P4）。前置：[CU-13](cu-13-browser-observation.md)、[CU-09](cu-09-ownership-cancellation.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

将每任务单页扩展为有归属的多标签，支持选择与清理。

## 当前依据与读取范围

当前每任务一个内存 WebView，新窗口链接在当前页打开；没有多标签或跨启动恢复。

[browser Spec](../spec/crates/browser.md)、[desktop crate Spec](../spec/crates/desktop.md)、[app Spec](../spec/crates/app.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 在现有 browser / Desktop 消费面实现列出、创建、选择和关闭标签，绑定 session/run 与页面代际。
- 区分 Agent 创建的标签与用户已有标签；清理只作用于本任务拥有的目标，选择标签不自动前置系统窗口。
- 保留隔离非持久网站数据、隐藏/切任务保留和关闭释放的既有语义，不提前实现下载或跨启动 profile 管理。
- 新增 action 与 UI 先完成必要 wire golden/版本协商，不让旧 Host 接收未知命令。

## 完成条件与验证

- 多页操作归属准确，任务切换、取消、断线和关闭不影响其它任务或用户标签。
- 创建、选择和清理过程中用户键鼠/焦点不受影响，真实页面状态可核对。
- 旧 Host 明确禁用新能力，历史重放只展示，不再创建或关闭标签。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

