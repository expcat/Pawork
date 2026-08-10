# Phase 13 Review：CLI Host 与多 GUI 协议

- **审查范围**：P13-1～P13-10（app-service 完整化 + 统一 Command Source、CLI Host 装配与运行模式、GUI Connection Protocol、GUI Server 与 Local Transport、多 GUI 运行时、Remote Transport 占位、TS 类型生成、大型 payload Artifact API、GUI Client 与 Contract Tests、Protocol schema 版本化）。涉及 16 个 crate：`app-service` / `core-runtime` / `cli-host` / `cli-command` / `cli-renderer` / `subscription-hub` / `gui-protocol` / `gui-server` / `gui-client` / `connection-manager` / `snapshot-service` / `artifact-store` / `transport-api` / `transport-local` / `transport-memory` / `transport-remote-placeholder` / `client-auth`，以及 `apps/pawork` / `apps/protocol-test-gui`。
- **事实源**：当前源码（16 crate 逐文件实读 + 全仓 `rg` 交叉核对）、`plan/P13-*.md`、`ROADMAP.md` Phase 13 段、ADR-021/022/023/024/025/026/027/028/029/030/034/036、`docs/architecture/workspace-layout.md`、`docs/features/gui-connection.md` / `cli-host.md`。
- **方式**：Commander（GLM）统筹复核与最终结论；四个只读 `deepseek_explorer` 并行调查四条互不重叠的切片（app-service+core-runtime+cli-host / gui-protocol+schema-typegen+版本化 / gui-server+connection-mgr+subscription-hub+snapshot-service+artifact-store / transport-*+client-auth+gui-client）。Commander 对最关键的两项结论（gui-server 未装配、idempotency/expected_revision 死字段）再逐行 `rg` 复核。**本次只 Review，不修改实现。**
- **审查日期**：2026-08-10。

---

## 0. 总评

Phase 13 的设计目标——`pawork` 成为 Core 的唯一正式宿主、CLI 与 GUI 共享同一个 `app-service` 与 Event Hub、本地与远程 GUI 经同一 GUI Connection Protocol 接入、一个 Core 可同时服务多个 GUI、GUI 断线不取消 Run——**在库代码与协议层诚实落地**：`CommandRouter` 是 CLI 与 GUI 真正共享的唯一业务路由（经 `dispatch` / `dispatch_query` + `AppCommandEnvelope`，CLI 与 GUI 入站帧均经此，无绕过）；`RunSupervisor` 真实 `tokio::spawn` 跑 `ProviderLoop`（不再是 P12 那种「只 emit 状态迁移、不跑循环」）；GUI Connection Protocol 的握手 / 协商 / 编解码 / Snapshot / Resume / Ack / Artifact 分片有真实双向生产消费，并由 `multi_gui_runtime.rs` 与 `contract.rs` 集成测试实证 3 GUI 并发、断线重连 Replay、慢客户端隔离、命令幂等、100k 行 diff 流式；`schema-typegen` 导出完整且 `--check` 通过；`subscription-hub` 全局序列单调（`AtomicU64`）+ ring buffer + 有界广播，被 `core-runtime` 与 `cli-host` 生产消费。架构红线（纯 Rust、无 GUI framework 依赖、`agent-domain` 零业务依赖、无循环依赖、Transport 只搬运字节）全部满足。

但 Phase 13 最本质的问题不是「缺功能」，而是**一条贯穿全 Phase 的「库代码完整、正式宿主二进制不接线」断层**——这与 P12「四个子系统彼此不接线」是同一类结构性缺口，只是 P13 已在 crate 内部接好（gui-server 真把 connection-manager / subscription-hub / snapshot-service / artifact 串起来），缺的是「最后一公里」的宿主装配。具体表现：

1. **正式宿主 `pawork` 从未装配 GUI Server**：[apps/pawork/src/main.rs:22](../../apps/pawork/src/main.rs:22) 的 `// host.attach_gui_server(gui_server);` 被注释；`GuiServerHost` trait（[cli-host/src/lib.rs:55](../../crates/cli-host/src/lib.rs:55)）全仓零实现。`gui-server` 的完整 `GuiServer`（含握手 / Snapshot / Resume / 慢客户端隔离 / artifact 分片）只在测试、`harness.rs`、`protocol-test-gui` 中被 `GuiServer::new` 构造——**发布版 `pawork serve` 不会打开任何 GUI Endpoint**，与 [docs/features/cli-host.md](../features/cli-host.md)「serve 打开本地 GUI Endpoint」及 ADR-022「GUI 经 CLI 连接」的承诺漂移。这是 P13-4 自认的「装配位未注入」，属落地缺口而非接线缺失，但评审必须显式记录，避免误判「GUI 已可连」。
2. **正式宿主从未注册真实 Provider、从未注入 ArtifactStore**：`register_provider` 的生产调用者为零（仅测试 / harness / protocol-test-gui），`CoreRuntime::new` / `cli-host` 全部用 `AppService::new`（store=None）。后果：出厂 `pawork run` 必然返回 Authentication 错误（`modes.rs` 测试把「无 provider 报错」当预期），`artifact_read` 在发布二进制上恒返回 `Unavailable`。链路在生产二进制上整体不可达——不是 P13 的库级缺陷，但意味着 Phase 13 交付的「能跑」目前只在 MockProvider 测试范围内成立。
3. **协议契约的「双向版本校验」只接了半边**：服务端入站经 `decode_client_frame_checked` / `ensure_compatible_api_version` 真实生效，但服务端出站 `send_frame` 从不跑 `validate_server_frame_api_version`，gui-client 入站只用无版本校验的 `decode_server_frame`——`decode_server_frame_checked` / `validate_server_frame_api_version` 生产代码零调用。ADR-036 明确要求「服务端只发送协商 minor 内的帧、客户端只发送协商 minor 内的信封」双向校验，现状单向。
4. **「重试不重复建 Run」在默认 API 下不成立**：`IdempotencyStore` 服务端真实接线（check / record，错误不缓存），但 CLI（[cli-host/src/lib.rs:810](../../crates/cli-host/src/lib.rs:810)）与 GUI SDK（[gui-client/src/lib.rs:410](../../crates/gui-client/src/lib.rs:410)）都硬编码 `idempotency_key: None` 且每次调用新生成 `command_id`——去重仅当调用方显式复用信封时才生效。同理 `expected_revision` 全仓恒 `None`（乐观并发是死字段）。两项都是 P13-1 验收勾选「重试不重复创建 Run」但默认路径不满足的脱节。
5. **慢客户端隔离正确但 Lagged 对客户端不可见**：`try_send` + 满则标记 `lagged` + forwarder `continue` 不阻塞，机制正确且经测试触发；但 `session.lagged` 在生产代码中从无读取，协议无任何 Lagged / Resync 通知帧，客户端不知道自己丢过事件，丢事件在 `global_sequence` 上留下静默空洞，只能靠断线重连 Resume 按 `last_ack` 补齐（非实时）。

