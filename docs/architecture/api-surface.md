# GUI Connection Protocol（GUI 接入协议）

原「Core Client Protocol」更名为 **GUI Connection Protocol**。该协议只用于 GUI 与 CLI/Core 宿主之间的通信。CLI 命令不需要通过网络协议绕回自身，但必须调用与 GUI Command 相同的 Application Service。

Phase 13 不实现真实 GUI，但必须先冻结协议；Phase 19 的 Desktop GUI 只消费该契约。Rust 类型（`core-api` / `gui-protocol`）是唯一 schema source，自动生成 TypeScript 类型。

## 1. 命令路径

```text
CLI Command → app-service → core-runtime
GUI → GUI Connection Protocol → gui-server → app-service → core-runtime
```

两条路径共享同一个 `app-service` 实例、同一个 Command Router 与同一个 Event Hub，保证 CLI 与 GUI 的业务行为一致，同时避免 CLI 对自身建立不必要的 IPC 连接。

> 边界：GUI Connection Protocol 仅用于 GUI↔`pawork` Host。Codex App Server / Claude Gateway（P18-11/12）、IDE（P17-9）/ Agent SDK / Headless（P17-8）/ ACP（P17-7）/ Mobile（P17-12）是并列的 Client Channel，各自走自己的接入语义，共享同一 `app-service` 但**不消费 GUI 协议帧**，也不构造第二 Core。外部 Agent Client 统一实现 P18-10 `ClientAdapter` 契约；GUI 不实现该契约（[ADR-021](../adr/ADR-021-cli-core-same-process.md)、[ADR-025](../adr/ADR-025-cli-is-sole-host.md)、[ADR-030](../adr/ADR-030-core-sole-source-of-truth.md)、[ADR-033](../adr/ADR-033-control-plane-separation.md)、总体架构 §2.5）。

## 2. Command / Query / Event / Snapshot 示例

Command（写操作，进入 Command Router）：

```text
core.initialize
workspace.list / workspace.add / workspace.trust
session.create / session.open / session.fork / session.compact
run.start / run.cancel / run.retry
model.list
auth.start / auth.remove
tool.approve
diff.list_files / diff.get_hunks
git.stage
terminal.create / terminal.write / terminal.resize
plugin.list
mcp.list
plan.create / plan.review / plan.approve
goal.create / goal.update / goal.complete
task.cancel / task.resume
automation.create / automation.trigger / automation.pause
monitor.start / monitor.stop
memory.forget
review.resolve
hook.approve
```

Query（只读，可返回 Artifact ID 或 Snapshot 片段）：

```text
session.get
run.status
diff.get
artifact.read
snapshot.fetch
plan.get / goal.get / task.list
automation.list / automation.inbox
monitor.status / memory.search / review.list
```

Event（由 Event Hub 扇出到 CLI 渲染器与所有 GUI）：

```text
core.ready
workspace.changed
session.changed
run.changed
assistant.delta
thinking.delta
tool.started / tool.output / tool.approval_required / tool.completed
diff.changed
terminal.output
auth.changed
provider.status
plugin.error
server_tool.started / server_tool.progress / server_tool.completed
reasoning.committed
plan.changed / goal.changed / task.changed
automation.triggered / automation.result_archived
monitor.changed / memory.changed / review.changed / hook.completed
diagnostic
gui.client.connected / gui.client.disconnected
```

Snapshot（重连恢复用，含 snapshot_sequence）：

```text
workspace.list
session.tree
run.active
tool.pending_approval
terminal.sessions
provider.status
plan.active
goal.active
task.background
automation.schedules
monitor.active
review.open_findings
```

`reasoning.committed` 只携带 `ReasoningItem` 摘要与 `protected_blob_ref`。GUI Command / Query / Event / Snapshot 均不得返回 Protected Blob 明文、解密句柄或密钥材料；只有 Provider continuation 的受信运行时可按作用域解析引用（ADR-032）。

## 3. 统一 Command Source

CLI 和 GUI 发起的命令统一转换为信封，所有状态变化都记录来源与身份，以便 GUI 和 CLI 显示「任务由本地 CLI 发起 / 由远程 GUI 发起」「审批由 GUI B 完成」「取消由 CLI 执行」等。

```rust
pub struct AppCommandEnvelope {
    pub api_version: ApiVersion,
    pub command_id: CommandId,
    pub source: CommandSource,
    pub identity: ActorIdentity,
    pub expected_revision: Option<u64>,
    pub idempotency_key: Option<String>,
    pub issued_at: Timestamp,
    pub command: AppCommand,
}

pub enum CommandSource {
    LocalCli { terminal_session_id: Option<TerminalSessionId> },
    LocalGui { client_id: GuiClientId },
    RemoteGui { client_id: GuiClientId, connection_id: ConnectionId },
    HeadlessClient { client_id: ClientId },
    Ide { client_id: ClientId },
    Acp { client_id: ClientId },
    Mobile { client_id: ClientId },
    Automation,
    Plugin,
    Mcp,
}
```

