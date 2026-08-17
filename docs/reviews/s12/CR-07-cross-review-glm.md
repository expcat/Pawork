# CR-07 High Findings 交叉复核（GLM）

- 复核对象：[CR-07-protocol-host-clients.md](CR-07-protocol-host-clients.md) 中 3 条 High（S12-CR07-01 ~ 03）
- 复核人：zai/glm-5.3（glm_reviewer）
- 复核日期：2026-08-18
- 方法：不采信报告转述，逐条独立打开源码（路径+符号+行号）核对实际行为；遵守 S12 只读纪律，未运行任何构建/测试/二进制。

## 裁定表

| 编号 | 原严重度 | 裁定 | 一行理由 |
| --- | --- | --- | --- |
| S12-CR07-01 | High | uphold（维持 High） | TokenAuthenticator 全仓库零生产接线、握手缺认证即 Accepted、socket/目录无权限收紧逐行核实；唯一证据瑕疵是「umask 022 → 他人可 connect」高估了跨用户面，但同用户任意进程可驱动 Run/PTY/审批已足以支撑 High。 |
| S12-CR07-02 | High | uphold（维持 High） | 队列满丢新事件与 broadcast Lagged 两条路径均只置内部 lagged 标记、零客户端信号，且该标记生产代码从不读取；headless 通道同类场景已按 fail-closed 发 Backpressure 错误帧，反证 gui-server 缺陷真实且违约。 |
| S12-CR07-03 | High | adjust-severity（降为 Medium） | check/record TOCTOU、record 错误被吞、文档「绝不重复执行」承诺违约全部成立；但报告的触发面被高估——GUI 单连接串行、多连接按 client 隔离 key、headless 单循环串行、跨通道不共享进程内表，现实只剩 ACP 同 id 并发重试一条窄路径。 |

## 逐条复核记录

### S12-CR07-01 — 生产 gui serve 无认证（uphold，维持 High）

- 核心接线全部核实：
  - host/cli/src/gui.rs run_gui 39-49：HandshakeService::new 只带 5 项 capability（Events/Snapshots/ArtifactStreaming/TerminalStreaming/Approvals），无 with_authenticator；全仓库 rg with_authenticator|TokenAuthenticator 仅命中 foundation/protocol 测试与 feature 门控的 remote transport（host/transport/src/remote/session.rs），本地生产装配零调用。
  - foundation/protocol/src/handshake.rs HandshakeService 92（authenticator: Option）、105（new 置 None）、144-156（仅 Some 时要求 ClientAuthentication；None 时缺 proof 直接进入 capability 协商并 Accepted）。
  - foundation/protocol/src/client_auth.rs TOKEN_SCHEME=23、TokenAuthenticator 169-208（scheme 校验 + constant_time_eq）实现完整，即「已有件未接线」。
  - host/transport/src/local_unix.rs bind 46-51：UnixListener::bind 后无 chmod/属主收紧；host/transport/src/local_windows.rs accept 101-112：ServerOptions::new().create 未设安全描述符。
  - host/cli/src/gui.rs 29-30 + host/app/src/data_dir.rs default_data_dir 10-26 + host/cli/src/ops.rs gui_socket_path 27-30：socket 落 ~/.pawork/pawork-gui.sock，create_dir_all 不设 0700。
  - host/gui-server/src/session.rs host_stamp_command 347-359、handle_frame 396-397：任意连接的客户端自报 identity 被覆盖为 LocalUser{actor_id=client-N} 后直接进入 adapter.command。
  - host/cli/src/service.rs install_definition 53-68：service install --apply 把同一无认证 gui serve 写成 launchd/systemd 常驻。
- 证据修正（不影响裁定）：报告括注「常见 umask 022 → 他人可 connect」不成立——umask 022 下 socket 为 0755，Linux/macOS connect(2) 对 UDS 要求写权限，其他用户通常被拒；跨用户现实面是 umask 0/002（部分服务上下文）或 Windows 默认 pipe DACL。但同用户任意进程可无条件连接并驱动已登录 Provider 的 Run、写 PTY（叠加 S12-CR03-02 PTY 无沙箱）、代为审批，service install 又使其常驻，High 维持。
- 附带核实：apps/protocol-probe/src/main.rs 104-107 scheme="token" 与 TOKEN_SCHEME="pawork-token" 不一致（CR07-06 关联成立）。

### S12-CR07-02 — Lagged 静默丢事件（uphold，维持 High）

