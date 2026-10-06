# CU-15 · 外部 Chrome / Edge 的真实后台控制

> 状态：待开发（P4）。前置：[CU-01](cu-01-background-feasibility.md)、[CU-02](cu-02-target-contracts.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

通过 Rust 连接或原生消息通道控制授权浏览器标签页，补齐只读上下文以外的能力。

## 当前依据与读取范围

当前 external.rs 经 AppleScript 读取当前页 URL / 标题 / 有界正文，不导航或控制标签。LCU 浏览器接入依赖官方扩展和安装应用组件，不能直接当独立开源引擎。

[browser Spec](../spec/crates/browser.md)、[app Spec](../spec/crates/app.md)、[protocol Spec](../spec/crates/protocol.md)、[官方 Browser](https://learn.chatgpt.com/docs/browser)。

## 实施范围

- 先验证目标浏览器实际提供的授权 CDP 端点；不能假定普通已打开 profile 自动可连接。若必须控制普通用户标签，采用自有浏览器扩展与 Rust native messaging，保持 Core 和构建链无 Node/V8。
- 浏览器选择、标签 ID、网站授权、导航与重定向绑定真实连接代际；只操作明确目标。
- 实现截图、读取、点击、输入、导航及标签生命周期，不用前台激活或全局键鼠完成网页操作。
- 开发模式下受控 CDP 能力与一般网页动作区分，页面脚本不能调用宿主工具或绕过审批。
- 扩展由用户确认安装/启用，不复制官方私有组件；沿用 Host 授权链，必要布局或依赖变化先在 CU-02 中裁决。

## 完成条件与验证

- 真实 Chrome / Edge 中完成后台页面流程，用户在其它应用输入不中断。
- 用户已有标签不被清理，网站范围、连接断开和旧标签句柄正确拒绝。
- 有页面/请求/文件等独立结果与截图；不能把扩展发现或只读快照记为控制成功。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

