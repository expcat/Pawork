# pawork-client

> GUI Connection Protocol 的 typed 连接面（`GuiClient`）+ headless SDK（`headless::PaworkClient`，R1 由原 sdk crate 并入）。生产依赖仅 `pawork-domain` / `pawork-protocol` / `pawork-transport`；是外部进程（Desktop、编程驱动）唯一允许的业务 crate。

## 1. 职责与边界

- 两个连接面：
  1. `GuiClient`（crate 根）——经 `pawork-transport` 的字节帧连接 CLI 的 GUI Connection Protocol：握手、Command / Query、订阅、Snapshot / Resume、Ack / Heartbeat。
  2. `headless::PaworkClient`——经 stdio JSONL 驱动 `pawork headless --json-stdio` 宿主的 Rust Agent SDK：typed 请求 / 事件流 / 背压 / compat 导入。
- 不嵌入 Core、不实例化 Provider、不加载 GUI framework、不做业务决策；纯 client。
- 作为 Desktop 的唯一业务依赖面，re-export 下游所需的 protocol 帧类型与 `LocalTransport`，Desktop 不得再直接依赖 protocol / app / engine。
- `pawork-app` 只是 dev-dep（供 probe / contract 进程内装配），生产构建不含。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~1 280 | `GuiClient` 全部实现：`ClientConfig`、`SessionInfo`、`ResumeOutcome`、`ClientError` / `ClientErrorKind`、私有 `FrameWant` 帧路由、握手 / 往返 / 订阅屏障 / Snapshot / Resume / Ack / Heartbeat；每连接实例 request namespace 防 Host 重启后的幂等键碰撞；对 Desktop 的 re-export（protocol 类型含 Settings 载荷、`projection`、`TOKEN_SCHEME`、transport 四类型）；10 个内联测试 |
| `src/headless/mod.rs` | ~90 | headless SDK 门面：模块文档（版本策略 / 稳定面 / 背压）、re-export（`PaworkClient`、`SdkError`、`EventSubscription`、`SDK_API_VERSION` 等）、`spawn_pawork` 便捷入口、`experimental`（`CompatOutcome`）与 `reexport`（常用协议类型）子模块 |
| `src/headless/client.rs` | ~770 | `PaworkClient`：`spawn` / `from_transport`（自动握手）、typed 高层 API（`create_session` / `run_start` / `cancel` / `run_retry` / `list_workspaces` / `subscribe` / `unsubscribe` / `resume` / `import_compat` / `compat_history` / `close`）、`RouterState`（pending 请求 + 订阅槽）、`reader_loop`（逐行解析 JSONL 并路由） |
| `src/headless/transport.rs` | ~180 | `Transport` trait（行级 send / recv / close）与 `StdioTransport`（进程 spawn + stdin/stdout 管道）；`PaworkOptions`（binary 默认 `PAWORK_BIN` 或 `pawork`、args 默认 `["headless", "--json-stdio"]`、env、timeout） |
| `src/headless/stream.rs` | ~110 | `EventSubscription`（有界 `mpsc` 事件通道，可取消）与 `BackpressurePolicy`（Drop 计数丢弃 / Error 显式溢出） |
| `src/headless/error.rs` | ~150 | `SdkError` / `SdkErrorKind`（spawn、I/O、malformed frame、`UnknownResponseType`、`UnsupportedCapability`、`IncompatibleApiVersion`、`RequestFailed`、`Backpressure`、`Cancelled`、`Timeout`；`as_str` 稳定标签） |
| `src/headless/mock.rs` | ~150 | `MockTransport`：脚本化响应队列 + 已发送行记录（`Clone` 共享），供下游无进程测试 |
| `src/headless/version.rs` | ~40 | `SDK_VERSION`（crate 版本）与 `SDK_API_VERSION`（跟随 protocol 当前版本，现为 1.9）；2 个内联测试 |
| `examples/probe.rs` | ~580 | live 模式测试客户端：`--connect`（外部握手 + WorkspaceList）、`--live-two-gui`、`--live-pty`、`--token`（缺省读 `{data_dir}/gui.token`） |
| `tests/contract.rs` | ~650 | GUI Connection Protocol 契约测试（LocalTransport UDS × 进程内 `GuiServer` + `GuiHostAdapter` + `MockProvider`），9 测试 |
| `tests/probe.rs` + `tests/probe/harness.rs` + `tests/probe/scenarios.rs` | ~1 110 | `--self-test` 13 场景（MemoryTransport 进程内装配）；harness 提供 AppCore / GuiServer / 握手 / CLI 侧命令辅助；默认不编译，`probe-self-test` feature 显式启用 |
| `tests/client_tests.rs` | ~590 | headless SDK 契约测试（MockTransport + `tests/fixtures/` 5 个 JSON/JSONL fixture），22 测试 |
| `tests/spawn_e2e.rs` | ~350 | 真实进程 e2e：spawn 工作区 `pawork` 二进制（无 `headless` 子命令时 SKIP，不作门禁），3 测试；默认不编译，`spawn-e2e` feature 显式启用 |

