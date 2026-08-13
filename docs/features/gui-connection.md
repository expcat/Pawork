# GUI 连接与多客户端

## 职责

定义 GUI 作为独立进程如何连接 CLI/Core，以及一个 CLI 实例如何同时服务多个本地与远程 GUI。本模块只描述运行时与连接层；协议契约（Command/Query/Event/Snapshot、统一 Command Source、Event Hub、多客户端同步）见 [GUI Connection Protocol](../architecture/api-surface.md)。

## 设计要点

- GUI Server 运行在 CLI 进程内，与 CLI 命令共享同一个 `app-service` 与 Event Hub。
- 一个 CLI/Core 实例可同时连接多个 GUI；本地与远程 GUI 可同时在线。
- GUI 之间不进行点对点同步，所有状态统一由 Core 广播（[ADR-029](../adr/ADR-029-no-peer-gui-sync.md)、[ADR-030](../adr/ADR-030-core-sole-source-of-truth.md)）。
- Transport 不包含 Agent 业务逻辑；本地与远程 GUI 使用同一 GUI Connection Protocol（[ADR-027](../adr/ADR-027-local-remote-same-protocol.md)）；远程能力由可替换 Adapter 提供（[ADR-028](../adr/ADR-028-replaceable-remote-transport.md)）。
- GUI 断线/退出不结束正在运行的 Agent（[ADR-026](../adr/ADR-026-gui-disconnect-safe.md)）。

## 组件

| 组件 | 职责 |
| --- | --- |
| `gui-server` | CLI 内部运行的 GUI 协议服务器：握手、认证、提供 Query/Command、广播 Event、提供 Snapshot、断线重放、流式终端与 Artifact 传输 |
| `connection-manager` | 管理一个 CLI 实例上的多个 GUI：心跳、断线、重连、慢客户端隔离、每 GUI 独立权限与订阅 |
| `subscription-hub` | 将 Core Event 广播给 CLI 渲染器与所有 GUI，保证相同顺序 |
| `snapshot-service` | 为 GUI 生成当前状态快照与重连恢复（Snapshot 重建或 Event Replay） |
| `client-auth` | GUI 客户端身份验证 |
| `gui-client` | 协议测试客户端与未来 GPUI Desktop 复用的 Rust 连接 SDK（握手/订阅/Snapshot/Resume/Artifact 分片读取/心跳），无 GUI 框架依赖 |
| `transport-*` | 传输实现：`transport-local`（Unix Socket / Named Pipe）、`transport-memory`（测试）、`transport-remote`（真实远程 Transport：TCP + TLS 1.3，当前仅 127.0.0.1 loopback）、`transport-remote-placeholder`（re-export + Mock 测试支持，生产不依赖）；远程契约归 `transport-api` |

## 数据模型

```rust
pub struct GuiClientSession {
    pub client_id: GuiClientId,
    pub connection_id: ConnectionId,
    pub client_name: String,
    pub client_version: Version,
    pub locality: ConnectionLocality,
    pub identity: ActorIdentity,
    pub capabilities: HashSet<GuiCapability>,
    pub connected_at: Timestamp,
    pub last_heartbeat_at: Timestamp,
    pub last_acknowledged_sequence: u64,
    pub subscriptions: HashSet<SubscriptionId>,
}

pub enum ConnectionLocality {
    Local,
    Remote,
    InProcess,
}
```

## Transport 抽象

Transport Server 运行在 CLI 进程内，GUI 使用 Client 连接：

```rust
#[async_trait]
pub trait GuiTransportServer: Send + Sync {
    async fn bind(&self, endpoint: TransportEndpoint) -> Result<Box<dyn GuiListener>, TransportError>;
}

#[async_trait]
pub trait GuiListener: Send + Sync {
    async fn accept(&self) -> Result<Box<dyn GuiConnection>, TransportError>;
    async fn close(&self) -> Result<(), TransportError>;
}

#[async_trait]
pub trait GuiConnection: Send + Sync {
    async fn send(&self, frame: TransportFrame) -> Result<(), TransportError>;
    async fn receive(&self) -> Result<TransportFrame, TransportError>;
    async fn close(&self) -> Result<(), TransportError>;
    fn info(&self) -> ConnectionInfo;
}

#[async_trait]
pub trait GuiTransportClient: Send + Sync {
    async fn connect(&self, endpoint: TransportEndpoint, options: ConnectOptions)
        -> Result<Box<dyn GuiConnection>, TransportError>;
}
```

`TransportFrame` 只拥有一段有界字节；`ClientFrame` / `ServerFrame` 的 JSON 编解码、1 MiB 帧上限与 64 KiB Artifact chunk 上限由 `gui-protocol` 负责。由此 Transport 无需依赖业务协议类型，本地与远程实现只处理连接和字节搬运。