此外存在一批与 P12 同型的「为未来预留但当前无生产者」的死枚举 / 死字段（`AppEvent` 19 个变体中 10 个从未被生产代码产生；`ServerFrame::CommandAccepted` / `GuiCapability::TerminalStreaming` / `GuiCapability::Approvals` / `ProtocolErrorCode::PermissionDenied` 无生产构造路径；`expected_revision` / `CommandSource::Plugin` / `CommandSource::Mcp` 死字段），以及 transport-memory 与 transport-remote-placeholder Mock 之间约 250 行的逐行同构重复。

没有发现需要新增抽象的设计缺口。几乎所有建议都是「接线 / 删死码 / 收敛重复 / 收紧装配」，方向与本次「优先减少代码与概念」一致。**核心判断：Phase 13 的正确形态不是「16 个完整库 + 一个不装配它们的二进制」，而是「`pawork` 真正装配 gui-server + 真实 Provider + ArtifactStore，让 serve 模式可连、run 模式可跑」——当前交付了完整的运行时零件，但宿主没有把它们装上车。**

---

## 1. 设计符合度（正面结论）

| 设计目标 | 实现事实 | 证据 | 判定 |
| --- | --- | --- | --- |
| CLI 与 GUI 共享同一 Command Router（ADR-024） | `CommandRouter::dispatch` / `dispatch_query` 统一路由 `AppCommandEnvelope`；CLI（cli-host lib.rs:798/816 信封包装）与 GUI（session.rs:345 入站帧 → `dispatch_envelope` / `dispatch_query`）均经此，不直接调 agent-engine / workspace-service | [router.rs:174](../../crates/app-service/src/router.rs:174) / [router.rs:214](../../crates/app-service/src/router.rs:214) / [session.rs:345](../../crates/gui-server/src/session.rs:345) | 符合 |
| CLI 是 Core 唯一正式宿主，CLI 与 Core 同进程同二进制（ADR-021/025） | `apps/pawork` 装配 `CoreRuntime`（AppService + EventHub + EventPump）+ `CliHost`，四模式 run/serve/shell/service；不存在 core-daemon / core-cli / core-rpc 入口 | [main.rs:18-20](../../apps/pawork/src/main.rs:18) / [cli-host/src/lib.rs:231-570](../../crates/cli-host/src/lib.rs:231) | 符合 |
| RunSupervisor 真跑 ProviderLoop（修复 P12「只 emit 不跑 loop」） | `ProviderLoop::new` + `engine.run(queue, cancel)` 在 `tokio::spawn` 内真实执行，经 EventHub 等待终态并流式输出 | [supervisor.rs:405-410](../../crates/app-service/src/supervisor.rs:405) | 符合（库内） |
| 一个 CLI/Core 实例同时服务多个 GUI（ADR-023） | `ConnectionManager` 登记多 `GuiClientSession`（每连接有界事件队列 + 订阅 + lagged 标记），gui-server forwarder 经 `should_forward` 过滤后 `enqueue`；3 GUI 并发集成测试实证 CLI/GUI 互发 Run 与审批同步 | [connection-manager/src/lib.rs:241](../../crates/connection-manager/src/lib.rs:241) / [session.rs:144](../../crates/gui-server/src/session.rs:144) | 符合 |
| GUI 断线/退出不取消 Run（ADR-026） | 心跳超时断线清理只 `unregister` 连接，绝不取消 Run；恢复靠 Resume（`compute_resume_disposition` → Replay / SnapshotRequired / UpToDate） | [session.rs:406-440](../../crates/gui-server/src/session.rs:406) | 符合 |
| 慢 GUI 不阻塞 Core 或其他 GUI | `enqueue` 用 `try_send` 非阻塞，队列满标记 `lagged` 返回 `ManagerError::Lagged`；集成测试用 SlowTransport 灌 1300+ 事件实证快客户端与 Agent 不受影响 | [connection-manager/src/lib.rs:309](../../crates/connection-manager/src/lib.rs:309) / [multi_gui_runtime.rs:1088](../../crates/gui-server/tests/multi_gui_runtime.rs:1088) | 符合（隔离目标达成，但见 §3.4） |
| 本地与远程 GUI 同协议，差异只在 Transport（ADR-027/028） | `transport-api` 只搬运 `Vec<u8>`，零 gui-protocol 依赖；transport-local（UDS / Named Pipe）与 transport-remote-placeholder（Mock loopback）实现同一 transport traits，remote e2e 与 local 用完全相同协议帧 | [transport-api/src/lib.rs:15-26](../../crates/transport-api/src/lib.rs:15) / [remote_e2e.rs](../../crates/transport-remote-placeholder/tests/remote_e2e.rs) | 符合 |
| 大 payload 经 Artifact ID 传递、不内联（ADR-018） | `artifact-store.read_range` → `app-service.artifact_read`（offset/limit 流式）→ gui-server `artifact_chunks`（按 ≤64 KiB 分片，末片 eof）→ `ArtifactChunk`；事件流只携带 Artifact ID | [lib.rs:326](../../crates/app-service/src/lib.rs:326) / [session.rs:542](../../crates/gui-server/src/session.rs:542) | 符合（链路真，但见 §2.2 宿主未注入 store） |
| 协议帧编解码、握手、版本协商 | u32 LE 长度前缀分帧、`negotiate_api_version_with` 表式协商经 `HandshakeService::accept` 接入生产握手；Snapshot 校验 data/artifact_id 互斥且有界；`compute_resume_disposition` 三态 | [handshake.rs:24-37](../../crates/gui-protocol/src/handshake.rs:24) / [session.rs:303-318](../../crates/gui-server/src/session.rs:303) | 符合（协商主路径真，校验单向见 §3.5） |
| Schema 是 Rust 单一来源、TS 生成一致（P13-7/10） | `schema-typegen` 以 core-api 四信封 + ClientFrame/ServerFrame 为根 ts-rs 全量导出，`--check` 比对 `schemas/` 与 scratch 生成物，漂移即失败；`versions.d.ts` 含 API_VERSION / SUPPORTED_API_VERSIONS | [schema-typegen](../../crates/schema-typegen/src/main.rs) / ci.yml:42-43 | 符合 |
| `agent-domain` 零业务依赖红线 | 仅依赖 serde / serde_json / 可选 ts-rs | [agent-domain/Cargo.toml](../../crates/agent-domain/Cargo.toml) | 符合 |
| Transport 不含 Agent 业务逻辑 | transport-remote-placeholder 实现 1010 行全部是传输层内容（Provider/Connector trait / mock 注册表 / 地址生成），零 `agent-domain` 引用 | [transport-remote-placeholder/src/lib.rs:70-440](../../crates/transport-remote-placeholder/src/lib.rs:70) | 符合 |