## 3. 对外 API 面

**GuiClient 连接与会话信息**

- `connect(transport, endpoint, options, authentication)` / `connect_with_config(..., ClientConfig)`：连接 + 握手；获授 `Snapshots` 能力时同步消费首帧 Snapshot（`initial_snapshot()` 取出）。Accepted 握手的可选 `host_data_dir` 原样保留在 `SessionInfo`，不做路径推断或规范化。`connect_with_resume(...)` / `connect_with_resume_config(...)`：握手后按 `last_global_sequence` 自动 Resume，返回 `(client, Option<ResumeOutcome>)`。
- `ClientConfig { timeout（默认 10 s）, client_name, client_version, capabilities（默认 Events + Snapshots + Approvals）, supported_api_versions（默认 SUPPORTED_API_VERSIONS）}`。
- `SessionInfo { handle, client_id, connection_id, capabilities（授予集）, resume（服务端初始 disposition）, host_data_dir? }`：握手成功后固定不变；`host_data_dir` 缺字段解码为 `None`，存在时原样透传给 Desktop。
- `ResumeOutcome { disposition, replayed（Replay 补发的事件，序列严格递增）, snapshot（SnapshotRequired 附带的重建快照）}`。
- 会话信息访问：`info()` / `handle()` / `client_id()` / `connection_id()` / `api_version()` / `capabilities()` / `connection_info()` / `is_connected()` / `last_acked_sequence()`。

**GuiClient 往返与事件**

- `command(AppCommand, CommandSource, ActorIdentity)` / `query(AppQuery, ...)`：自动装配信封；自动 id 由握手 `client_id` + 每连接实例 request namespace + 自增序号组成，Host 进程重启后也不会与持久化幂等账本中的旧请求碰撞。`command_envelope` / `query_envelope` 仍接受调用方完整信封（显式幂等重放场景）。返回 `AppResponseEnvelope`。
- `subscribe(subscription_id, streams)`（空 `streams` = 全量）/ `subscribe_all()` / `unsubscribe(id)`：协议无订阅专用 Ack，实现用「Heartbeat 屏障」——控制帧后紧发 `Heartbeat{nonce}`，Pong 前出现的 request-scoped Error 即订阅失败（如 `PermissionDenied`），不误报成功、不污染后续 Heartbeat。
- `next_event()` / `next_event_timeout(duration)`：读取下一条 `AppEventEnvelope`；`request_id = None` 的连接级 Error 帧（如 `ReplayUnavailable`）在此路径显式抛出。
- `snapshot()`、`resume(last_global_sequence) -> ResumeOutcome { disposition, replayed, snapshot }`、`ack(global_sequence)`、`heartbeat()` / `heartbeat_with_nonce(nonce) -> nonce`、`close()` / `disconnect()`（幂等）。

**GuiClient 错误**

- `ClientError`：`Transport` / `Codec` / `HandshakeRejected` / `Protocol` / `Version` / `Timeout` / `Disconnected` / `UnexpectedFrame` / `Internal`；`kind()` 返回 `ClientErrorKind`。辅助判定：`is_retryable`（Transport/Protocol 按 retryable 位，Timeout/Disconnected 恒可重试）、`is_auth_failure`、`is_incompatible_version`（握手拒绝或后续信封版本漂移，ADR-036）、`is_request_not_found`、`is_replay_unavailable`。错误结构化，不携带原始帧字节。