Local Transport：macOS/Linux 使用 Unix Domain Socket，Windows 使用 Named Pipe。Remote Transport 的可替换 Adapter 契约（`RemoteGuiTransportProvider` CLI 端发布 / `RemoteGuiConnector` GUI 端连接）定义在 `transport-api`（单一来源）；`transport-remote` 已实现 TCP + TLS 1.3、端点独立 token 认证与有界续传，但当前仅绑定 127.0.0.1 loopback 临时端口，`remote publish` 长驻至 SIGINT。外部可达地址（NAT 穿透 / relay）、网络切换与底层重连延 P19-14，CLI/Core 与 GUI Protocol 不感知其内部实现；`transport-remote-placeholder` 仅提供 re-export 与 `MockRemoteTransport` 测试支持，不进入生产依赖。

## CLI 必须支持

多个 GUI；本地和远程 GUI 同时在线；单个用户多窗口；单个用户多设备；GUI 心跳；GUI 断线；GUI 重连；事件补发；Snapshot 重建；慢客户端隔离；每个 GUI 独立权限；每个 GUI 独立订阅。单/多实例与退出策略见 [CLI Host](cli-host.md)。

## Phase 15–19 事件、Desktop 投影与 Client Channel 边界

GUI 可订阅 `ServerToolEvent`、ReasoningItem 摘要/引用、Plan/Goal/BackgroundTask/Automation/Monitor/Memory/Review/Hook 等状态事件并在 Snapshot 中恢复其投影，但绝不能请求 Protected Blob 明文、解密句柄或密钥材料。大型 server tool 输出继续只暴露 Artifact 引用。

Codex App Server、Claude Gateway、IDE、Agent SDK / Headless、ACP 与 Mobile 是连接同一 `pawork` Host 的并列 Client Channel：它们共享 `app-service`、Command Router 与 Event Hub 的业务语义，但不复用或隧穿 GUI protocol frame；GUI 也不经这些 adapter 访问 Core。外部 Agent Client 统一实现 [ClientAdapter](client-adapters.md) 契约并持久化 capability/session ownership snapshot；任何 channel 都不能构造第二个 Core。

Phase 19 的 `apps/desktop` 通过 `gui-client` 实现真实 GPUI GUI。Desktop controller 负责本地/远程 Transport、认证与 frame 验证；纯 Rust projection 只保留由 Snapshot/Event 重建的 materialized view 和纯 UI preference。所有业务写入发送带 `command_id`、`expected_revision` 的 `AppCommandEnvelope`，Core 拒绝或重同步必须覆盖 optimistic 展示。

## 优先级

P0（协议冻结）：GUI Connection Protocol 类型与 Transport 抽象在 [Phase 0](../../ROADMAP.md) 冻结。P0 实现：GUI Server、Local Transport、Connection Manager、Subscription Hub、Snapshot/Event Replay、多 GUI 支持、Remote 占位接口在 [Phase 13](../../ROADMAP.md) 落地。真实 GPUI Desktop Client、主交互与三平台发布门禁在 [Phase 19](../../ROADMAP.md) 落地。

## 验收标准

- [ ] 一个 CLI/Core 实例可同时连接至少三个 GUI 测试客户端
- [ ] 本地和远程 GUI 可以同时连接
- [ ] CLI 发起的 Run 会同步到所有 GUI；GUI A 发起的 Run 会同步到 CLI 和 GUI B
- [ ] 任一 GUI 的审批会同步到其他 GUI 和 CLI
- [ ] GUI 断线重连后可恢复完整状态（Event Replay 或 Snapshot 重建）
- [ ] 慢 GUI 不阻塞 Agent 或其他 GUI
- [ ] GUI 不能直接访问 Core 数据库
- [ ] Remote Transport 可通过 Mock 实现完成端到端测试
- [ ] GUI 只能看到 Protected Blob 安全引用；IDE/SDK/ACP/Mobile 与 GUI 协议帧保持隔离

## 相关文档

- [GUI Connection Protocol](../architecture/api-surface.md) · [Agent Client Adapters](client-adapters.md) · [CLI Host](cli-host.md) · [总体架构](../architecture/overview.md)
- [Desktop GUI](desktop-gui.md) · [artifacts](artifacts.md) · [observability](observability.md)
- [ADR-022 GUI 经 CLI 连接](../adr/ADR-022-gui-connects-via-cli.md) · [ADR-023 一 Core 多 GUI](../adr/ADR-023-one-core-many-guis.md) · [ADR-026 GUI 断线安全](../adr/ADR-026-gui-disconnect-safe.md) · [ADR-027 本地远程同协议](../adr/ADR-027-local-remote-same-protocol.md) · [ADR-028 远程可替换](../adr/ADR-028-replaceable-remote-transport.md) · [ADR-029 不点对点同步](../adr/ADR-029-no-peer-gui-sync.md) · [ADR-030 Core 唯一权威](../adr/ADR-030-core-sole-source-of-truth.md)
- [ADR-033 控制面分离](../adr/ADR-033-control-plane-separation.md) · [ADR-035 GPUI Desktop](../adr/ADR-035-gpui-desktop.md) · [ADR-034（已被替代）](../adr/ADR-034-desktop-gui-client-boundary.md) · [ROADMAP Phase 13 / Phase 19](../../ROADMAP.md)
