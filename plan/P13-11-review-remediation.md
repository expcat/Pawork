 # P13-11：Phase 13 评审修复（REVIEW remediation）
 
 > Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P13-1 ~ P13-10
 
 **最终目的**：按 [docs/review/p13-review.md](../docs/review/p13-review.md) §2/§3/§4 收敛 Phase 13「库代码完整、正式宿主二进制不接线」断层与一批预留死码——让 `pawork serve` 真正装配 `GuiServer`（实现首个生产 `GuiServerHost`，绑定本地端点、accept 连接、走真实帧循环），让协议版本校验从单向变双向（服务端出站接入 `validate_server_frame_api_version`、客户端入站接入 `decode_server_frame_checked`），删除一批全仓零调用方的死公开 API（aggregate `clear_run_approvals` / `note_gui_connect` / `note_gui_disconnect` / `gui_clients` 死字段、connection-manager `sessions()` / `is_subscribed()` / `expired_clients()`、client-auth `store()` / `scheme()`），并把 `TokenStore::generate` 从 `exists()`+`create` 改为原子 `create_new`（消除 TOCTOU）。`§4.1` transport-memory ↔ transport-remote-placeholder 去重经 DeepSeek worker 核验确认「需扩大 transport-memory 公开面（仅测试消费者）才能去重」，与「优先减少代码与概念」冲突，显式延后。无新增抽象，全部为「接线 / 删死码 / 小修」。
 
 **涉及范围**：`apps/pawork`（Cargo.toml、src/main.rs、src/lib.rs 新、src/gui_host.rs 新、tests/gui_serve.rs 新）、`app-service`（aggregate.rs、lib.rs、tests/run_lifecycle.rs）、`connection-manager`（src/lib.rs）、`client-auth`（src/lib.rs）、`gui-server`（src/session.rs）、`gui-client`（src/lib.rs）
 
 ## 处置策略（按评审 §6 矩阵）
 
 - **现在修复（落地）**：§2.1 宿主装配 GuiServer（W1）、§3.5 双向协议版本校验（W2）、§3.7 删 aggregate + connection-manager + client-auth 死公开 API（W3+W4）、§3.8 client-auth 原子创建（W4）、Commander 后处理 reviewer finding #1（`gui_clients` 死字段/访问器/快照字段清理）。
 - **显式延后（含后续任务）**：§2.2 宿主真实 Provider/ArtifactStore 注入、§2.3 idempotency_key/expected_revision 默认接线、§3.1 AppEvent 10/19 死变体、§3.2 router 命令面语义假实现、§3.4 慢客户端 Lagged 通知帧、§3.6 冻结枚举登记、§4.1 transport 去重（需扩 pub 面，与减概念冲突）、§4.6 cli-host 双入口收敛。
 
 ## 细分步骤（分组）
 
 ### A. §2.1 宿主装配 GuiServer（W1，apps/pawork）
 
 1. `apps/pawork/Cargo.toml` 加 `gui-server` / `transport-local` / `client-auth` / `transport-api` / `gui-protocol` / `agent-domain` / `core-api` / `tracing` 依赖 + dev-deps `gui-client` / `tempfile`；新增 `[lib]` 目标（`src/lib.rs` 暴露 `pub mod gui_host`，供集成测试构造 `ServeGuiHost`）。
 2. 新 `apps/pawork/src/gui_host.rs`：`ServeGuiHost` 实现 `cli_host::GuiServerHost`（全仓首个生产实现）。`start(instance)`：`block_in_place`+`block_on` 调 `server.bind(endpoint)`（Unix `<tempdir>/pawork-<instance>.sock` / Windows named pipe `pawork-<instance>`），`tokio::spawn` accept 循环（`GuiServerListener::accept` 内部已 spawn 会话任务，宿主只持 `SessionHandle` 到循环结束防 drop 断线）；`stop()` abort 任务 + close 监听器，清状态使端点可重绑。
 3. `apps/pawork/src/main.rs`：`build_gui_server` 装配 LocalTransport + 每实例 `gui.token`（首次 generate / 已存在 load，`PAWORK_DATA_DIR` 覆盖目录）+ `HandshakeService.with_authenticator(TokenAuthenticator)`；`host.attach_gui_server(Arc::new(server))`（替换原注释行）；装配失败降级为 warn + 仅等待信号。
 4. 新 `apps/pawork/tests/gui_serve.rs`：3 个集成测试——认证客户端连接+握手+查询往返；错误 token 握手被拒；stop-without-start 无操作 / restart-after-stop 证明清理。
 
 ### B. §3.5 双向协议版本校验（W2，gui-server + gui-client）
 
 5. `crates/gui-server/src/session.rs`：`send_frame` 加 `negotiated: Option<ApiVersion>` 参数，encode 前调 `validate_server_frame_api_version`（pre-negotiation 帧 `None` 跳过）；7 个调用点全部更新（握手后 `Some(negotiated)`，握手前 `None`）。2 个单测：版本匹配发送 / 过高 minor（1.1 vs 1.0）被拒且不发。
 6. `crates/gui-client/src/lib.rs`：两处入站 `decode_server_frame` → `decode_server_frame_checked`（`GuiClient::recv_frame` 用 `self.api_version()`，自由函数 `recv_frame` 加 `negotiated: Option` 参数）；新增 `ClientError::Version` / `ClientErrorKind::Version`（`is_incompatible_version()` 返回 true、非 retryable），其余 ProtocolError 映射 `ClientError::Protocol`。3 个单测：版本不匹配被拒 / 匹配通过 / pre-negotiation 跳过。
 
 ### C. §3.7 删 aggregate 死公开 API（W3，app-service）
 
 7. `crates/app-service/src/aggregate.rs`：删 `clear_run_approvals`（零调用方）、`note_gui_connect` / `note_gui_disconnect`（生产从不更新聚合，gui-server 用 connection-manager）；删 `tests/run_lifecycle.rs` 中对应测试块 + 失活 `GuiClientId`/`ConnectionId` import（RunCancel 幂等覆盖在 supervisor 单测仍存）。
 8. Commander 后处理（reviewer finding #1）：删 writer 后 `gui_clients` 字段 / `gui_clients()` 访问器 / `Snapshot.gui_clients` 字段 / `GuiClientRecord` 结构 + `lib.rs` re-export 全部失活（snapshot-service 从不读它，gui-protocol 公开 Snapshot 用通用 `SnapshotSectionKind` 无 GuiClients 变体，删它不碰冻结协议 schema），一并删 + 清失活 import。
 
 ### D. §3.7+§3.8 删 connection-manager 死公开 API + client-auth（W3+W4）
 
 9. `crates/connection-manager/src/lib.rs`：删 `sessions()` / `is_subscribed()` / `expired_clients()`（零生产调用方，仅自测）；裁剪仅测这些 API 的断言（保留 `should_forward` / `is_timed_out` / `heartbeat` 覆盖）。
 10. `crates/client-auth/src/lib.rs`：删 `TokenAuthenticator::store()` / `scheme()`（零调用方）；`TokenStore::generate` 改 `OpenOptions::new().create_new(true)`（消除 `exists()`+`create` TOCTOU，`AlreadyExists` 映射既有 `ClientAuthError::AlreadyExists`），新增 `generate_is_atomic_under_contention` 断言预存内容不被截断。
 
 ## 主要产出物
 
 - **接线**：`ServeGuiHost`（全仓首个 `GuiServerHost` 实现）+ `apps/pawork` 装配链（LocalTransport + TokenAuthenticator + HandshakeService + attach_gui_server）；`pawork serve` 从「只等信号」变为「绑定本地端点 + accept 连接 + 真实帧循环」。
 - **复活**：`validate_server_frame_api_version` / `decode_server_frame_checked`（两函数从零生产调用方变为双向校验主路径，ADR-036 双向校验落地）。
 - **删除**：aggregate `clear_run_approvals` / `note_gui_connect` / `note_gui_disconnect` / `gui_clients` 字段 / `gui_clients()` / `GuiClientRecord` / `Snapshot.gui_clients`；connection-manager `sessions()` / `is_subscribed()` / `expired_clients()`；client-auth `store()` / `scheme()`（共 11 项死公开 API + 1 死结构 + 2 死字段）。
 - **修复**：`TokenStore::generate` 原子创建（`create_new`，消除并发双进程截断窗口）。
 - **测试**：新增 9 个测试（gui_serve 3 + send_frame 版本校验 2 + client 版本校验 3 + client-auth 原子 1）。
 
 ## 验收标准（保留 REVIEW 追踪章节）
 
 - [x] **§2.1**：`pawork serve` 装配 GuiServer——绑定本地端点（Unix socket / Windows named pipe）、accept 连接、`GuiServerListener::accept` 内部 spawn 会话任务不二次派发、`stop` 中止任务+close 监听器；3 集成测试（认证连接+查询往返 / 错误 token 被拒 / restart 清理）通过
 - [x] **§2.1 装配健壮性（reviewer 复核）**：`GuiServerHost::start` 用 `block_in_place`+`block_on`（trait 同步 + 多线程 runtime）；装配失败降级 warn + 仅等信号；`cli-host` serve 在 `start` 失败时亦降级（lib.rs:240-243）
 - [x] **§3.5**：`validate_server_frame_api_version` 接入服务端 7 个出站点（pre-negotiation `None` 跳过），`decode_server_frame_checked` 接入客户端入站；新增 `ClientError::Version`；ADR-036 双向校验落地
 - [x] **§3.7**：aggregate 4 项 + connection-manager 3 项 + client-auth 2 项死公开 API 全删；reviewer finding #1（`gui_clients` 死字段）Commander 后处理一并清理（含 `GuiClientRecord` 结构 + `Snapshot.gui_clients` + re-export + 失活 import）
 - [x] **§3.8**：`TokenStore::generate` 原子 `create_new`，消除 TOCTOU；`generate_is_atomic_under_contention` 断言预存内容不被截断
 - [x] **定向验证**：workspace 全量 `cargo test --workspace --all-targets`（1155 passed / 0 failed，94 个 result 行）/ `cargo clippy`（app-service/cli-host/gui-server/core-runtime/snapshot-service/gui-client/client-auth/connection-manager 全 `-D warnings` 干净）/ `cargo fmt --all -- --check`（干净）/ `cargo run -p schema-typegen -- --check`（干净）—— Commander 独立复跑确认
 
 ### Deferred items（建议/跟踪，本任务不做）
 
 - **§2.2 宿主真实 Provider / ArtifactStore 注入**：出厂 `pawork run` 仍返回 Authentication 错误（无 provider），`artifact_read` 恒 Unavailable（store=None）。需在 core-runtime/cli-host 装配阶段补 config → provider-* → `register_provider` 路径与 ArtifactStore 创建注入。属 Provider v2（Phase 15）/ 账号控制面（Phase 18）配套接线。
 - **§2.3 idempotency_key / expected_revision 默认死字段**：cli-host / gui-client 仍硬编码 `idempotency_key: None` 且每次新生成 command_id；`expected_revision` 全仓恒 None。需客户端重试层复用信封或接线乐观并发。
 - **§3.1 AppEvent 10/19 死变体**：`CoreReady` / `WorkspaceChanged` / `SessionChanged` / `DiffChanged` / `TerminalOutput` / `AuthChanged` / `ProviderStatus` / `PluginError` / `GuiClientConnected` / `GuiClientDisconnected` 无生产者；cli-renderer 写了永不触达的 match 分支。需补命令事件产生路径或收敛 match + 标注 deferred。
 - **§3.2 router 命令面语义假**：RunTool/AuthStart/GitStage/Terminal* 等为「聚合记一笔」占位，`tools: Vec::new()` + `execute_tools` no-op。需按真实需求接线（RunTool→builtin-tools、Terminal→pty-service、GitStage→git-service）。
 - **§3.4 慢客户端 Lagged 静默**：`session.lagged` 生产无读取，无 Lagged/Resync 通知帧；forwarder broadcast Lagged 走 `continue` 静默丢事件。需补服务端→客户端通知帧（minor bump）或至少置标记让客户端主动 Resume。
 - **§3.6 冻结枚举未接线**：`ServerFrame::CommandAccepted` / `GuiCapability::TerminalStreaming` / `Approvals` / `ProtocolErrorCode::PermissionDenied` / `CommandSource::Plugin` / `Mcp`。属冻结协议内预留，按 ADR-036 登记为「已定义未接线」。
 - **§4.1 transport-memory ↔ transport-remote-placeholder 去重**：DeepSeek worker 核验确认两 crate 的 `MemoryConnection`（私有）/ `MockRemoteConnection` 是同构 mpsc-pair，但 transport-memory 的 pair 类型全为私有 / 无公开构造器，`MemoryTransport::connect` 硬编码 `ConnectionLocality::InProcess` + `memory-client-*` ID，remote 需 `Remote` + 自定义 ID。去重需扩大 transport-memory 公开面（其消费者全为测试侧），与「优先减少代码与概念」冲突；review §4.2 亦判「维持现状可接受」。延后至下次触碰 transport 层时评估（如真 remote transport P17-11 落地）。
 - **§4.6 cli-host 双入口 + Placeholder stub**：`ServiceOperation::dispatch` 第二入口 + 十命令族 Placeholder。需收敛为单一 router 路径 + stub 标注 deferred。
 
 ### Reviewer 提出但判定为可接受的低优先项（不另立任务）
 
 - **§2.1 accept 循环持有 SessionHandle 的 Vec 无界增长**（reviewer finding #2）：`ServeGuiHost` 把每个 accept 的连接句柄 push 到 `Vec` 直到 `stop()`，长 serve 累积每客户端一个句柄。这是有意设计（drop 句柄释放 close 通道致会话断线），进程生命周期内线性内存增长可接受；若后续支持长生命周期多客户端可评估 drop 已关闭句柄。
 - **§2.1 build_gui_server 无条件在所有模式跑**（reviewer finding #3）：`pawork run`/`shell` 首次运行也会 create `gui.token`，虽 server 只 serve 模式启动。副作用无害（仅一次小文件创建），收紧到仅 serve 模式收益有限。
 - **§3.5 pre-negotiation codec 错误分类 Codec→Protocol**（reviewer finding #4）：`decode_error` 把非版本类 ProtocolError 映射 `ClientError::Protocol`（旧 `decode_server_frame` 直映 `Codec`）。无测试/调用方依赖 Codec 分类，是刻意的错误收敛。
 - **§3.5 潜在自校验风险**（reviewer finding #5）：未来若 `SUPPORTED_API_VERSIONS` 加 1.1 而事件仍盖戳当前 `API_VERSION`，1.0 客户端会触发服务端 send_frame 自校验失败。当前 negotiated==API_VERSION==1.0 不触发；属未来 minor 演进时需同步盖戳策略的事项，本任务不处理。
 
 ## 验证记录（2026-08-11）
 
 - `cargo build --workspace --all-targets`：通过。
 - `cargo test --workspace --all-targets`：**1155 passed / 0 failed**（94 个 test result 行）。本次新增 9 个测试全过：gui_serve 3（认证连接+查询往返 / 错误 token 被拒 / stop no-op+restart 清理）、send_frame 版本校验 2（匹配发送 / 过高 minor 拒绝）、client 版本校验 3（不匹配 Version / 匹配通过 / pre-neg 跳过）、client-auth 原子 1（预存内容不截断）；connection-manager 裁 3 处断言后 `subscribe_unsubscribe_and_stream_filterting` / `heartbeat_refreshes_and_timeout_is_detected` 仍覆盖 `should_forward` / `is_timed_out`。
 - `cargo clippy`（app-service / connection-manager / client-auth / gui-server / gui-client / cli-host / core-runtime / snapshot-service，`--all-targets -- -D warnings`）：通过。
 - `cargo fmt --all -- --check`：通过。
 - `cargo run -p schema-typegen -- --check`：TypeScript declarations up to date（W2 新增 `ClientError::Version` 不在 schema 根——协议帧才导出——schema 不变）。
 - 残留核验（Commander）：`rg "GuiClientRecord|\.gui_clients\(\)|note_gui_connect|note_gui_disconnect|clear_run_approvals"` 在 crates/ apps/ 零生产命中；`rg "validate_server_frame_api_version|decode_server_frame_checked"` 现有生产调用点（gui-server session.rs send_frame + gui-client lib.rs 两处 recv）；`rg "TokenAuthenticator.*(store|scheme)\(\)"` 零命中；`cargo check -p protocol-test-gui`（gui-client API 唯一非门禁消费者）通过。
 - 写集合核验（Commander）：仅 `apps/pawork/{Cargo.toml,src/main.rs,src/lib.rs(新),src/gui_host.rs(新),tests/gui_serve.rs(新)}` + `crates/app-service/{src/aggregate.rs,src/lib.rs,tests/run_lifecycle.rs}` + `crates/connection-manager/src/lib.rs` + `crates/client-auth/src/lib.rs` + `crates/gui-server/src/session.rs` + `crates/gui-client/src/lib.rs` + `Cargo.lock`（自动同步）。diff stat：9 文件 modified + 3 文件 new + Cargo.lock，未触碰其它源码。
 - 按本任务门禁节奏执行 workspace 全量定向门禁（修复面跨 6 crate + 1 app，需 workspace 级确认）；三平台与发布门禁留待 Core 主干 L2/L3。
 
 **相关文档**：[REVIEW.md](../REVIEW.md) §P13 · [docs/review/p13-review.md](../docs/review/p13-review.md) · [cli-host](../docs/features/cli-host.md) · [gui-connection](../docs/features/gui-connection.md) · [ADR-022 GUI 经 CLI 连接](../docs/adr/ADR-022-gui-connects-via-cli.md) · [ADR-036 协议版本化](../docs/adr/ADR-036-gui-protocol-versioning.md) · [ROADMAP Phase 13](../ROADMAP.md)