**headless SDK（`pub mod headless`）**

- `PaworkClient::spawn(PaworkOptions)`：spawn `pawork headless --json-stdio` 并完成 hello/ack 握手（`SDK_API_VERSION` 协商，major 不兼容显式失败）；`from_transport(Box<dyn Transport>, options)` 注入自定义 transport（如 `MockTransport`）。
- 高层 API：`create_session(workspace_id, title)`、`run_start(session_id, message, profile)`、`cancel(run_id)`、`run_retry`、`list_workspaces()`、`resume`、`import_compat` / `compat_history`（compat 导入与历史）、`close()`（取消 in-flight 与后续请求）；握手元信息 `api_version()` / `instance_id()` / `capabilities()`；raw 逃生口 `query_envelope` 直返 `AppResponse`。
- `subscribe(streams, buffer, BackpressurePolicy) -> EventSubscription`：有界事件通道；`Drop` 策略静默丢弃并计数（`dropped_count`），`Error` 策略在溢出时向消费者返回 `SdkErrorKind::Backpressure`。
- 未知 / 不支持情况全部落显式错误类别（`UnknownResponseType` / `UnsupportedCapability` / `IncompatibleApiVersion`），不静默忽略。稳定面 = `PaworkClient` / `PaworkOptions` / `EventSubscription` / `SdkError` / `Transport` / `MockTransport`；`experimental::CompatOutcome` 可能不发 major 调整。

**re-export（Desktop 依赖面）**

- protocol：`ActorIdentity`、`ApiVersion`、`AppCommand/Query/Event/Response` 及信封、`ClientAuthentication`、`TOKEN_SCHEME`、`CommandSource`、`EventStream`、`GlobalSequence`、`GuiCapability`、`ProtocolErrorCode`、`RunState`、`Snapshot`、`TerminalExitReason`、`TimelineItem/Page`、`ResumeDisposition`、`projection` 模块。
- transport：`ConnectOptions`、`GuiTransportClient`、`LocalTransport`、`TransportEndpoint`。

## 4. 核心行为与数据流

**一次 framed 连接全流程（GuiClient）**

1. **connect**：`GuiTransportClient::connect(endpoint, options)` 建立字节帧连接（`max_frame_bytes` 通常 1 MiB；local 端点为 UDS 路径 / pipe 名）。
2. **handshake**：发送 `ClientFrame::Handshake`（`request_id = "handshake"`，携带 client 名称 / 版本 / `supported_api_versions` / 请求的 capabilities / 可选 `ClientAuthentication` token）；服务端 `Accepted` 返回 `handle`（协商版本）、`client_id` / `connection_id`、**按服务端能力筛选后授予**的 capabilities、按重连历史计算的初始 `ResumeDisposition`（首连通常 `SnapshotRequired`）；`Rejected` → `ClientError::HandshakeRejected`（如 `IncompatibleVersion` / `AuthenticationFailed`）。连接级 Error 帧在握手阶段映射 `HandshakeRejected`，在首帧 Snapshot 阶段映射 `Protocol`，不得落成 `UnexpectedFrame`。获授 `Snapshots` 时再同步读一帧首个 Snapshot；未获授则不阻塞等待。transport `receive` 取消安全（半帧进度留在连接内），超时可同连接重试；对端关闭映射 `Disconnected`。
3. **subscribe**：`subscribe_all()` 走 Heartbeat 屏障确认订阅生效（见 §3）。
4. **事件泵与并发路由（FrameWant）**：连接实例先生成仅用于自动请求 id 的 namespace（process id + 时间戳 + 进程内序号）；所有读操作经 `recv_matching(timeout, want)`——先查共享 `inbox`（VecDeque 缓存），再持 `io` 互斥锁读传输层；读到不匹配的帧 stash 回 inbox 而不是丢弃。匹配规则：`Response` / `Snapshot` / `Resume` 按 `request_id` 严格匹配（含 request-scoped Error）；`Event` want 接受 Event 帧与 **`request_id = None` 的连接级 Error 帧**；`Pong(nonce)` 按 nonce。事件泵与并发 command/snapshot 因此互不拆包（有内联回归钉住）。
5. **消费事件**：`next_event*` 循环取 Event；收到连接级 Error（如 lag 后的 `ReplayUnavailable`）时显式报错，调用方决定 Resume 或重建。
6. **ack**：消费端定期 `ack(global_sequence)` 声明消费进度（服务端据此裁剪重放窗口），`last_acked_sequence()` 可查。
7. **断线与 Resume**：连接断开后新建客户端（或 `connect_with_resume`），带上次 `global_sequence` 调 `resume`：服务端返回 `Replay`（补发缺失事件，`global_sequence` 严格递增，收在 `ResumeOutcome::replayed`）、`SnapshotRequired`（重放不可用，附重建 Snapshot）或 `UpToDate`。GUI 断线**不会**取消进行中的 Run（宿主侧语义，contract / probe 双覆盖）。

