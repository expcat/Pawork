# ENV-02 · Windows 原生平台专项验收

> 状态：macOS UDS / Desktop 回收 / 系统 WebKit 已于 2026-10-12 真机通过；Windows 实例锁、Named Pipe、连接回收与 Exec Job 内核限额未验收。阻塞：缺可用 Windows 客户机，交叉类型检查不算 Windows 执行。更新：2026-10-13。

[返回 ROADMAP](../ROADMAP.md#环境待验)

## 目标

在 Windows 原生环境验证已有 `cfg(windows)` 实现，不把 macOS 或交叉类型检查推广为 Windows 通过。computer-use 的 Windows 能力另见 [CU-22](cu-22-windows-isolation.md)。

## 当前依据与读取范围

[验证规格](../spec/verification.md)、[gui-server Spec](../spec/crates/gui-server.md)、[transport Spec](../spec/crates/transport.md)、[exec Spec](../spec/crates/exec.md)、[Desktop Spec](../spec/desktop.md)。

## 完成条件

- 各项在 Windows 上有实际启动、操作与回收证据，缺环境保持待验。
- 实例、连接与进程限额结果与合同一致；macOS 上过滤到 0 个测试不得记为通过。

## 已核实

- 2026-10-12 macOS 26.6.2 arm64：独立 Host 与 Desktop 的 UDS socket 为 `0600`，Desktop 退出后 Host 仍监听、重连成功，回收后 socket 关闭；正式 Inspector Browser 经系统 WKWebView 打开本地页面，服务器收到 `GET / 200`。transport 7 项、gui-server Unix 会话与 multi-GUI 回归通过。
- Windows 实现与测试仍在源码中：`crates/auth/src/file_backend.rs` 的 `try_acquire_file_lock` 用 `share_mode(0)`（app 实例锁复用）；`crates/transport/src/local_windows.rs` 的 Named Pipe listener 以 `close_watch` 唤醒挂起 connect；`crates/exec/src/os/windows.rs` 的 `job_limits_are_installed_in_kernel` 用 QueryInformationJobObject 读内核 Job 限额。B1 时 `cargo check -p pawork-exec --offline --tests --target x86_64-pc-windows-msvc` 交叉类型检查通过，但未在 Windows 执行。
- 当前本机可见安装 / VM / Docker 范围没有可用 Windows 客户机。2026-10-13 的 Linux 盒（Debian 13 x86_64）同样没有：有 `/dev/kvm` 但无 QEMU / libvirt / VirtualBox / Docker / Podman，也无 Windows 镜像；准备 Windows VM 属用户决策，与 [CU-22](cu-22-windows-isolation.md) 共用同一前置。

## 剩余待验（前置：可用 Windows 客户机）

1. **实例锁**：`try_acquire_file_lock` 的 `share_mode(0)` 跨进程互斥——第二进程得到 `None`，释放后可再取。
2. **Named Pipe**：listener `close` 唤醒已挂起的 `connect`，随后新连接失败、端点可回收；现有 `listener_close_stops_accepts` 只断言 close 之后的 accept 失败，不覆盖挂起中被唤醒。
3. **原生 GUI 连接回收**：close 后客户端看到连接关闭，不留下占用中的管道实例。
4. **Exec Job 限额**：在 Windows 执行 `cargo test -p pawork-exec --offline --lib job_limits_are_installed_in_kernel` 并记录结果。
