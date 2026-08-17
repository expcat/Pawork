# S12 CR-07 审查报告：协议 / 宿主 / 客户端全链路

| 项 | 值 |
| --- | --- |
| CR 编号 | CR-07 |
| 主审范围 | foundation/protocol、host/app、host/gui-server、host/transport、host/channels、clients/gui-client、clients/sdk、clients/compat、apps/protocol-probe（含 tests） |
| 审查日期 | 2026-08-18 |
| 主审模型 | Grok（xai/grok-4.6） |

## 实际审查路径

- plan/S12-project-code-review.md、ROADMAP.md §3.2 K-01～K-10、docs/task-guide.md §3.1、docs/design.md §3.2、docs/gui-design.md §4 / §4.1、plan/S10-serve-clients.md 相关退出项
- foundation/protocol/src/{lib,handshake,client_auth,resume,codec,typegen}.rs、foundation/protocol/src/app/{command,version}.rs、foundation/protocol/tests/{golden,handshake,resume,typegen}.rs
- schemas/core-api/versions.d.ts、schemas/gui-protocol/AppCommand.d.ts、foundation/protocol/tests/golden/client_handshake.json（schema / golden 抽查，未跑 typegen）
- host/cli/src/{gui,ops,service,headless,adapter,acp}.rs（生产装配点；CLI 进程/服务路径主审归 CR-03，此处只追协议接线）
- host/gui-server/src/{lib,session,connection}.rs 与 host/gui-server/tests/{session,multi_gui_runtime}.rs
- host/app/src/{gui_host,hub,idempotency,data_dir}.rs（EventHub / 幂等 / 命令装配；rate_limit.rs 仅对照 K-07）
- host/transport/src/{local,local_unix,local_windows,api}.rs；remote/* 仅确认 feature 门控与 token 存在，不深审未激活 TLS 路径
- host/channels/src/acp/{host,adapter,map,wire,command_host}.rs
- clients/gui-client/src/lib.rs、clients/gui-client/tests/contract.rs、clients/sdk/src/{client,transport}.rs、clients/compat/src/{apply,limits,io}.rs
- apps/protocol-probe/src/{main,scenarios}.rs
- 跨包消费抽样：apps/desktop/src/controller.rs 的 Resume/Ack 调用（主审归 CR-08，此处只核对接线）
- 已有报告：docs/reviews/s12/CR-01-manifests-layout.md、CR-03-exec-cli.md、CR-05-persistence-ledgers.md（跨包只链接，不重复建号）

## 未覆盖路径与原因

- 未执行 cargo test/build/clippy/fmt、pawork / Desktop / protocol-probe 真机冒烟：S12 任务书禁止。
- host/transport remote feature（TCP+TLS）未作为生产 gui serve 装配路径深审；只确认 token/fingerprint 存在且默认关闭。
- Windows Named Pipe 仅源码审查，本机无法跑 Windows 运行时证据。
- schemas/ 未全量逐文件对照 typegen 输出；只核对 API_VERSION 与 AppCommand 形状。K-08 ArtifactStreaming 能力不一致只引用基线，不新建 finding。
- clients/compat 只审入口、上限与「只写计划、不改源」边界，不逐条复核全部 parser/fixture。
- Desktop 投影/订阅在落后后的 UI 表现归 CR-08；本包只登记协议层未发信号。
- Fork 后 resume_messages 混入其它 branch：已由 [S12-CR05-01](CR-05-persistence-ledgers.md) 登记，本包只链接。
- GUI PTY 绕过沙箱：已由 [S12-CR03-02](CR-03-exec-cli.md) 登记，本包不重复。

## 已知基线（审查过、不新建）

- K-07：host/app/src/rate_limit.rs 有实现与测试，生产 EventHub / GUI 广播热路径无调用。背压可观测性缺口与本包 S12-CR07-02 叠加，但限流模块本身不重复建号。
- K-08：host/cli/src/gui.rs 与 clients/gui-client 宣告 ArtifactStreaming，host/gui-server/src/session.rs 对 ArtifactRead 固定返回 unsupported。不重复登记。

## Findings

### S12-CR07-01 生产 gui serve 握手无认证，本机 socket/pipe 可被任意本地进程接管

> **交叉复核裁定**（2026-08-18 主代理回写，GLM 复核，详见 [CR-07-cross-review-glm.md](CR-07-cross-review-glm.md)）：**uphold High**，一处证据修正——umask 022 下 socket 为 0755，connect 需写权限，其他用户通常被拒，跨用户面被高估；同用户任意进程可驱动 Run/PTY/审批已足够支撑 High。

- 类别：Security
- 严重度：High　置信度：Confirmed
- 证据：
  - host/cli/src/gui.rs:39-47：生产装配 HandshakeService::new(...)，capabilities 含 Events/Snapshots/ArtifactStreaming/TerminalStreaming/Approvals；没有 with_authenticator。
  - foundation/protocol/src/handshake.rs:90-106,144-155：authenticator 默认 None；仅当注入钩子时才要求 ClientAuthentication。无钩子时缺 proof 的握手直接 Accepted。
  - foundation/protocol/src/client_auth.rs:23,169-205：TokenAuthenticator / TOKEN_SCHEME = "pawork-token" 已实现且 constant-time 比较，但生产 serve 未接线。
  - host/transport/src/local_unix.rs:46-52：UnixListener::bind 之后无 chmod / owner-only 收紧；权限= 0777 & !umask（常见 umask 022 → 他人可 connect）。
  - host/transport/src/local_windows.rs:101-112：ServerOptions::new().create 未设 DACL；默认 Named Pipe 可被本机其他用户打开。
  - host/cli/src/gui.rs:29-31 与 host/app/src/data_dir.rs:10-26：socket 落在 ~/.pawork/pawork-gui.sock（或 PAWORK_DATA_DIR），create_dir_all 不设 0700。
  - host/gui-server/src/session.rs:348-361,376-396：握手后 host_stamp_command 覆盖客户端自报 identity 为 LocalUser{actor_id=client-N}，然后把 WorkspaceAdd / RunStart / TerminalWrite / ToolApprove / SessionFork 直接交给 GuiHostAdapter。
  - host/cli/src/service.rs:52-63,67-90：service install --apply 把同一条无认证 gui serve 写成 launchd/systemd 常驻。
  - 实际行为：本机任意能连上 UDS/pipe 的进程（含其他本地用户，若目录可遍历）可完成握手并驱动 Core：改 workspace、开 run、写 PTY、代为审批。
  - 期望行为：本机 serve 至少 owner-only 套接字 + 强制 token（已有 TokenStore/TokenAuthenticator）；无 authenticator 时 fail-closed，而不是默许匿名。
  - 影响面：pawork gui serve / pawork service 的全部 GUI、pawork watch、Desktop、protocol-probe。Secret 不经协议明文回传，但攻击者可触发已登录 Provider 的 Run，或经 PTY 读 ~/.pawork/auth.json（与 S12-CR03-02 叠加）。
- 验证建议（S12 内不执行）：在另一用户或 socat 连默认 socket，不带 authentication 发 Handshake，断言应 Rejected；再确认 socket mode 为 0600、目录 0700。Windows 用非属主账户开 named pipe。
- 整改边界：最小写入集 = host/cli/src/gui.rs（生成/加载 token 并 with_authenticator）+ host/transport/src/local_unix.rs / local_windows.rs（owner-only）+ Desktop/pawork-client 读同一 token。不可顺带改 remote TLS 语义或打开 K-08 ArtifactStreaming。apps/protocol-probe/src/main.rs:105 现写 scheme: "token"，接线后必须改成 pawork-token，否则探针会假失败。

### S12-CR07-02 连接队列满 / broadcast Lagged 只丢新事件并打内部标记，协议层不通知客户端重建

- 类别：Bug
- 严重度：High　置信度：Confirmed
- 证据：
  - host/gui-server/src/connection.rs:8-10,287-310：每连接有界队列（默认 1024）try_send 满则 lagged=true 并丢弃该新事件，返回 ManagerError::Lagged。
  - host/gui-server/src/session.rs:568-585：forwarder 对 Lagged 只 continue；对 RecvError::Lagged 只 mark_lagged。两条路径都不向客户端发 ServerFrame::Error(ReplayUnavailable) 或 ResumeDisposition::SnapshotRequired。
  - host/gui-server/src/session.rs:619-621：manager_error_frame 虽把 Lagged 映射为 ProtocolErrorCode::ReplayUnavailable，但热路径从未调用它（只用于 subscribe/unsubscribe 失败）。
  - host/app/src/hub.rs:83-96,167-176：Hub 用 broadcast 有界扇出；慢订阅者 Lagged 后只能靠 ring replay。GUI 层丢掉实时事件后没有强制客户端去 replay。
  - docs/gui-design.md §4.1 第 4 点：落后窗口应走 Replay，补不齐则 SnapshotRequired 并替换基线。
  - 实际行为：慢 GUI 会永久缺事件（含 Run 终态、审批、PTY 输出），客户端投影继续当自己是最新；session.lagged 只留在服务端内存。
  - 期望行为：丢事件后立即 fail-closed 通知该连接（Error / SnapshotRequired），或断开让客户端按 §4.1 重建；禁止静默空洞。
  - 影响面：多窗口 Desktop、pawork watch、任何订阅后处理不过来的客户端。与 K-07（限流未接线）叠加时，突发增量更容易打满 1024。
- 验证建议（S12 内不执行）：把 queue_capacity 降到 2，快速 publish 超过容量，断言客户端收到 ReplayUnavailable 或被踢，且后续 Snapshot/Resume 能收敛；当前应看到服务端 lagged=true、客户端无帧。
- 整改边界：最小写入集 = host/gui-server/src/session.rs 的 forwarder + 一条协议帧（复用已有 ReplayUnavailable / SnapshotRequired）+ clients/gui-client 消费该帧。不要在此任务改 EventHub 容量算法，也不要顺手接线 rate_limit.rs（K-07 独立任务）。

### S12-CR07-03 IdempotencyStore 的 check/record 非原子，并发或超时重试会重复执行副作用

> **交叉复核裁定**（2026-08-18 主代理回写，GLM 复核，详见 [CR-07-cross-review-glm.md](CR-07-cross-review-glm.md)）：**adjust-severity → Medium**。缺陷属实，但触发面被高估：GUI 单连接串行、多连接按 {client_id}/{command_id} 隔离、headless 串行、各通道各持内存表；现实触发仅剩 ACP 同 JSON-RPC id 并发重试一条窄路径，双执行后果成立但可达性窄。

- 类别：Bug
- 严重度：High　置信度：Confirmed
- 证据：
  - host/app/src/idempotency.rs:1-6,85-150：文档承诺「相同标识绝不重复执行」；实现是 check 与 record 两段锁，中间无占位。check 未命中即 New，不预留 key。
  - host/app/src/gui_host.rs:892-918：command 先 check，再 await dispatch_command（SessionCreate / RunStart 等有副作用），成功后再 record；record 的 DuplicateCommand / KeyConflict 被 let _ = 吞掉。
  - host/app/src/gui_host.rs:1539-1556：GUI 来源用 {client_id}/{command_id} 隔离；Automation（headless / ACP，host/cli/src/adapter.rs:28-33,104）不加连接前缀，只靠调用方 command_id。ACP 为 acp-{request_id}（host/channels/src/acp/adapter.rs:293）。
  - host/app/src/gui_host.rs:893 与 foundation/protocol/src/app/version.rs:87：全部命令落在冻结 tenant local/default，跨通道共享同一张内存表。
  - 存储纯内存、无 inflight 态；进程重启后同一 key 会再执行一遍。
  - 实际行为：同一 key 在首次 record 前再次进入（客户端超时重试、ACP inflight 并发、GUI 多连接竞态）时，两次 check 都是 New，SessionCreate/RunStart/ToolApprove 会执行两次；后到的 record 失败被忽略，调用方各自拿到「成功」响应。
  - 期望行为：check 必须 CAS 占位（inflight / completed）；inflight 等待或返回冲突；错误仍可不缓存。Automation 通道也要按连接/会话隔离。
  - 影响面：所有走 GuiHostAdapter::command 的入口（GUI / headless / ACP）。GUI 单连接收包循环是串行的，因此最可能的触发是超时重试与 ACP 并发，而不是同连接流水线。
- 验证建议（S12 内不执行）：两个任务并行对同一 command_id 调 SessionCreate；或令第一次 create_session 阻塞，第二次在 record 前到达。预期只创建一条 session，当前会创建两条。
- 整改边界：最小写入集 = host/app/src/idempotency.rs + gui_host.rs 的 command 占位/等待；可选给 Automation 补连接前缀。不要改协议信封形状，不要把表持久化进 SQLite（那是独立任务）。

### S12-CR07-04 服务端 SnapshotRequired 会附带 Snapshot，客户端却按「只回 disposition」提前结束

- 类别：Bug
- 严重度：Medium　置信度：Confirmed
- 证据：
  - host/gui-server/src/session.rs:522-548：Replay 失败或 SnapshotRequired 时，在 Resume 帧之后 host.snapshot() 再推一帧 ServerFrame::Snapshot。
  - clients/gui-client/src/lib.rs:600-608：收到 SnapshotRequired 立即 break，注释写「V2 gui-server 对 SnapshotRequired 只回 disposition，不自动补发 Snapshot」——与服务端实现相反。
  - 随后的 Snapshot 留在连接上：next_event 会 stash，之后的 snapshot() 可能把这帧当成新请求的响应（clients/gui-client/src/lib.rs:527-543,550+）。
  - apps/desktop/src/controller.rs:106-124：重连基线用的是握手后的 initial_snapshot()，不是 ResumeOutcome.snapshot；SnapshotRequired 分支只 Ack 握手快照。
  - host/gui-server/src/lib.rs:152-157 与 session.rs:300-313：每个 accept 分配新 client-N；last_ack 存在该 id 的内存会话上，重连握手读到的是 0，不能代替客户端显式 Resume。
  - docs/gui-design.md §4.1：SnapshotRequired 必须以新 Snapshot 替换 stale 基线。
  - 实际行为：契约在服务端是「Resume + Snapshot」，在客户端是「Resume only」。Desktop 仍能靠握手快照活下来，但会丢掉 Resume 附带的更新快照，并让 inbox 多一帧 Snapshot。
  - 期望行为：客户端与 gui-design 一致：消费服务端附带 Snapshot，或服务端停止附带并让客户端显式 SnapshotRequest（二选一，不能各写各的）。
  - 影响面：所有 GuiClient::resume / connect_with_resume 调用方（Desktop、protocol-probe、pawork watch 不走 resume）。host/gui-server/tests 按「服务端会附带 Snapshot」写，clients/gui-client/tests/contract.rs:357-365 则允许 SnapshotRequired 后客户端再自己拉 Snapshot，两边测试都绿但语义分裂。
- 验证建议（S12 内不执行）：Resume 一个超出窗口的 last_global_sequence，断言客户端 ResumeOutcome.snapshot.is_some() 且等于服务端第二帧；当前为 None，连接上仍有未读 Snapshot。
- 整改边界：最小写入集二选一：改 clients/gui-client/src/lib.rs 收齐附带 Snapshot，或改 host/gui-server/src/session.rs 不再附带并更新测试注释。不要同时改 Timeline 投影（CR-05）或 Desktop 视觉。

### S12-CR07-05 Headless 能力门控把未映射命令当成「已授权」

- 类别：Security / Requirement Gap
- 严重度：Medium　置信度：Confirmed
- 证据：
  - host/cli/src/headless.rs:318-331：command_capability 只覆盖 Session* 与 Run*/ToolApprove；WorkspaceAdd / WorkspaceTrust / AuthStart / GitStage / Terminal* / CoreInitialize 返回 None。
  - host/cli/src/headless.rs:67-86,135-157：gate(None) 在 granted 非空时放行。因此只要握手拿到任意一项（例如只有 CompatHistory），即可发 WorkspaceAdd 或 TerminalCreate。
  - clients/sdk/src/transport.rs:73-80 默认向 Host 申请 Sessions/Runs/Streaming/CompatImport/CompatHistory；Host 按交集团授予（headless.rs:28-34,106-112）。
  - host/cli/src/headless.rs:143-153：仅 SessionClientContextReplace 检查 owned_sessions；RunStart / ToolApprove / TerminalWrite 不限制目标 session/terminal 是否由本连接打开。
  - Headless 是父进程 stdio，默认攻击面小于 UDS；但 SDK 可被其它程序 spawn pawork headless --json-stdio 复用同一用户的 AppCore/凭证。
  - 实际行为：能力表不是授权表。未列名的高权限命令走默认允许。
  - 期望行为：未映射命令 fail-closed（UnsupportedCapability），或显式增补 Workspace/Terminal/Auth 能力并按连接做对象级 ACL。
  - 影响面：clients/sdk 与任何 spawn headless 的集成。ACP 另有 method 白名单，不在本条。