**headless SDK 数据流**

1. `spawn(PaworkOptions)` 启动宿主进程（默认 args `["headless", "--json-stdio"]`，binary 取 `PAWORK_BIN` 环境变量或 `pawork`，可注入 env / timeout）→ `StdioTransport` 经 stdin/stdout 以 JSONL 行通信。
2. hello/ack 握手：SDK 发 hello（声明 `SDK_API_VERSION` 与请求的 `SdkCapability`），宿主回 ack（实际 `api_version` / `instance_id` / 授予 capabilities）；major 不兼容 → `IncompatibleApiVersion`，未知响应类型 → `UnknownResponseType`，均显式失败。
3. typed API 调用 → 组装带自增 id 的 command / query 帧写入 stdin → `RouterState` 登记 pending → `reader_loop` 后台任务逐行解析 stdout：带 id 的 response 回填对应 pending；event 帧派发到匹配订阅槽的有界通道；**无 id 的 error 帧不误路由到唯一 pending 请求**（连接级信号）。
4. 事件消费：`EventSubscription` 从有界通道拉取；通道满时按 `BackpressurePolicy`——`Drop` 丢最旧并累加 `dropped_count`，`Error` 让消费者收到 `SdkErrorKind::Backpressure`。
5. 宿主返回的业务错误（`AppResponse::Error`）映射为 `SdkErrorKind::RequestFailed`（保留错误码与消息）；请求超时 / 进程退出 / `close()` 时，所有 in-flight 与后续请求以 `Timeout` / `Cancelled` 显式失败，`close` 同时回收子进程。

## 5. 契约与不变量

- **版本协商**：`ClientConfig::supported_api_versions` 默认跟随 `pawork-protocol::SUPPORTED_API_VERSIONS`（1.0 / 1.1 / 1.2 / 1.3 / 1.4 / 1.5 / 1.6 / 1.7 / 1.8 / 1.9 / 1.10），服务端取 major 相同的最高共同 minor；不兼容必须显式拒绝（`IncompatibleVersion`），后续 ServerFrame 信封版本漂移由 `ClientError::Version` 捕获（ADR-036）。headless 侧 `SDK_API_VERSION` 跟随 `pawork_protocol::API_VERSION`（当前 1.10）同理。
- **帧上限**：经 `ConnectOptions::max_frame_bytes` 与 transport 对齐 1 MiB（见 [transport.md](transport.md)）；本 crate 不改帧格式。
- **FrameWant 路由不变量**：Response / Snapshot / Resume 只按 `request_id` 匹配；Event 消费路径独占 `request_id = None` 的错误帧；不匹配帧只 stash 不丢弃——并发调用互不吞帧。
- **幂等重放**：同 `command_id` 的 `command_envelope` 重放由宿主 IdempotencyStore 返回相同响应（probe `command-idempotency` 钉住）。
- **自动请求 id 隔离**：`command` / `query` 的自动 id 在不同 `GuiClient` 连接实例间不复用；即使 Host 重启后 `client_id` 重新从 `client-0` 计数，也不能命中旧进程留下的持久化幂等记录。调用方显式传入 `command_envelope` 的 id 不改写。
- **Snapshot 能力约定**：未获授 `Snapshots` 时服务端不得发送首帧 Snapshot、客户端不得等待（contract 测试钉住）。
- **安全**：headless spawn 只经 `PaworkOptions`（binary / args / env）配置；GUI 侧认证用 `TOKEN_SCHEME` token 文件；错误与日志不携带原始帧字节。
- **SDK 稳定面**：稳定面内行为改动需 minor 版本 + CHANGELOG；`SdkErrorKind::as_str` 标签冻结（如 `"backpressure"`），下游可安全按标签分支。

