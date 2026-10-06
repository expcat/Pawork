# CU-13 · 内置浏览器截图与 DOM 观测

> 状态：待开发（P4）。前置：[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

在现有系统 WebView 上补截图与稳定 DOM 观测，保持用户可同时使用 Pawork。

## 当前依据与读取范围

现有 WebKit 支持有界 read/click/type 等固定 DOM 操作，没有截图或观测元素句柄；BrowserView 位于 AppKit 主线程，不能 Send / Sync。

[browser Spec](../spec/crates/browser.md)、[app Spec](../spec/crates/app.md)、[desktop crate Spec](../spec/crates/desktop.md)、[protocol Spec](../spec/crates/protocol.md)。

## 实施范围

- 复用 Host → GUI 请求/回执桥，截图、DOM 查找、元素动作返回目标/页面代际及新观测。
- 固定受限 DOM 脚本运行在系统网页内容进程，不引入 Core JS Runtime 或网页到工具的桥。
- 页面导航、重载、DOM 变化使旧句柄失效；保持唯一可见匹配、密码/文件输入拒绝和输出预算。
- 在隐藏/切任务及用户操作 Composer 时验证截图与 DOM 操作不抢系统焦点；不能以 AX 状态正常代替实际输入验证。
- 涉及浏览器 wire 形状时按 CU-02 的已裁决契约先做 golden/版本门控，再接操作实现。

## 完成条件与验证

- 真实 WebView 的截图和 DOM 结果对应同一目标页面，用户输入连续不受影响。
- 旧页句柄、歧义、越权、拒绝和超时均有明确结果，历史重放不重新执行网页动作。
- 页面与 HTTP 请求/DOM 事实联合验证，不能只用浏览器脚本自测证明产品通过。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

