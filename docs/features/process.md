# Process 与 PTY Runtime

## 职责

在独立 crate 中承载跨平台子进程、进程树与集成终端，避免 Agent Engine、工具和 GUI 直接操作 `std::process::Command`。普通命令使用 `process-runtime`；需要终端语义的会话使用 `pty-service`，两者共用进程树终止契约。

## Process Runtime

`process-runtime` 同时提供缓冲执行与流式执行，支持并发读取 stdout/stderr、共享输出预算、截断标记、wall timeout、协作式 cancel 和外部 `ProcessHandle::kill()`。kill 幂等并最多等待 5 秒；句柄被丢弃时也会请求清理。

`CommandSpec::limits` 映射 CPU、内存、打开文件数和最大进程数：Unix 使用 `RLIMIT_CPU` / `RLIMIT_AS` / `RLIMIT_NOFILE` / `RLIMIT_NPROC`；Windows Job Object 使用 Job CPU 时间、Job memory 与 active-process limit。Linux 还可携带预先构造的 Landlock ruleset，子进程 `pre_exec` 只执行 `no_new_privs` 与 `landlock_restrict_self`。

进程树按平台实现：

- Unix 在 `pre_exec` 中执行 `setpgid(0, 0)`，终止时调用 `killpg(pgid, SIGKILL)`；Linux 同时设置 parent-death signal，并以 root start-time 防 PID 复用，先冻结 process group，再从 `/proc` 递归冻结/终止已通过 `setsid` 离组的后代。
- Windows 以 suspended 状态创建子进程，先绑定带 `KILL_ON_JOB_CLOSE` 的 Job Object，再恢复执行，避免子进程在绑定前逃逸；终止直接作用于 Job，不再调用 `taskkill /T`。
 - `ProcessTreeGuard::attach_external` 暴露给 PTY 等外部启动器；Windows 会在绑定根 PID 后收编绑定窗口内已产生的后代（以 creation time 排除 PID 复用误判），保证 `run_command`、Sandbox 与 PTY 使用同一终止语义。`attach_external` 的契约不对称已显式文档化：Unix 分支忽略 `limits`（仅用 pgid 构造守卫），Windows 分支用 `limits` 创建 Job Object；Unix 要求目标已是进程组 leader（否则返回 `InvalidInput`，由 portable-pty `setsid` 保证）；Windows 后代收养在 `spawn_blocking` 内同步执行，封顶 16 轮，每轮全量进程快照，最坏耗时取决于系统进程数，仅 PTY 会话路径触发。

## PTY Service

`pty-service` 采用 `portable-pty` 提供 ConPTY/Unix PTY 基础，并自实现会话层：

- `create` / `resize` / `write` / `subscribe` / `wait_exit` / `kill`；
- 多终端与 `SessionId` 归属校验；
- 有界字节环形缓冲、单调 cursor、快照与断线续读；cursor 已落后于缓冲起点时返回明确 stale 错误；
- 输出同时写入环形缓冲与 broadcast 事件流；当慢消费者导致 broadcast 槽位被覆写时，会话级 `dropped_events` 计数器递增并在 `PtySnapshot` 暴露，使重连消费者可感知丢弃事实（`PtyEvent` 序列化保持兼容不变）；
- `cleanup`、owner cleanup 与 shutdown 均幂等，终止后在有界时间内等待 waiter 回收子进程；
- reader/writer/wait/kill 等 blocking 操作不占用 async runtime worker；PTY spawn 使用短临界区串行化，规避 Unix musl 下并发 fork/`pre_exec` 路径导致宿主崩溃，同时不限制已创建会话的并发 I/O；
- PTY 子进程通过 `ProcessTreeGuard` 绑定 Unix session/process group 或 Windows Job Object，清理会话时连同后代一起终止；底层未返回 PID 或绑定失败时创建 fail-closed，不产生无树守卫会话。

PTY 输出不是 Agent Message Store 的一部分；只有用户或上层流程明确附加的内容才进入消息历史。

## 主流程集成边界

`process-runtime` 与 `pty-service` 的进程树终止、PTY 会话层已在代码层面实现并通过定向测试，但当前证据限于实现自身：`pty-service` 在整个 workspace 中尚无生产消费方（app-service 仅 mock 引用）。真实 agent 循环通过 PTY 执行交互式命令的通电发生在 GUI Connection Protocol（Phase 13 CLI Host 装配）与 P19-9 Terminal/Process 接线之后；当前不应据此误读为「PTY 已保护真实运行」。

## 验收状态

- [x] timeout、cancel、显式 kill 能在限时内清理完整进程树，kill 幂等
- [x] Linux `setsid` 离组后代有 `/proc` 冻结/终止兜底并通过真实内核回归
- [x] stdout/stderr 并发读取无死锁，超大输出受共享预算约束
- [x] Windows Job Object 在执行前完成绑定，句柄关闭触发整树回收
- [x] PTY 支持归属、多会话、重连、有界缓冲与自动清理
- [x] Windows 原生测试与 Linux WSL/musl 运行测试通过；Linux GNU、macOS aarch64 交叉编译通过

## 相关文档

- [tools（run_command）](tools.md) · [sandbox](sandbox.md) · [CLI Host](cli-host.md)
- [ROADMAP P4-12 / P11-6 / P11-7](../../ROADMAP.md)