**结论**：单看每个 crate 的内部行为与协议契约，与 P13-1～P13-10 的库级验收标准基本一致；架构红线全部保持。问题集中在「库 ↔ 正式宿主二进制」的装配缺口（§2、§3）与一批预留死枚举（§4）。

### 1.1 多 GUI 运行时三 crate 接线真实（deepseek 核查）

与 P12「四个子系统彼此不接线」不同，Phase 13 的 connection-manager / subscription-hub / snapshot-service **已在 gui-server 内真实互联**，并由 `multi_gui_runtime.rs`（3 GUI 并发 / 断线重连 / 慢客户端）与 `contract.rs`（9 场景）集成测试实证：

- `session.rs:144` register 连接 → `session.rs:452` `spawn_forwarder` 订阅 hub（subscription-hub）→ `session.rs:465` `should_forward` 过滤 → `session.rs:470` `enqueue`（connection-manager 有界队列）→ 帧循环逐帧 `ServerFrame::Event`。
- `session.rs:406` `handle_resume` 用 `hub.replay` / `hub.earliest_available` + `snapshots.build`（subscription-hub + snapshot-service）。
- `snapshot-service` 由 `app-service` AggregateState 生成全部 6 类 section（Workspaces / SessionTree / ActiveRuns / PendingToolApprovals / TerminalSessions / ProviderStatus），`snapshot_sequence` 取 `hub.current()`。

三者职责边界清晰（Hub=共享广播 / ConnectionManager=GUI 连接 / SnapshotService=GUI 快照），消费者分布与边界一致，**不建议合并**（合并会把 GUI 协议类型拖入 core-runtime，违反 workspace-layout 分层）。详见 §4。

---

## 2. 主流程接入与设计-实现一致性

> 严重度：〔高〕= 影响正确性或架构判断；〔中〕= 明显可收敛的脱节；〔低〕= 清理收益有限。

### 2.1〔高〕正式宿主 `pawork` 从未装配 GUI Server——serve 模式 GUI 不可达

**事实**：[apps/pawork/src/main.rs:22](../../apps/pawork/src/main.rs:22)：

```rust
// GUI Server 装配位（P13-4 注入）；未装配时 serve 仅等待信号。
// host.attach_gui_server(gui_server);
```

全仓 `rg attach_gui_server` 命中：`main.rs:22`（注释）、[cli-host/src/lib.rs:94](../../crates/cli-host/src/lib.rs:94)（`attach_gui_server` 方法定义）。`GuiServerHost` trait（[lib.rs:55](../../crates/cli-host/src/lib.rs:55)）**全仓零实现**。`GuiServer::new` 的全部构造点（`rg GuiServer::new`）只在 `harness.rs`、`contract.rs`、`multi_gui_runtime.rs`、`artifact_streaming.rs`、`remote_e2e.rs` 及 gui-server 自测——**生产 `pawork` 二进制不构造、不绑定、不 accept 任何 GUI 连接**。

后果：

- ADR-022「GUI 经 CLI 连接」、ADR-027「本地与远程同协议」在发布二进制上**无从成立**——没有端点可连。
- P13-2 验收「`pawork serve` 可独立启动完整 Core」字面成立（Core 启动 + 等信号），但 P13-2 验收「CLI 实时输出与 GUI 状态一致」、gui-connection 验收「本地和远程 GUI 可同时连接」在发布二进制上不可验证。
- [docs/features/cli-host.md](../features/cli-host.md) 与 [docs/architecture/workspace-layout.md §6](../architecture/workspace-layout.md) 都把「serve 打开本地 GUI Endpoint」写成已落地能力——文档-实现漂移。

这是 P13-4 计划自认的「装配位保留为 trait，P13-4 落地后验证」的遗留，但 P13-4 已标 🟢 TargetVerified，而装配从未发生。这是 Phase 13「最后一公里」断层的核心。

**建议方向（接线优先，不新增抽象）**：在 `apps/pawork`（或 `core-runtime` 装配阶段）真正构造 `GuiServer`（含 transport-local 端点绑定 + client-auth TokenStore + snapshot-service 注入 ArtifactStore），实现 `GuiServerHost` trait 并 `attach_gui_server`，让 `serve` 模式 `bind` 本地端点、`accept` 连接、走真实帧循环。cli-host 的 `GuiServerHost` 槽位已为此预留，无需改 trait。短期内若不接线，至少应在 cli-host.md / P13-4 计划显式声明「serve 的 GUI 端点装配位未注入，GUI 暂不可连」，避免 TargetVerified 掩盖缺口。

### 2.2〔高〕正式宿主无真实 Provider、无 ArtifactStore——run / artifact 链路生产不可达

