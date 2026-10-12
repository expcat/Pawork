# CU-22 · Windows 独立会话 / VM 后台能力

> 状态：当前环境核查与既有拒绝回归通过，独立 review PASS；独立 Windows 会话 / VM 前置未满足，原生实现与验收继续待环境。前置：[CU-02](cu-02-target-contracts.md)、[CU-09](cu-09-ownership-cancellation.md)、[CU-16](cu-16-target-approvals.md)。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#computer-use-rust-升级)

## 触发条件

准备独立 Windows 会话或 VM，并以不干扰用户主桌面为前提。

## 目标

通过 Rust Windows 系统 API 实现独立环境中的应用、窗口、UIA、截图和输入。

## 当前依据与读取范围

官方 Windows Computer Use 工作在前台活跃桌面，LCU Windows 在对比版本仍为候选。这不能满足用户要求的同会话无干扰目标。

[computer-use Spec](../spec/crates/computer-use.md)、[架构](../architecture.md)、[官方 Computer Use](https://learn.chatgpt.com/docs/computer-use)。

## 实施范围

- 先验证独立登录会话或专用 VM 的捕获/UI Automation/输入和宿主隔离，不把 SendInput 注入用户主桌面。
- 在已有 computer-use 契约下增加必要 Windows 实现，Core 正式宿主仍是 pawork；guest 只承载受控系统能力。
- 首个切片完成真实窗口发现、UIA 和截图，再按共享任务补输入与生命周期；复杂 VM 管理不在首个切片。
- 拒绝权限/UIPI 或目标会话不一致等真实条件，不以最小化窗口宣称后台隔离。

## 完成条件与验证

- 用户主桌面持续操作时，独立环境动作正确且不影响用户输入、焦点或剪贴板。
- 窗口、句柄、审批、停止与断线有真实 Windows 证据。
- 主桌面前台控制仅能登记为限制，不能作为本任务完成方式。

实施时优先受影响包的现有定向回归；真实后台能力必须有真实目标状态和用户侧/文件/Host 等独立事实。截图不写入仓库，证据注明日期、环境、输入、实际结果与位置。

## 完成记录

2026-10-12 执行对话 `01a12f10-136c-7de3-aa63-6692efa5978a` 使用 `xai/grok-4.7 xhigh` 完成当前前置核查与既有拒绝回归；未新增生产实现。基线为 `835538ef7c519c0e60657925ad28de5d7d160527`，证据位于 `/tmp/pawork-cu22-windows-isolation/evidence`。

宿主为 Darwin arm64 / macOS 26.6.2；`kern.hv_support=1` 只证明硬件支持虚拟化。Docker Desktop 服务端为 Linux / aarch64 / 29.6.1，当前 17 个镜像与可见容器未发现 Windows；本项所核 CU-21 和原 RFB 容器均为 Linux。核查同时覆盖常见 hypervisor 的命令与安装目录、系统 / 用户 / SSD 应用目录及 Spotlight、Homebrew / 软件包记录、标准 VM 存储目录、精确进程名与磁盘卷关键词，未发现可用的独立 Windows 客户机。`launchctl` 中两个 utm 子串属于输入法，未误计为 UTM。此结论限于本轮所核当前设施，不推断其它未提供连接的远程机器不存在。

现行 `platform_probe()` 在 macOS 装箱 macOS 后端，非 macOS 分支装箱 `UnsupportedProbe`；没有 WindowsNative / UIA 实现。`NativeHost::isolated()` 显式选择 CU-21 Linux guest，不是 Windows 环境。transport 的 Windows Named Pipe 仍为 `cfg(windows)`，此次未在 Windows 编译或运行。相关 [computer-use Spec](../spec/crates/computer-use.md) 与 [transport Spec](../spec/crates/transport.md) 的既有待验边界保持准确。

| 验证 | 实际结果与证据 |
| --- | --- |
| 当前主机、安装 / VM / 容器环境核查 | `env-probe`、`env-extra` 总体 exit 0；可用独立 Windows 前置未满足 |
| `cargo test -p pawork-computer-use --offline --lib unsupported_probe_fails_closed` | 1 passed / 104 filtered，exit 0；`unsupported-probe` |
| `git diff --check` | 执行原回执 exit 0；仅指挥的本项状态文档差异，无 Rust 写入集 |

拒绝回归直接验证现有 `UnsupportedProbe`：进程存活恒 false，窗口代际、树版本、origin 与尺寸恒 None。它在 macOS 测试进程中执行该替身后端，未运行 Windows 分支的 `platform_probe()`，也未证明 UIA、截图或输入可用。没有新增测试、交叉编译或主桌面 SendInput。

`env-probe` 内一次 `docker version` 模板子命令 exit 1（OS 字段未取到）；同次 `docker info` exit 0，给出真实 Linux 服务端事实。旧 `coordinator-*` 预检只作检索线索，不替代本次 `env-probe` / `env-extra`。未下载 Windows 系统、创建付费实例或修改系统权限。下一原生切片须先提供可用的专用 Windows 会话 / VM，再验证 UIA、捕获、输入与宿主隔离；本项不标 Windows 能力完成。

独立 `gpt-6.1-sol max` 审查 `01a12f18-d83e-7952-be96-68b6dcf1a5b3` / 回合 `01a12f18-de2a-79e0-adc3-9337b93dda3c` PASS，无阻断 / 非阻断问题。受审两文件快照 6,016 字节，SHA256 `973f8ed5be7bebc9c4e9a395c3c49797f8e502fbbe651d7636d9eb399e4c0f13`；PASS 后仅同步审查状态，最终差异及本地提交见证据目录 `FINAL.json` / `COMMIT.json`。

Validated: 当前环境核查；上述既有单测 1 passed / exit 0；文档链接与 diff；独立审查 PASS。
Targeted regressions: `target::tests::unsupported_probe_fails_closed`，仅现有拒绝边界。
Full workspace gate: NOT RUN（当前未设置全量门禁）

