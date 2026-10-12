# ENV-02 · 既有跨平台专项环境验收

> 状态：当前可运行专项核实完成，独立 review PASS；macOS UDS / 回收 / WebKit 真窗口通过，已有 Linux 与 Provider 证据按范围复用；Windows 原生仍待环境。更新：2026-10-12。

[返回 ROADMAP](../ROADMAP.md#缺环境与外部条件)

## 目标

补齐 Linux / Windows、WebKit、容器和真实 Provider 的既有专项矩阵。

## 当前依据与读取范围

重新核当前 macOS、专用 Linux 容器、真实 Provider 及 Windows 前置；已审证据按实际日期、版本与链路复用，不推定整个 HEAD 相同。

[验证规格](../spec/verification.md)、[gui-server Spec](../spec/crates/gui-server.md)、[transport Spec](../spec/crates/transport.md)、[Desktop Spec](../spec/desktop.md)。

## 验收范围

- 按当前实现选择对应平台与真实环境，不把一般本机测试推广为跨平台通过。
- Windows 核对实例锁 `share_mode`、listener close/connect 唤醒和连接回收。
- 新增 computer-use 平台能力独立见 [CU-21](cu-21-isolated-linux.md)、[CU-22](cu-22-windows-isolation.md)、[CU-23](cu-23-wayland-feasibility.md)，不重复声明已实现。

## 完成条件

- 各选定环境有实际启动、操作与回收证据，缺环境保持待验。
- 专项实例、连接和 Provider 真实终态与合同一致。

## 完成记录

2026-10-12 在 `main@9556f277`、macOS 26.6.2 arm64 核实下列矩阵。未改生产源码、依赖或测试资产；指挥只同步状态文档。

| 环境 / 专项 | 实际输入与结果 | 证据边界 |
| --- | --- | --- |
| macOS UDS / Desktop 回收 | 独立 `/tmp` Host 与 `Pawork-env02.app`，socket `0600`；Desktop 3507 显示 `Local · Connected`，退出后 Host 仍监听，4061 重连成功；随后本项进程回收、socket down，原有 Pawork 进程保留 | 正式产品实例；临时数据与 HOME 不带用户凭证，无新 Provider 推理 |
| macOS 系统 WebKit | 正式 Inspector Browser 地址输入并打开 `http://127.0.0.1:18702/`；17:49:21 本地服务器 `GET / 200`，AX 标签 / 状态为 `ENV02-WEBKIT-OK`，记录同次 WebKit 子进程及退出 | 独立真窗口与系统 WKWebView；本项没有截图，不声称像素 / 焦点 / IME 验收 |
| 连接定向回归 | 一次 Cargo exit 0：transport 7、gui-server Unix 6、multi-GUI memory 3，共 16 passed | 前 13 项使用真实本地 socket；后 3 项是 MemoryTransport；0-test 库 / doc-test 目标不计通过 |
| Linux / 专用 X11 | [CU-21](cu-21-isolated-linux.md) 的 `cu21-live-test-12` exit 0、1 passed；当前专用容器仍在，无 mount，镜像 `167603772419…` | 复用正式 Host / Tool / Scheduler / Engine / Storage 与隔离 Firefox；Provider 与用户批准模拟，本项未重跑 |
| 真实 Provider | [B20](b20-desktop-probe-models.md) 的 `probe-smoke-final` exit 0，`glm-coding/glm-5.3` 返回 `pong`，断线后 Run 仍 running | 复用真实 GLM 与 probe；不是本轮新请求或真窗口，审批未触发、模型切换未覆盖 |
| Wayland | [CU-23](cu-23-wayland-feasibility.md) 的 Sway 专用 compositor；`run3-dev-click` exit 0，容器 `/dev/cu23-vpointer` 点击红→绿、按键→蓝 | 可行性实验，非生产 Wayland 支持；`run2-files` 与成功资产相同，不用作失败截图 |
| Windows | 当前可见安装 / VM / Docker 范围未发现可用 Windows 客户机；既有 [CU-22](cu-22-windows-isolation.md) 前置核实适用 | `share_mode(0)`、Named Pipe close/connect、原生连接回收未执行，仍待 Windows 环境 |

复用时核对了必要源码版本：`835538ef..9556f277` 无 Rust 差异；CU-21 的 30 文件快照仅 ROADMAP 文档与当前树不同；B20 的 Desktop main / bus / run_start、transport、gui-server、browser macOS 路径自 `afeb9d87` 无提交改动。以上只支持所列链路复用，不声称所有生成物等于当前整个 HEAD。

本项证据为 `/tmp/pawork-env02-platform-matrix/evidence`，包括 `cargo-uds-recovery` 原命令 / stdout / exit、各 AX 动作、`ax-after-go2`、`ax-reconnect`、WebKit 进程与回收记录。指挥从执行对话保存的 `coordinator-native-receipts.json` 保留实际 HTTP `GET / 200`、Host 与回收工具回执及原截断标记；HTTP 服务随后由本项 SIGTERM 停止，原 exit 143 保留，不写成正常自然退出。

复用证据：`/tmp/pawork-cu21-isolated-linux/evidence`、`/tmp/pawork-cu22-windows-isolation/evidence`、`/tmp/pawork-cu23-wayland-feasibility/evidence`、`/tmp/pawork-b20-probe-acceptance/evidence`。CU-23 第二轮失败只依据 `run2-css-eventbox.stdout/.exit/.command.json`（exit 1），原失败保留；26 个 `run2-files` 文件与成功轮逐字节一致。

Validated: `cargo test --offline -p pawork-transport -p pawork-gui-server --features pawork-transport/memory -- <16 filters>`（完整过滤名见原 command.json）；exit 0、16 passed；独立 UDS / Desktop / WebKit 的 AX、HTTP 与回收取证；文档链接与 diff 检查。

Targeted regressions: 帧与权限、超限拒绝、pending accept close、旧端点归属、心跳、断线不取消 Run、resume / replay 和 wait_done；未新增测试。

Full workspace gate: NOT RUN（当前未设置全量门禁）。