**事实**：`register_provider` 的生产调用者（`rg register_provider`，排除测试 / harness / protocol-test-gui）**为零**。`CoreRuntime::new`（[core-runtime/src/lib.rs:63](../../crates/core-runtime/src/lib.rs:63)）与 cli-host 全部用 `AppService::new`（store=None），`with_artifact_store` 仅 [harness.rs:54](../../apps/protocol-test-gui/src/harness.rs:54) 调用。

后果：

- 出厂 `pawork run` 必然返回 Authentication 错误——[modes.rs:16](../../apps/pawork/tests/modes.rs:16) 把「无 provider 报错」当预期结果，说明这是已知状态。
- `artifact_read` 在发布二进制上恒走 `Unavailable` 分支（[app-service/src/lib.rs:338](../../crates/app-service/src/lib.rs:338)），P13-8「100,000 行 Diff 不需一次复制到 GUI」在宿主上不成立。
- RunSupervisor 虽真跑 ProviderLoop，但 `tools: Vec::new()`（[supervisor.rs:359](../../crates/app-service/src/supervisor.rs:359)）+ `execute_tools` 恒返回成功 no-op（[supervisor.rs:786](../../crates/app-service/src/supervisor.rs:786)）+ `NoopProcessTreeCleaner`（[supervisor.rs:180](../../crates/app-service/src/supervisor.rs:180)），loop 只在 MockProvider + 空工具的测试范围内可跑。

这并非 Phase 13 的库级缺陷（Provider 注册本属 provider-runtime / 配置装配，ArtifactStore 注入本属 core-runtime 装配阶段），但意味着 **Phase 13 交付的「能跑」目前仅在 MockProvider + 测试 harness 内成立**。

**建议方向**：在 core-runtime 或 cli-host 装配阶段补最小 Provider 注册路径（至少从 config-service 读取凭证 → provider-openai 等构造 → `register_provider`）与 ArtifactStore 创建注入。这会让 `pawork run` 与 `artifact_read` 在宿主上真正可达，否则 Phase 13 的 e2e 价值只停留在测试报告里。

### 2.3〔中〕「重试不重复建 Run」默认不成立——idempotency / expected_revision 是死字段

**事实**：

- `IdempotencyStore` 服务端真实接线（[router.rs:191](../../crates/app-service/src/router.rs:191) check / [router.rs:204](../../crates/app-service/src/router.rs:204) record，错误不缓存），但 CLI（[cli-host/src/lib.rs:810](../../crates/cli-host/src/lib.rs:810)）与 GUI SDK（[gui-client/src/lib.rs:410](../../crates/gui-client/src/lib.rs:410)）都硬编码 `idempotency_key: None`，且每次调用新生成 `command_id`。`rg idempotency_key` 在 cli-host / gui-client 命中的全是 `None`。
- `expected_revision`（[core-api/src/lib.rs:60](../../crates/core-api/src/lib.rs:60)）全仓恒 `None`（`rg expected_revision` 命中全是 `None`，仅 gui-protocol golden/frames fixture 与 wasm-plugin-host 测试用 `Some`）。乐观并发是死字段——服务端从不读取它做冲突检测。

后果：P13-1 验收「网络重试不会重复创建 Run 或消息」在默认 API 下不成立——只有调用方显式复用同一 `command_id` + `idempotency_key` 才生效，而 CLI/GUI 客户端从不复用。

**建议方向**：让 cli-host / gui-client 在「重试同一逻辑请求」时复用信封（command_id + idempotency_key），或在客户端重试层自动复用首次生成的 idempotency_key；若 `expected_revision` 短期不接线，应从协议面收敛或标注为 deferred，避免「协议有字段、全仓不读」的漂移。

### 2.4〔低〕EventPump 10ms 轮询合理但存在更简方案

**事实**：[core-runtime/src/lib.rs:111](../../crates/core-runtime/src/lib.rs:111) 用 `tokio::time::interval(10ms)` 轮询 `router.drain_events()` → `hub.publish`，配合 30ms 合并窗最坏 ~40ms 延迟。对 CLI 流式输出合理，且定时器驱动非忙轮询、简单可测。更简方案是 `tokio::sync::Notify`（或 mpsc）按需唤醒，可省掉固定延迟与 pump 任务，但当前实现非缺陷。判定：设计合理，存在简化空间，低优先。

---

## 3. 冗余 / 死代码 / 过度预留

### 3.1〔高〕`AppEvent` 19 变体中 10 个无生产者——协议面虚胖

**事实**：[core-api/src/lib.rs:337](../../crates/core-api/src/lib.rs:337) `AppEvent` 19 个变体，经 `rg "AppEvent::"`（排除 `match` / `=>` / 注释 / 测试 / cli-renderer 消费侧）核对，**以下 10 个从未被任何生产代码产生**：

`CoreReady`、`WorkspaceChanged`、`SessionChanged`、`DiffChanged`、`TerminalOutput`、`AuthChanged`、`ProviderStatus`、`PluginError`、`GuiClientConnected`、`GuiClientDisconnected`。

它们仅在 [cli-renderer/src/lib.rs:41-98](../../crates/cli-renderer/src/lib.rs:41) 被 `render_event` 的 `match` 分支「消费」（即渲染侧写了分支，但这些事件永远不会到达）。这与 ADR-024「CLI 与 GUI 看到同一事件流」承诺漂移——WorkspaceAdd / SessionCreate / GitStage / Terminal / Auth 等命令在 router 里只是「聚合记一笔」，**不产生任何 AppEvent**，Hub 流实质是「run-only」。

**建议方向**：要么补齐这些命令的事件产生路径（让 CLI/GUI 真正看到 workspace/session/diff/terminal/auth 变更），要么把无生产者的变体标注为 deferred 并从 cli-renderer 的 match 收敛（避免渲染侧写永不触达的分支）。前者是「接线」，后者是「删死码」，二选一即可。

### 3.2〔中〕router 命令面「接线真、语义假」比例偏高

**事实**（[router.rs:319](../../crates/app-service/src/router.rs:319) 起）：`RunTool` 返回 `Unavailable`、`AuthStart` / `AuthRemove`、`GitStage`、`Terminal*` 均为「聚合记一笔」的假实现；`PluginList` / `McpList`（[router.rs:510-511](../../crates/app-service/src/router.rs:510)）恒返回空数组；[app-service/src/lib.rs:552](../../crates/app-service/src/lib.rs:552) 等多处仍带 `"implementation_phase"` 标记。结合 §3.1，命令面大量「信封能收、聚合能写、但既不产生事件也不真正执行」。

