# Process 与 PTY Runtime

## 职责

在独立 crate 中处理跨平台进程与终端，避免 Agent Engine 直接操作 `std::process::Command`。

## Process Runtime

需解决：Unix 和 Windows 命令差异；Shell 选择；PATH 解析；子进程树终止；stdout/stderr 死锁；管道继承；退出后仍有后代进程持有句柄；超大输出；取消；timeout；Windows Job Object；Unix Process Group。

## PTY Service

不实现 TUI，但 GUI 集成终端仍需 Rust PTY 服务。功能：创建 PTY；指定 shell；指定 cwd；resize；stdin 写入；输出流；exit event；kill；多终端；重连；有界缓冲；Session 归属；自动清理。

PTY 不属于 Agent Message Store，除非用户明确附加输出。

## 验收标准

- 取消命令能清理进程树（三平台）
- stdout/stderr 无死锁
- 超大输出受控
- PTY 可重连、自动清理

## 相关文档

- [tools（run_command）](tools.md) · [sandbox](sandbox.md) · [CLI Host](cli-host.md)
- [ROADMAP P4-12 / P11-6 / P11-7](../../ROADMAP.md)