网络重试不会重复创建 Run 或消息：每个信封携带 `command_id` 与可选 `idempotency_key`，Command Router 据此去重。

## 4. Event Hub 与事件信封

Core Event 首先进入 CLI 进程内的统一 Event Hub，再扇出到 CLI Renderer、所有 GUI、Audit Log 与 Automation Hooks。CLI 的实时输出来自 Event Hub 而非执行函数单独打印，从而保证 CLI 显示的状态与 GUI 相同，`pawork watch` 可显示所有客户端活动。

```text
core-runtime → Event Hub ─┬─ CLI Renderer
                          ├─ Local GUI A / B
                          ├─ Remote GUI C
                          ├─ Audit Log
                          └─ Automation Hooks
```

```rust
pub struct AppEventEnvelope {
    pub api_version: ApiVersion,
    pub instance_id: CoreInstanceId,
    pub event_id: EventId,
    pub global_sequence: GlobalSequence,
    pub stream: EventStream,
    pub stream_sequence: u64,
    pub timestamp: Timestamp,
    pub source: EventSource,
    pub payload: AppEvent,
}
```

文件相关的 Core API 参数使用构造与反序列化时均校验的 `WorkspaceRelativePath`；跨平台拒绝绝对路径、Windows drive/UNC 前缀与 `..` traversal。唯一例外是登记新 Workspace 时的 `workspace.add.root_path`，它由后续 Workspace/Policy 服务解析并建立 `workspace_id` 边界。

## 5. 多客户端同步模型

权威状态：CLI 内部运行的 Core 是唯一权威状态来源（Workspace / Session / Branch / Run / Message / Tool Call / Approval / Git 与 Diff / Terminal / Provider / Plugin / MCP / Artifact）。CLI 输出和所有 GUI 都是该状态的观察者与操作入口（[ADR-030](../adr/ADR-030-core-sole-source-of-truth.md)）。

GUI 首次连接：握手与认证 → 获取 Core Instance 信息 → 请求 Snapshot → 获得 `snapshot_sequence` → 订阅之后的 Event。

GUI 重连：提交 `last_global_sequence`，事件仍可重放则发送缺失事件，否则重新发送 Snapshot。

多 GUI 一致性：多个 GUI 不进行客户端之间的点对点同步；任何 GUI 的操作必须先提交给 CLI/Core，成功后再由 Core Event 广播给所有 GUI（[ADR-029](../adr/ADR-029-no-peer-gui-sync.md)）。

连接管理、Transport 抽象、慢客户端隔离与单/多实例等运行时细节见 [GUI 连接与多客户端](../features/gui-connection.md)。

## 6. 设计要求

- Rust 类型是唯一 schema source，自动生成 TypeScript 类型
- API version 与版本协商
- request / command ID 与 idempotency key
- event sequence（global_sequence 严格递增）
- cancel token
- 结构化错误
- 大型数据只返回 Artifact ID（[ADR-018](../adr/ADR-018-large-payload-artifact-id.md)）
- bounded channel 与 backpressure
- GUI 断开不影响 Run（[ADR-026](../adr/ADR-026-gui-disconnect-safe.md)）
- GUI 重连可获取 Snapshot 或 Event Replay
- 一个 Core 实例同时连接多个本地与远程 GUI（[ADR-023](../adr/ADR-023-one-core-many-guis.md)）

生成入口为 `cargo run -p schema-typegen`，输出提交到 `schemas/core-api/` 与 `schemas/gui-protocol/`；`--check` 模式和 CI 会拒绝类型漂移。

大型 payload 传递见 [ADR-018](../adr/ADR-018-large-payload-artifact-id.md)；GUI 不直接访问底层见 [ADR-017](../adr/ADR-017-gui-no-direct-access.md)；GUI 经协议连接 CLI 见 [ADR-022](../adr/ADR-022-gui-connects-via-cli.md)；共享 app-service 与 Event Hub 见 [ADR-024](../adr/ADR-024-shared-app-service-event-hub.md)。

## 7. 相关文档

- [总体架构](overview.md)
- [GUI 连接与多客户端](../features/gui-connection.md)
- [CLI Host](../features/cli-host.md)
- [Desktop GUI](../features/desktop-gui.md)
- [artifacts](../features/artifacts.md)
- [ROADMAP Phase 13 / Phase 19](../../ROADMAP.md)