这与 P13-1「app-service 完整化」表述有落差——完整化的是路由与聚合骨架，业务语义大半仍是占位。RunSupervisor 的 `tools: Vec::new()` 与 `execute_tools` no-op 同属此类。

**建议方向**：按真实需求优先级接线（RunTool 接 builtin-tools / Terminal 接 pty-service / GitStage 接 git-service），或在命令枚举上明确标注「deferred」避免「枚举存在即视为已实现」的误判。

### 3.3〔中〕transport-memory 与 transport-remote-placeholder Mock 约 250 行逐行同构

**事实**：`MemoryConnection`（[transport-memory/src/lib.rs:188](../../crates/transport-memory/src/lib.rs:188)）与 `MockRemoteConnection`（[transport-remote-placeholder/src/lib.rs:296](../../crates/transport-remote-placeholder/src/lib.rs:296)）是同一 mpsc 半部模式（`Mutex<Option<Sender>>` + `tokio Mutex<Receiver>` + `AtomicBool` + `FrameTooLarge` 校验）的逐行同构；`MemoryListener` / `MockRemoteListener` 同理，约 250 行复制。

**建议方向（减重复）**：把 channel-pair 提炼为 transport-api 下的共享 internal 模块（feature 化），或让 placeholder 的 Mock 直接复用 transport-memory 再改 locality / 地址语义——两 crate 各可减约 150 行。transport-memory 自身全部消费者都是测试侧，也可降为 transport-local 的 `memory` feature 或并入 test-support（见 §4.2）。

### 3.4〔中〕慢客户端 Lagged 对客户端不可见，丢事件静默

**事实**：隔离机制正确且触发（§1），但 `session.lagged`（connection-manager 内部标记）在 gui-server 生产代码中**从无读取**；协议无任何 `Lagged` / `Resync` / `SnapshotRequired` 通知帧（除 Resume 降级路径）。丢事件在 `global_sequence` 上留下空洞，活跃连接内客户端无法察觉，只能靠断线重连 Resume 按 `last_ack` 补齐（ADR-030 语义，但非实时）。另：[session.rs:515](../../crates/gui-server/src/session.rs:515) 把 `ManagerError::Lagged` 映射为 `ReplayUnavailable`，但该映射仅在 Subscribe/Unsubscribe 错误帧可达，与转发丢事件无关，易误导。

注意区分两套滞后：connection-manager 的 `enqueue` 满会置 `lagged`（有内部留痕）；forwarder 的 broadcast 落后（`HubError::Lagged`）走 `Err(_) => continue`（[session.rs:473](../../crates/gui-server/src/session.rs:473)）**同样静默丢事件且不置 lagged**——只有前者留痕。

**建议方向**：在协议层补一个「Lagged / Resync 建议」服务端→客户端通知帧（minor bump 内可加，符合 ADR-036），或至少让 forwarder 的 broadcast Lagged 也置 `session.lagged` 并在下次出站帧附带「你可能落后」提示，让客户端主动触发 Resume。当前「静默丢事件 + 仅靠重连补齐」对长连接慢客户端体验不友好。

### 3.5〔中〕协议版本校验单向——出站 / 客户端侧未接线

**事实**：协商主路径真实（`negotiate_api_version_with` → `HandshakeService::accept` → `negotiated_version` 进帧循环），服务端入站经 `decode_client_frame_checked` / `ensure_compatible_api_version` 真实生效（[session.rs:219](../../crates/gui-server/src/session.rs:219)）。但：

- 服务端出站 `send_frame` 只调 `encode_server_frame`，从不跑 `validate_server_frame_api_version`（[session.rs:594-602](../../crates/gui-server/src/session.rs:594)）。
- gui-client 入站只用无版本校验的 `decode_server_frame`（[gui-client/src/lib.rs:795](../../crates/gui-client/src/lib.rs:795)、[:841](../../crates/gui-client/src/lib.rs:841)），出站只盖戳 `api_version()`。
- `decode_server_frame_checked` / `validate_server_frame_api_version` 生产代码**零调用点**，仅 gui-protocol 自测覆盖。

ADR-036 明确要求双向（「服务端只发送协商 minor 内的帧与事件，客户端只发送协商 minor 内的信封」），现状单向。短期内 major 不变时无功能影响，但「校验函数写好却不接」属预留死代码，且一旦 minor 演进会暴露缺口。

**建议方向**：要么把 `validate_server_frame_api_version` 接入服务端出站、`decode_server_frame_checked` 接入客户端入站（接线），要么删除这两个零调用函数并在 ADR-036 标注「双向校验 deferred」（删死码）。

### 3.6〔低〕`ServerFrame::CommandAccepted` 与若干冻结枚举值未接线

**事实**：`ServerFrame::CommandAccepted`（[gui-protocol/src/lib.rs:84](../../crates/gui-protocol/src/lib.rs:84)）无任何生产生产者（session.rs:344 注释自认推迟到 P13-5，但 P13-5 仍未产生）。`GuiCapability::TerminalStreaming`（lib.rs:140）仅测试出现，生产服务端只声明 Events/Snapshots/ArtifactStreaming；`GuiCapability::Approvals`（lib.rs:141）全仓零使用；`ProtocolErrorCode::PermissionDenied`（lib.rs:276）无构造路径。`CommandSource::Plugin` / `CommandSource::Mcp`（router.rs:714-715）仅 source_name 分支、生产无构造。

**判定**：这些多为冻结协议内的预留变体，符合 ADR-036「变体只可废弃不可删除」策略，但应登记为「已定义未接线」或补接线，避免「枚举存在即视为已实现」误判。低优先。

### 3.7〔低〕aggregate 死面与 connection-manager 死公开 API

