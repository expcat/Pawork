# pawork-exec

进程树、沙箱与 PTY。无内部 `pawork-*` 依赖。R7 沙箱演进的承载包。

## 职责

拉起、限制、取消子进程（Job Object / 进程组），按平台探测沙箱后端（Seatbelt / bwrap+Landlock / AppContainer），并提供可重连的 PTY 环形缓冲。不依赖 `pawork-domain` / `pawork-policy`（路径与取消令牌为本 crate 自有类型）。PTY 输出留在本模块，**不写入** Agent Event Store。

## 模块树

```
src/
  lib.rs
  cancel.rs          # pub mod；本 crate 取消令牌
  process.rs         # CommandSpec / ProcessRuntime
  sandbox.rs         # 策略、后端选择、软/硬隔离
  tree.rs            # ProcessTreeGuard
  path.rs            # crate 内路径判定（不对外）
  os/{linux,macos,windows}.rs   # 平台探测与 profile，多为 pub(crate)
  pty/{mod.rs,buffer.rs}
```

无 `tests/` 目录；回归在各文件 `#[cfg(test)]`。

## 对外入口/API 面

`lib.rs` re-export：

- `CancellationToken`（亦经 `pawork_exec::cancel`）
- 进程：`CommandSpec`、`ProcessRuntime`（`run` / `spawn_stream` / `spawn_interactive`）、`ProcessHandle`、`ProcessEvent`、`ProcessLimits`、`ProcessError`
- 沙箱：`SandboxPolicy`、`SandboxSelector::pick`、`SandboxBackend`、`SandboxProcess`、`IsolationLevel`、`NetworkMode`、`FilesystemPolicy`、`NativeRestricted`、`default_secret_paths`、`default_env_allowlist`
- 进程树：`ProcessTreeGuard`（`attach_external` / `terminate`）
- PTY：`PtyService`、`PtyCreateSpec`、`PtySnapshot`、`RingBuffer`、`TerminalId`、`DEFAULT_BUFFER_CAPACITY`

平台专用符号（`LinuxLandlockPolicy`、Seatbelt profile 生成、AppContainer 配置）为 `pub(crate)`：R0 D21 包外零消费；R7 再评估可见性。

## 依赖与被依赖

- **依赖**：无 `pawork-*`。`tokio` / `portable-pty` / `tracing`；Linux `landlock`；Windows `windows`（Job Object 等）。
- **被依赖**：`pawork-tools`（`run_command`）、`pawork-git`、`pawork-app`。
- **刻意不依赖本包**：`pawork-engine`（杀树由宿主注入）、`pawork-policy`、`pawork-workflow`。

## 红线与注意事项

- fail-closed 对沙箱是 **可观测回退**（落到 `NativeRestricted`），不是拒跑（ADR-031）；CLI/GUI 必须展示 fallback。
- `NativeRestricted` 不是对抗性隔离，挡不住显式读 Secret 文件。
- `default_secret_paths()` 是 Secret 目录 deny 的单一来源（含 `~/.pawork/auth.json`），与 builtin `run_command` 共用。
- 文件工具路径红线在 `pawork-policy`；本 crate 接收已解析 `PathBuf`。
- PTY 入 policy 闸是 R7 工作，当前不要假定 PTY 已走审批内核。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §3（Policy / 沙箱）
- [plan/R7-sandbox-isolation.md](../../plan/R7-sandbox-isolation.md)
- [AGENTS.md](../../AGENTS.md) §8
- [代码地图总索引](../../docs/code-map/README.md)
