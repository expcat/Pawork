# CU-22 · Windows 独立会话 / VM 后台能力

> 状态：未实现；独立 Windows 会话 / VM 前置未满足。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#未完成任务)

## 触发条件

准备独立 Windows 会话或 VM，并以不干扰用户主桌面为前提。

## 目标

通过 Rust Windows 系统 API 实现独立环境中的应用、窗口、UIA、截图和输入。

## 当前依据与读取范围

官方 Windows Computer Use 工作在前台活跃桌面，LCU Windows 在对比版本仍为候选，均不满足用户要求的同会话无干扰目标。

[computer-use Spec](../spec/crates/computer-use.md)、[transport Spec](../spec/crates/transport.md)、[架构](../architecture.md)、[官方 Computer Use](https://learn.chatgpt.com/docs/computer-use)。

## 实施范围

- 先验证独立登录会话或专用 VM 的捕获 / UI Automation / 输入和宿主隔离，不把 SendInput 注入用户主桌面。
- 在已有 computer-use 契约下增加必要 Windows 实现，Core 正式宿主仍是 pawork；guest 只承载受控系统能力。
- 首个切片完成真实窗口发现、UIA 和截图，再按共享任务补输入与生命周期；复杂 VM 管理不在首个切片。
- 拒绝权限 / UIPI 或目标会话不一致等真实条件，不以最小化窗口宣称后台隔离。

## 完成条件与验证

- 用户主桌面持续操作时，独立环境动作正确且不影响用户输入、焦点或剪贴板。
- 窗口、句柄、审批、停止与断线有真实 Windows 证据。
- 主桌面前台控制仅能登记为限制，不能作为本任务完成方式。

真实后台能力必须有真实目标状态和用户侧 / 文件 / Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 当前状态

- 没有 Windows UIA / 捕获 / 输入实现。`crates/computer-use/src/native_host.rs` 的 `HostSurface::local()` 在 macOS 装箱 `MacosNative`，其它平台装箱 `UnsupportedProbe`（进程存活恒 false，窗口代际、树版本、origin 与尺寸恒 None）；回归 `target::tests::unsupported_probe_fails_closed` 只钉这条拒绝边界，不运行 Windows 分支。
- `NativeHost::isolated()` 是 CU-21 的 Linux guest，不是 Windows 环境。transport 的 Windows Named Pipe 仍为 `cfg(windows)`，平台专项见 [ENV-02](env-02-platform-matrix.md)。
- 2026-10-12 核查本机（Darwin arm64）的 hypervisor、应用目录、VM 存储与 Docker（仅 linux/aarch64），未发现可用的独立 Windows 客户机；未下载 Windows 系统或创建付费实例。

## 剩余工作

1. 提供不干扰用户主桌面的独立 Windows 登录会话或专用 VM。
2. 在该环境实现并验收窗口发现、UIA 与截图，再补输入、停止与断线。
3. 验收权限 / UIPI / 会话不一致拒绝，以及用户主桌面持续操作时输入、焦点、剪贴板不受影响。