**事实**（[aggregate.rs](../../crates/app-service/src/aggregate.rs)）：`git_stages` 只写不读（router.rs:366 写入后无任何查询 / 快照引用）；`seed_diff`（:532）仅测试调用，DiffListFiles/DiffGet 查询生产恒空；`clear_run_approvals`（:453）无调用者；`note_gui_connect` / `note_gui_disconnect`（:607/:622）生产无调用（gui-server 用 connection-manager，从不更新聚合）。connection-manager：`is_subscribed`（lib.rs:288）、`expired_clients`（lib.rs:346）、`sessions()`（lib.rs:195）生产代码零调用（仅自带单测）。client-auth：`TokenAuthenticator::store()`（lib.rs:199）、`scheme()`（lib.rs:203）全仓零调用。

**建议方向**：删除无调用者的公开 API（pub 方法不受 dead_code lint，需手动清理）；`note_gui_connect` / `note_gui_disconnect` 若要聚合 GUI 状态应在 gui-server 接线，否则删。

### 3.8〔低〕client-auth `generate` 非原子创建

**事实**：[client-auth/src/lib.rs:87-91](../../crates/client-auth/src/lib.rs:87) 用 `exists()` 预检 + `File::create`，非原子创建，并发双进程存在截断窗口。建议 `OpenOptions::create_new(true)`。Windows 侧未收紧 token 文件 ACL（仅 unix cfg），本地多用户环境注意。低优先。

---

## 4. 合并 / 拆分 / 简化建议

> 总原则：本次「优先减少代码与概念」。下列建议按收益排序，无新增抽象。

### 4.1〔建议执行〕消除 transport-memory ↔ transport-remote-placeholder Mock 的 ~250 行重复

两 crate 的 `MemoryConnection` / `MockRemoteConnection`、`MemoryListener` / `MockRemoteListener` 是同一 channel-pair 模式的逐行同构。提炼为 transport-api 下的共享 internal 模块（feature 化），或让 placeholder Mock 复用 transport-memory 改 locality / 地址语义。两 crate 各可减约 150 行，且消除「同一逻辑两处维护」的漂移风险。

### 4.2〔可选〕transport-memory 降为 transport-local 的 `memory` feature 或并入 test-support

transport-memory（434 行）全部消费者都是测试侧（gui-server / gui-client 仅 dev-deps，protocol-test-gui 是测试 app）。当前作为独立 workspace member 增加了 crate 计数。可降为 transport-local 的 `memory` feature 或并入 test-support，与 §4.1 合并执行收益更大。**不强制**——独立利于语义对等测试，维持现状可接受。

### 4.3〔不建议合并〕connection-manager / snapshot-service 不并入 gui-server

虽两者生产消费者仅 gui-server，但：connection-manager 依赖 gui-protocol + transport-api，snapshot-service 依赖 app-service + subscription-hub；并入 gui-server 会让 gui-server 直接依赖更多上游，且当前分离利于独立测试（connection-manager 6 项 / snapshot-service 3 项单测）。**维持现状**。

### 4.4〔不建议合并〕gui-protocol 五模块拆分合理

gui-protocol/src 共 881 行（lib.rs 281 / handshake.rs 263 / codec.rs 180 / error.rs 67 / resume.rs 55 / snapshot.rs 35），拆 5 模块在此体量下不构成过度碎片化，且职责边界清晰（lib.rs:13-19 模块文档）。可选合并：`resume.rs`（55 行，唯一调用在 handshake.rs:168）→ handshake、`snapshot.rs`（35 行，唯一调用在 codec.rs:129）→ codec，可减为 3 模块 + lib。**属可选项而非缺陷**。

### 4.5〔不建议改动〕transport-api 必须独立

transport-api 仅 141 行，但是 6 个 crate（gui-server / connection-manager / gui-client 生产 + 3 个 transport 实现）的共享枢纽，保持零 tokio 依赖（仅 async-trait / serde / thiserror）。若并入 transport-local，则 gui-server / connection-manager / gui-client 全部被迫链接 transport-local（tokio net + 平台代码），把 socket 实现拉进 Desktop SDK 与 Core 侧连接管理，直接破坏 ADR-034 的依赖链 `gui-client → transport-api → gui-server`。141 行换一个无环、零平台依赖的抽象层，成本可忽略。**维持现状**。

### 4.6〔建议执行〕收敛 cli-host / app-service 的双入口与 Placeholder stub

cli-host 有 legacy `ServiceOperation::dispatch`（status/doctor/serve/shell/shutdown）作为并行的第二入口，且 `run_operation` 内部又重入 router（[app-service/src/lib.rs:148](../../crates/app-service/src/lib.rs:148)）——双表面设计，非业务绕过但建议收敛为单一 router 路径。cli-host lib.rs:896 起十个命令族（Workspace/Session/Approval/Gui/Provider/Auth/Plugin/Mcp/Models/Tools/ImportPi/Benchmark）全是 `Placeholder` stub——应明确标注 deferred 或按真实需求接线，避免「命令枚举存在即视为已实现」。

---

## 5. 架构符合度

### 5.1 架构红线全部满足

- **纯 Rust Core / 无 GUI framework 嵌入**：16 crate 全 Rust，无 Node / Bun / V8 / 嵌入式 JS Runtime；gui-client 不链接 core-runtime / app-service 运行时（[Cargo.toml](../../crates/gui-client/Cargo.toml) 常规 deps 仅 agent-domain / client-auth / core-api / gui-protocol / thiserror / tokio / transport-api，app-service / gui-server 仅 dev-deps，core-runtime 零出现），与 ADR-034/035 一致。
- **`agent-domain` 零业务依赖**：仅 serde / serde_json / 可选 ts-rs，不依赖 GUI / SQLite / HTTP / Keychain / Git / 具体 Provider。
- **无循环依赖**：依赖方向单向（agent-domain ← core-api ← app-service ← cli-host / gui-server；transport-api 零业务依赖；gui-protocol ← client-auth / connection-manager / gui-server / gui-client / snapshot-service / transport-remote-placeholder / schema-typegen）。
- **Transport 不含 Agent 业务逻辑**：transport-remote-placeholder 1010 行全部是传输层内容，零 agent-domain 引用（仅 dev-deps + e2e 测试装配）。
- **Secret 不落库 / 不入日志**：client-auth Token 不实现 Serialize / Display，Debug 脱敏（lib.rs:45-50），constant-time 比较（lib.rs:205-210）。