## 6. 依赖关系

- **生产依赖**：`pawork-domain`（[domain.md](domain.md)）、`pawork-protocol`（[protocol.md](protocol.md)）、`pawork-transport`（[transport.md](transport.md)）。`default = []`，无 feature。
- **dev-dep**：`pawork-app`（[app.md](app.md)，进程内 GuiServer / GuiHostAdapter）、`pawork-storage`（SessionStore）、`pawork-testkit`（MockProvider / MockScript）、transport 的 `memory` feature。
- **下游**：`pawork-cli`；`apps/desktop`（**唯一**业务依赖，所需类型全部从本包 re-export）。

## 7. 测试与验证资产

**`tests/probe.rs`（`--self-test` 13 场景，MemoryTransport 进程内装配；`probe-self-test` feature 显式启用）**

- `session-events`：握手消费首帧 Snapshot → GUI 建会话 → RunStart → 收 `AssistantDelta` 流式增量与 `Completed`。
- `snapshot-reconnect`：长跑 Run 中拉 Snapshot 重建 `ActiveRuns` → ack → close 不取消 Run → 重连 Resume + heartbeat。
- `resume-snapshot-fallback`：resume 序列领先服务端 → 必须降级 `SnapshotRequired`。
- `three-gui-sync`：三 GUI 并发订阅，CLI 与 GUI A 发起的 Run 完成事件对所有客户端与 CLI 观察者可见。
- `command-idempotency`：同 `command_id` 信封重放返回逐字段相同的响应。
- `terminal-gate`：`TerminalCreate` 在 `AskForDangerous` + trusted 放行（携带 sandboxed / approval_mode / policy 元数据）；`ReadOnly` 档 fail-closed 拒绝。
- `artifact-chunks`：缺失 artifact fail-closed；`ArtifactChunk` 帧编解码往返一致。
- `version-reject`：只声明 2.0 的客户端握手被拒，错误码 `IncompatibleVersion`。
- `disconnect-keeps-run`：GUI close 后 Run 仍活跃，CLI 取消才消失。
- `quota-alert-roundtrip`：`QuotaAlert` 事件线上 JSON 携带 kind/source、`mask_credential_hint` 脱敏（不泄漏原文）、缺字段旧 JSON 解码为 `None`、`QuotaFailureView.adapter_kind` 序列化形态冻结。
- `diff-list-files` / `diff-get`：无会话时返回空 `files`（`diff-get` 另验 `complete = true`）。
- `mcp-list`：未装配 MCP 时 `servers` 为空数组。

**`tests/contract.rs`（LocalTransport UDS 真机装配，9 测试；socket 落 tempdir）**

- `create_session_send_message_and_receive_streaming_run_events`：Accepted 握手的 `host_data_dir` 原样进入 `SessionInfo`，随后建会话 / 发消息 / 收流式 Run 事件。
- `snapshot_and_reconnect_resume_replays_missing_events`：Snapshot + 断线重连，有 last_ack 且 host 能 replay 时 Resume 返回 Replay。
- `resume_falls_back_to_snapshot_required_when_replay_unavailable`：无共享 replay 源时降级 SnapshotRequired。
- `three_gui_clients_sync_runs_from_cli_and_each_other`：三 GUI 同步 CLI 与彼此的 Run 事件。
- `incompatible_version_handshake_is_rejected`：版本不兼容握手被拒。
- `gui_disconnect_does_not_cancel_run`：GUI 断线不取消 Run。
- `ack_and_heartbeat_round_trip`：Ack / Heartbeat 往返。
- `connect_without_snapshots_does_not_wait_for_initial_snapshot`：未获授 Snapshots 时不等待首帧。
- `subscribe_without_events_returns_permission_denied_and_does_not_poison_heartbeat`：未获授 Events 时订阅报 PermissionDenied 且不污染后续 Heartbeat。