- 验证建议（S12 内不执行）：Hello 只申请 CompatHistory，再发 WorkspaceAdd；当前应成功，期望 UnsupportedCapability。
- 整改边界：最小写入集 = host/cli/src/headless.rs 的 command_capability / gate。不要顺手改 GUI 握手 capabilities（K-08）或 ACP method 表。

### S12-CR07-06 协议探针默认认证 scheme 与生产 token 常量不一致（潜伏）

- 类别：Maintainability
- 严重度：Low　置信度：Confirmed
- 证据：
  - apps/protocol-probe/src/main.rs:100-107：--token 发出 scheme: "token"。
  - foundation/protocol/src/client_auth.rs:23,182-186：TokenAuthenticator 只接受 "pawork-token"，其它 scheme 一律 authentication_failed。
  - golden foundation/protocol/tests/golden/client_handshake.json 使用 "bearer"，只锁定 serde 形状，不是生产 scheme。
  - 实际行为：今日生产无 authenticator，探针不带 token 也能握手。一旦 S12-CR07-01 接线，--token 探针会稳定被拒，造成「认证已接但探针全红」的假失败。
  - 期望行为：单一 scheme 常量，探针/Desktop/client 共用 TOKEN_SCHEME。
  - 影响面：仅 protocol-probe --connect --token；不单独构成现网漏洞。
- 验证建议：与 CR07-01 同批：接线 authenticator 后用错误 scheme / 正确 scheme 各握手一次。
- 整改边界：apps/protocol-probe/src/main.rs 改用 pawork_protocol::client_auth::TOKEN_SCHEME；不要改 golden 里的历史 scheme 除非连 typegen 一起更新。

## 统计

| 严重度 | 条数 | Confirmed | Needs Verification |
| --- | ---: | ---: | ---: |
| Critical | 0 | 0 | 0 |
| High | 3 | 3 | 0 |
| Medium | 2 | 2 | 0 |
| Low | 1 | 1 | 0 |
| 合计 | 6 | 6 | 0 |

类别：Security 2（CR07-01、CR07-05）、Bug 3（CR07-02、CR07-03、CR07-04）、Maintainability 1（CR07-06）。

未把 K-07 / K-08 / S12-CR03-02 / S12-CR05-01 计为新 finding。本包无 Needs Verification 条目。
