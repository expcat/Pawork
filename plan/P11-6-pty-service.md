# P11-6：PTY Service

> Phase 11 · Sandbox 与跨平台强化 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P4-12

**最终目的**：实现集成终端 PTY 服务（创建/resize/write/exit/kill/重连/有界缓冲），支持终端会话归属与自动清理。

**涉及范围**：`pty-service`

## 细分步骤

1. **PTY 创建/resize/write/exit/kill** —— 目的：终端基本操作。
2. **重连 + 有界缓冲** —— 目的：断连可恢复。
3. **session 归属与自动清理** —— 目的：资源不泄漏。
4. **三平台测试** —— 目的：一致可用。

## 主要产出物

- `pty-service` crate

## 验收标准

- [x] session 归属、多会话、owner cleanup 与 shutdown 正确且幂等
- [x] 快照、单调 cursor 与有界环形缓冲支持断线续读并报告 stale cursor
- [x] resize/write/output/exit/kill 可用，blocking I/O 不阻塞 async runtime
- [x] PTY 复用 `ProcessTreeGuard`，清理会话会回收后代

## 验证记录（2026-08-09）

- 新增 workspace member `pty-service`；在 `portable-pty 0.8.1` 上自实现会话层，未引入 Node/JS runtime。
- Windows 原生 11/11 通过，PTY 后代清理重复压力 30/30；Linux musl 测试包在 WSL 默认并行运行 11/11 通过，覆盖 concurrent spawn、后代清理与重连。
- PTY child 未暴露 PID 或 `ProcessTreeGuard` 绑定失败时创建 fail-closed，先 kill/wait 再返回错误，不保留无整树守卫的会话。
- Linux GNU、Linux musl 与 macOS aarch64 目标编译通过。musl 压力测试暴露的并发 PTY spawn 崩溃已用短临界区修复；已创建会话的 I/O 仍并发。

**相关文档**：[process](../docs/features/process.md) · [ROADMAP](../ROADMAP.md)

**依赖决策（2026-08-09）**：采用 workspace 锁定的 `portable-pty 0.8.1` 作为最小 PTY 基础；重连、有界缓冲、归属、并发 spawn 防护与自动清理由 Pawork 自实现。升级或换 fork 需重跑三平台 PTY/进程树门禁。