- 服务端路径逐行核实：
  - host/gui-server/src/connection.rs 模块文档 8-9、DEFAULT_QUEUE_CAPACITY=1024（24-25）、enqueue 287-311：try_send Full → entry.session.lagged=true 并丢弃**新**事件，返回 ManagerError::Lagged。
  - host/gui-server/src/session.rs spawn_forwarder 571-585：enqueue Lagged → continue；broadcast RecvError::Lagged → 仅 mark_lagged；两条路径都不向客户端发任何帧、不断连。
  - manager_error_frame 617-632 虽将 Lagged 映射为 ReplayUnavailable，但调用点仅 Subscribe 462 / Unsubscribe 471 失败分支，事件热路径不可达。
  - connection.rs lagged 字段全量 rg：生产代码只写（302、320）不读，仅测试断言（405、486、502）——不存在任何恢复触发。
  - host/app/src/hub.rs with_capacity 64-76、publish 83-96、HubSubscription::recv 170-178：broadcast 有界（默认 4096），慢订阅者 Lagged 后 ring replay 是唯一补偿。
- 客户端无自愈：clients/gui-client/src/lib.rs 全文无 sequence gap 检测逻辑（rg lagged/gap/resync 无命中）；apps/desktop/src/controller.rs 仅 record_last_acked（143、182-183），不比较连续性。
- 反证强化：host/cli/src/headless.rs poll_event 229-233 对 SDK 通道的 TryRecvError::Lagged 显式回 Backpressure 错误帧——fail-closed 模式在本仓库已有实现，gui-server 未跟上。
- 契约基准：docs/gui-design.md §4.1 第 4 点要求落后不可补时以 SnapshotRequired 替换基线重建；当前实现违反。
- 裁定：慢客户端永久静默缺事件（含 Run 终态/审批/PTY 输出）且无任何通知或自愈，High 维持。

### S12-CR07-03 — 幂等 check/record 非原子（adjust-severity → Medium）

- 事实全部核实：
  - host/app/src/idempotency.rs 文档 3-5 承诺「相同标识……绝不重复执行」；check 85-115 命中失败即返回 New、不预留占位；record 118-149 另起一次锁、冲突返回 DuplicateCommand/KeyConflict。两段之间无 in-flight 态，纯内存无持久化。
  - host/app/src/gui_host.rs command 892-918：check → await dispatch_command（副作用）→ record，910 行 let _ = 吞掉 record 全部错误；scoped_idempotency 1537-1559：GUI 用 {client_id}/{command_id}，Automation 用原始 command_id。
  - host/channels/src/acp/adapter.rs command_envelope 290-303：ACP command_id=acp-{request_id}；foundation/protocol/src/app/version.rs 87：DEFAULT_CONTROL_PLANE_TENANT="local/default"。
- 降级理由（触发面修正）：
  - GUI：gui-server 每连接 handle_frame 串行 await，同连接超时重试会在首次 record 之后才处理（check 命中 Replay，幂等按设计生效）；报告所列「GUI 多连接竞态」不成立——多连接 key 带 client_id 前缀（1539-1559），本就不可能同 key 碰撞。
  - headless/SDK：host/cli/src/headless.rs 44 单 handler stdio 串行循环，无并发入口。
  - 「跨通道共享同一张内存表」为误读：gui serve / acp / headless 各自独立进程、各自实例化内存 store，local/default tenant 不产生跨进程共享。
  - 现实唯一并发向量：host/cli/src/acp.rs run_frame_loop 141-153 对 session/prompt tokio::spawn 并发处理，同一 JSON-RPC id 在原始请求仍在 dispatch 窗口内重发才会双执行；RunStart dispatch 含 models_overview 探测/模型切换（gui_host.rs 1000-1150）窗口可达数秒，但规范 JSON-RPC 客户端同 id 重试并非常态。
- 仍成立的缺陷：store 核心承诺在网络重试场景下被 TOCTOU 打破，record 失败被静默吞掉，任何未来并发宿主接入即触发；双执行后果为重复 Session/Run 落库与重复 Provider 调用。按当前可达触发面，Medium 更贴切；若出现新的并发命令入口应回升。

## 结论

- S12-CR07-01、02 维持 High；S12-CR07-03 调整为 Medium（Bug，Confirmed）。
- CR-07 报告证据路径全部真实存在、行号基本准确；CR07-01 的 umask 括注与 CR07-03 的触发面表述需按上文修正后回写。