### 5.2 分层与依赖方向正确

workspace-layout.md §6 的依赖图（agent-domain → provider-api/tool-api/plugin-api → agent-engine → core-runtime → app-service → cli-host / gui-server → transport-api → transport-local / transport-remote-placeholder；gui-client ↑ Desktop 独立进程）在 P13 实现中如实落地。subscription-hub 作为共享件被 core-runtime（EventPump）与 cli-host（watch）生产消费，不拖入 GUI 协议类型——分层正确。

### 5.3 唯一架构层缺口：宿主装配断层

架构本身（crate 拆分、依赖方向、协议分层）是合理的。唯一的架构层问题是 §2.1/§2.2：正式宿主 `pawork` 没有完成「把已造好的零件装上车」——gui-server / 真实 Provider / ArtifactStore 均未在 `apps/pawork` 装配。这不是 crate 结构问题，而是装配阶段的 TODO 被标成了 TargetVerified。

---

## 6. 改进优先级（总结）

按「正确性 / 架构判断影响」→「明显可收敛」→「清理收益」排序：

| 优先级 | 项 | 类型 | 建议 |
| --- | --- | --- | --- |
| P0 | §2.1 正式宿主未装配 GUI Server（serve 不可连） | 接线 | 在 apps/pawork / core-runtime 装配阶段构造 GuiServer + transport-local 端点 + client-auth + snapshot-service，实现 GuiServerHost trait 并 attach；或显式标注 serve GUI 端点 deferred |
| P0 | §2.2 宿主无真实 Provider / ArtifactStore（run / artifact 生产不可达） | 接线 | core-runtime / cli-host 装配阶段补最小 Provider 注册路径（config → provider-* → register_provider）与 ArtifactStore 创建注入 |
| P1 | §2.3 idempotency_key / expected_revision 默认死字段 | 接线或收敛 | 客户端重试层复用 idempotency_key；expected_revision 接线或标注 deferred |
| P1 | §3.4 慢客户端 Lagged 静默丢事件 | 协议补帧 | 补 Lagged/Resync 服务端→客户端通知帧（minor bump），或至少让 broadcast Lagged 也置标记 |
| P1 | §3.5 协议版本校验单向 | 接线或删死码 | 接入 validate_server_frame_api_version / decode_server_frame_checked，或删除零调用函数并标注 deferred |
| P2 | §3.1 AppEvent 10/19 变体无生产者 | 接线或删死码 | 补命令事件产生路径，或收敛 cli-renderer match + 标注 deferred |
| P2 | §3.2 router 命令面「接线真、语义假」 | 接线 | 按需求优先级接线 RunTool/Terminal/GitStage，或标注 deferred |
| P2 | §4.1 transport-memory ↔ placeholder Mock ~250 行重复 | 去重复 | 提炼共享 channel-pair 或让 mock 复用 transport-memory |
| P3 | §3.6 冻结枚举未接线（CommandAccepted / GuiCapability / PermissionDenied） | 登记或接线 | 登记为「已定义未接线」或补接线 |
| P3 | §3.7 aggregate 死面 / connection-manager 死公开 API / client-auth 死 getter | 删死码 | 删除无调用者的 pub API |
| P3 | §4.6 cli-host 双入口 + Placeholder stub | 收敛 | 收敛为单一 router 路径，stub 标注 deferred |
| P3 | §3.8 client-auth generate 非原子创建 | 小修 | 改 OpenOptions::create_new(true)，Windows 收紧 ACL |
| P4 | §2.4 EventPump 10ms 轮询 | 可选简化 | 评估 tokio::sync::Notify 按需唤醒 |
| P4 | §4.2 transport-memory 降级 / §4.4 gui-protocol 模块合并 | 可选 | 维持现状可接受 |

**P0/P1 是结构性的「接线 / 协议补全」，P2/P3 是「删死码 / 去重复 / 收敛」，P4 是可选简化。** 全部方向与「优先减少代码与概念」一致，无新增抽象建议。

---

## 7. 行数勘误

四个 explorer 对任务书行数估计的实测勘误（均以当前工作区实读为准，任务书行数为过时估计）：

| 文件 / crate | 任务书估计 | 实测 |
| --- | --- | --- |
| cli-host/src/lib.rs | ~3882 | 1389（非空 1319） |
| gui-protocol/src（全） | ~2000 | 881 |
| transport-api/src | ~272 | 141（117+24） |
| transport-memory/src | ~1032 | 434（272+162） |
| transport-remote-placeholder | ~1955 | 1010（lib 781 + e2e 229） |
| gui-client（lib + tests） | ~2377 | 1889（lib 880 + contract.rs 1009） |
| gui-server lib.rs / session.rs | 2181 / 1695 | 814 / 623 |

行数虚高不影响「碎片化 / 重复」定性，但说明后续派发任务书应先实测行数再下结论。

---

## 7.5 修复记录（review-remediation）

**修复任务**：[P13-11](../../plan/P13-11-review-remediation.md) · 状态：🟢已完成 · TargetVerified · 修复日期：2026-08-11

Commander（GLM）统筹 + 1 个 `deepseek_explorer` 评估 §2.1 装配可行性 + 5 个写集互不重叠的 `deepseek_worker` 并行执行（W1 §2.1 宿主装配 / W2 §3.5 双向版本校验 / W3 §3.7 删 aggregate+connection-manager 死 API / W4 §3.7+§3.8 client-auth 死 getter+原子创建 / W5 §4.1 transport 去重——W5 经核验确认需扩 transport-memory 公开面与减概念冲突，显式延后）+ 1 个 `deepseek_reviewer` 独立复核（verdict PASS，1 项遗漏）+ Commander 后处理清理 reviewer finding #1（`gui_clients` 死字段）。

### 已修复（§2/§3/§4）

