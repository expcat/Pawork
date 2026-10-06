# CU-01 · macOS 本机后台操作可行性与能力基线

> 状态：待开发（P0）。前置：无。更新：2026-10-06。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 目标

证明用户继续操作本机时，Agent 能在后台观察和操作目标应用；先取得真实支持边界，再进入原生重构。

## 当前依据与读取范围

2026-10-06 静态对比固定于 Pawork `87d878f65db634e64974cb920d0bfa76ad965f57` 和 LCU `21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a`。Pawork 已是 Rust RFB 引擎；LCU 是官方已安装运行时的包装与接入层，其 MIT 代码不提供底层引擎源码。本任务未做原生输入或实时验收。

[computer-use Spec](../spec/crates/computer-use.md)、[browser Spec](../spec/crates/browser.md)、[现行设计](../design.md#computer-use-首版)、[架构](../architecture.md)。

## 对比与目标边界

| 维度 | 已核实现状 | 本次升级目标 |
| --- | --- | --- |
| 引擎 | Pawork 是纯 Rust；LCU 调用原 runtime / Sky / REPL | 独立 Rust 能力实现，不依赖官方应用组件 |
| 桌面 | Pawork 只操作专用 Xvnc；无应用/窗口/AX 树 | 本机目标身份、窗口观测和受控元素动作 |
| 浏览器 | WebKit 基本 DOM；Chrome/Edge 是只读上下文 | 内置与外部浏览器的真实截图、操作和标签页 |
| 图片 | 进入模型并持久化；GUI 时间线仅投影文本 | 保留模型通路，补用户可查看证据 |
| 平台 | LCU Linux 部分 GTK/Qt 路径会激活窗口；Windows 仍是候选 | 不照搬会干扰用户的输入回退 |

依据：[LCU README](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/README.md)、[实现约定](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/AGENTS.md)、[Provenance](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/docs/PROVENANCE.md)、[Linux 输入限制](https://github.com/amontlabs/lcu/blob/21c4b5e70e9dedbc772f969cfdfbcc7ccc08139a/docs/upstream/openai-linux-window-input.md)、[官方 Computer Use](https://learn.chatgpt.com/docs/computer-use)、[官方 Browser](https://learn.chatgpt.com/docs/browser)。引擎接口对齐不等于已证明模型任务成功率与 Codex 相同。

## 实施范围

- 以 AppKit、Electron 和浏览器应用建立“应用 × 动作 × 窗口状态”矩阵；覆盖窗口被遮挡、用户在其它应用持续输入、取消和用户接管。
- 分别验证 ScreenCaptureKit 窗口截图、AX 读取/语义动作、定向文本/按键和指针动作；独立记录权限、签名/进程归属与调用线程要求。
- 连续观察用户鼠标、前台窗口、输入焦点和剪贴板；使用用户侧文本结果与目标侧 UI/文件事实核对，不能只比较操作前后两张截图。
- 只使用公开 API 和独立 Rust 实现。`CGEventPostToPid`、Private event source、进程锁或切回焦点都不能单凭 API 名称证明无干扰；需要实测。
- 明确普通后台、最小化和锁屏的实际支持边界；锁屏能力另见 [CU-24](cu-24-macos-locked-use.md)，不把未执行环境记为支持。

## 完成条件与验证

- 用户继续正常键鼠操作，Agent 不抢焦点、不移动用户指针、不串入用户输入或污染剪贴板；目标动作及新观测同时有证据。
- 用户开始操作同一目标时能停止竞争；没有前台激活、全局输入或事后恢复状态的隐式降级。
- 记录每类动作的通过、不支持和失败。未证明原生路径满足目标时，阻塞相关实现；容器通过不能替代本机后台验收。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

未实施；自动验证与真实验收均待执行。满足本任务全部条件后填写实现、实际命令、定向回归、真实/人工结果、遗留缺口和日期，再将 ROADMAP 对应项标为 ✅。