（文件头注明两个 V2 用例不迁：同 command_id 重放在 pawork-app 单测覆盖；large artifact 分片读因 V2 无 artifact-store 停止宣告。）

**`tests/client_tests.rs`（headless SDK × MockTransport，22 测试）**：握手三态（版本 / instance / capabilities 暴露、不兼容版本显式失败、未知响应类型显式失败）；`create_session` / `query` / `cancel` 往返 framing；宿主业务错误映射 `RequestFailed`、error 帧携带显式 kind、**无 id error 帧不误路由到唯一 pending**；订阅只路由匹配 stream、背压 Drop 计数 / Error 溢出、退订移除槽位；fork + resume 生命周期；compat 导入与历史往返；close 取消 in-flight 与后续请求；`tests/fixtures/` 5 个固定协议样例（`hello_ack.json` / `session_response.json` / `error_frames.json` / `run_events.jsonl` / `compat_import_response.json`）端到端解码；raw query 信封直返 `AppResponse`；mock 按序记录发送行；`SdkErrorKind::as_str` 标签稳定。

**`tests/spawn_e2e.rs`（真实进程，3 测试；`PAWORK_BIN` 或 `target/debug/pawork`，无 `headless` 子命令时 SKIP 不作门禁；`spawn-e2e` feature 显式启用）**：spawn + 握手 + 已映射 Command/Query 往返 + 未映射命令 fail-closed + compat 经真实 SessionStore 持久化 + 关闭回收；无 provider 时 RunStart 返回错误响应；真实宿主强制执行已授予 capabilities。

**`examples/probe.rs`（live 模式，需真实 `pawork gui serve`）**：`--connect`（握手 + WorkspaceList）、`--live-two-gui`（双客户端、kill 一个后 Resume Replay）、`--live-pty`（开 PTY、写入、断线重连续接）；token 缺省读 `{data_dir}/gui.token`。

**`src/lib.rs` 内联（10）**：FrameWant 匹配矩阵（request-scoped vs 连接级 Error）、事件等待者与响应等待者互不饿死、`next_event` 显式暴露 `ReplayUnavailable`、连接实例 request namespace 不重复等。

默认验证命令：`cargo test -p pawork-client --offline --lib --tests`。

2026-09-03 SET-6g 后该默认命令 41/41 通过（lib target 10、client_tests 22、contract 9）；contract 主路径同时锁定 `host_data_dir` 原样透传。Host 重启后的真实 policy fail-closed 另由 Desktop U2 矩阵覆盖。

opt-in 复跑：`cargo test -p pawork-client --offline --features probe-self-test --test probe`；spawn_e2e 用 `--features spawn-e2e --test spawn_e2e`（2026-08-30 起默认死表不再编译这两箱）。

## 8. 注意事项与已知限制

- `snapshot-reconnect` 场景有既有偶发超时记录，见 history；改 probe 相关代码先复跑该场景。
- `spawn_e2e` 依赖已构建的 `pawork` 二进制，属可跳过 e2e，不纳入默认死表（`spawn-e2e` feature 门控编译）。
- `GuiClient` 无后台读任务：事件与响应都在调用方 await 中拉取；长时间不调 `next_event` 时事件会积压在服务端 / inbox。`GuiClient` 是 `Clone`（内部 Arc 共享），可以一个克隆跑事件泵、另一个发 command。
- headless SDK 相反有 `reader_loop` 后台任务；两个连接面的线程模型不同，集成时勿混淆。
- headless `experimental` 模块 API 可能不发 major 调整；生产集成应只用稳定面。
- Desktop 若需要新协议类型，应在本包补 re-export 而不是让 Desktop 直接依赖 `pawork-protocol`（架构红线，见 [../../design.md](../../design.md) §2）。
- 跨包全流程（GUI 连接 / 事件重放）见 [../flows.md](../flows.md) 与 [../../architecture.md](../../architecture.md)；产品能力汇总见 [../README.md](../README.md)。