| 章节 | 问题 | 处置 |
| --- | --- | --- |
| §2.1 | 正式宿主 `pawork` 从未装配 GuiServer——serve 模式 GUI 不可达 | 新增 `ServeGuiHost`（全仓首个 `GuiServerHost` 生产实现）+ main.rs 装配链；`pawork serve` 绑定本地端点（Unix socket / Windows named pipe）+ accept + 真实帧循环；3 集成测试 |
| §3.5 | 协议版本校验单向——`validate_server_frame_api_version` / `decode_server_frame_checked` 零生产调用 | 服务端 `send_frame` 接入 `validate_server_frame_api_version`（7 调用点，pre-negotiation `None` 跳过）；客户端两处入站接入 `decode_server_frame_checked`；新增 `ClientError::Version`；ADR-036 双向校验落地 |
| §3.7 | aggregate 死公开 API（clear_run_approvals/note_gui_connect/note_gui_disconnect）+ 死字段（gui_clients/GuiClientRecord） | 删 3 方法 + gui_clients 字段/gui_clients() 访问器/Snapshot.gui_clients/GuiClientRecord 结构 + re-export + 失活 import（reviewer finding #1 Commander 后处理） |
| §3.7 | connection-manager 死公开 API（sessions/is_subscribed/expired_clients）+ client-auth 死 getter（store/scheme） | 删 6 项 + 裁剪仅测这些 API 的断言（保留 should_forward/is_timed_out/heartbeat 覆盖） |
| §3.8 | client-auth `generate` 非原子创建（exists()+create TOCTOU） | 改 `OpenOptions::create_new(true)`，消除并发双进程截断窗口；新增 `generate_is_atomic_under_contention` 断言预存内容不截断 |

### 显式延后

- **§2.2 宿主真实 Provider / ArtifactStore 注入** → Provider v2（Phase 15）/ 账号控制面（Phase 18）配套接线
- **§2.3 idempotency_key / expected_revision 默认死字段** → 客户端重试层复用信封或接线乐观并发
- **§3.1 AppEvent 10/19 死变体** → 补命令事件产生路径或收敛 cli-renderer match + 标注 deferred
- **§3.2 router 命令面语义假** → 按真实需求接线（RunTool→builtin-tools、Terminal→pty-service、GitStage→git-service）
- **§3.4 慢客户端 Lagged 静默丢事件** → 补 Lagged/Resync 服务端→客户端通知帧（minor bump）
- **§3.6 冻结枚举未接线** → 按 ADR-036 登记为「已定义未接线」
- **§4.1 transport-memory ↔ transport-remote-placeholder 去重** → DeepSeek worker 核验确认需扩 transport-memory 公开面（消费者全为测试侧）与减概念冲突，review §4.2 亦判「维持现状可接受」；延后至真 remote transport（P17-11）落地时评估
- **§4.6 cli-host 双入口 + Placeholder stub** → 收敛为单一 router 路径 + stub 标注 deferred

### 验证记录（2026-08-11）

- `cargo test --workspace --all-targets`：**1155 passed / 0 failed**（94 个 test result 行）；新增 9 测试（gui_serve 3 / send_frame 版本校验 2 / client 版本校验 3 / client-auth 原子 1）全过
- `cargo clippy`（app-service / connection-manager / client-auth / gui-server / gui-client / cli-host / core-runtime / snapshot-service，`--all-targets -- -D warnings`）：通过
- `cargo fmt --all -- --check`：通过
- `cargo run -p schema-typegen -- --check`：通过（W2 新增 `ClientError::Version` 不在 schema 根——协议帧才导出）
- 跨 crate 引用一致性 `rg` 复核：`GuiClientRecord` / `gui_clients()` / `note_gui_connect` / `clear_run_approvals` 零生产命中；`validate_server_frame_api_version` / `decode_server_frame_checked` 现有生产调用点；`TokenAuthenticator` `store()`/`scheme()` 零命中；`cargo check -p protocol-test-gui`（gui-client API 唯一非门禁消费者）通过

### 关键实证修正

review §4.1 建议「提炼 transport-memory ↔ transport-remote-placeholder 共享 channel-pair」，DeepSeek worker 实测发现：两 crate 的 pair 类型（`MemoryConnection` 私有 / `MockRemoteConnection`）虽同构，但 transport-memory 的 pair 全私有、无公开构造器，且 `MemoryTransport::connect` 硬编码 `ConnectionLocality::InProcess` + `memory-client-*` ID，remote 需 `Remote` + 自定义 ID + publish/unpublish 语义。去重需扩大 transport-memory 公开面（其消费者全为测试侧），与「优先减少代码与概念」冲突；review §4.2 亦判「维持现状可接受」。故 §4.1 显式延后而非机械执行。reviewer finding #1（删 writer 后 `gui_clients` 字段失活）Commander 核实该字段非冻结协议 schema（gui-protocol 公开 Snapshot 用通用 `SnapshotSectionKind` 无 GuiClients 变体，snapshot-service 从不读它）后一并清理，避免「删半截」。

## 8. 相关文档

- 计划：[P13-1](../../plan/P13-1-app-service.md) ～ [P13-10](../../plan/P13-10-protocol-schema-version.md)
 - 评审修复：[P13-11](../../plan/P13-11-review-remediation.md)
- 架构：[workspace-layout](../architecture/workspace-layout.md) · [api-surface](../architecture/api-surface.md) · [overview](../architecture/overview.md)
- 功能：[gui-connection](../features/gui-connection.md) · [cli-host](../features/cli-host.md) · [artifacts](../features/artifacts.md)
- ADR：[ADR-021](../adr/ADR-021-cli-core-same-process.md) · [022](../adr/ADR-022-gui-connects-via-cli.md) · [023](../adr/ADR-023-one-core-many-guis.md) · [024](../adr/ADR-024-shared-app-service-event-hub.md) · [025](../adr/ADR-025-cli-is-sole-host.md) · [026](../adr/ADR-026-gui-disconnect-safe.md) · [027](../adr/ADR-027-local-remote-same-protocol.md) · [028](../adr/ADR-028-replaceable-remote-transport.md) · [029](../adr/ADR-029-no-peer-gui-sync.md) · [030](../adr/ADR-030-core-sole-source-of-truth.md) · [034](../adr/ADR-034-desktop-gui-client-boundary.md) · [036](../adr/ADR-036-gui-protocol-versioning.md)
- 历史 Review：[p12-review](p12-review.md)（同类「库完整但接线断层」对照）
