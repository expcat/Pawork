# P13-5：多 GUI 运行时（连接管理 / 订阅 / 快照重放 / 慢客户端隔离）

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P13-4

**最终目的**：让一个 CLI/Core 实例同时服务多个本地与远程 GUI。落地 Connection Manager（心跳、断线、重连、每 GUI 独立权限与订阅）、Subscription Hub（按相同顺序广播 Core Event）、Snapshot Service 与 Event Replay（断线不影响 Run，重连可恢复）、慢客户端隔离（[ADR-023](../docs/adr/ADR-023-one-core-many-guis.md)/[026](../docs/adr/ADR-026-gui-disconnect-safe.md)/[029](../docs/adr/ADR-029-no-peer-gui-sync.md)/[030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。

**涉及范围**：`connection-manager`、`subscription-hub`、`snapshot-service`

## 细分步骤

1. **Connection Manager** —— `GuiClientSession`/`ConnectionLocality`、心跳、断线、重连、每 GUI 权限与订阅。目的：多 GUI 在线管理。
2. **Subscription Hub** —— Core Event 扇出到 CLI 与所有 GUI，相同顺序、bounded channel、背压。目的：一致广播。
3. **Snapshot Service + Event Replay** —— 首连给 Snapshot + snapshot_sequence；重连按 last_global_sequence 补发或重建。目的：重连恢复。
4. **慢客户端隔离** —— 慢 GUI 不阻塞 Core 或其他 GUI。目的：稳定性。
5. **测试** —— 多 GUI 在线、重连、慢客户端场景。目的：可复核。

## 主要产出物

- `connection-manager` + `subscription-hub` + `snapshot-service` + 慢客户端隔离

## 验收标准

- [x] 一个 CLI/Core 实例可同时连接至少三个 GUI 测试客户端
- [x] CLI 发起的 Run 同步到所有 GUI；GUI A 发起的 Run 同步到 CLI 和 GUI B
- [x] 任一 GUI 的审批同步到其他 GUI 和 CLI
- [x] GUI 断线重连后可恢复完整状态
- [x] 慢 GUI 不阻塞 Agent 或其他 GUI

## 实现记录（2026-08-10）

- `connection-manager`：`ConnectionManager` 登记多 GUI 连接（`GuiClientSession`：
  client_id / connection_id / name / version / locality / identity / capabilities /
  connected_at / last_heartbeat_at / last_ack / subscriptions），`register` 返回每连接
  有界事件队列接收端；`heartbeat` / `ack`（单调保留）/ `subscribe`（同 id 替换）/
  `unsubscribe`（幂等）/ `should_forward`（空 streams = 全量）；`enqueue` 用
  `try_send` 非阻塞投递，队列满标记 `lagged` 并返回 `ManagerError::Lagged`，不阻塞
  发布者与其他 GUI；心跳超时判定与断线清理不取消 Run（[ADR-026]）。
- `snapshot-service`：`SnapshotService::new(Arc<AppService>, Arc<EventHub>)`，
  `build()` 以 `hub.current()` 为 `snapshot_sequence`，由聚合快照生成六个 section
  （Workspaces / SessionTree / ActiveRuns / PendingToolApprovals / TerminalSessions /
  ProviderStatus，SessionTree 按 forked_from 建树，ActiveRuns 过滤终态，approvals
  仅 Pending），section 大小超限返回 `SnapshotError::SectionTooLarge`。
- `gui-server` 接线：握手后注册连接并发首帧 Snapshot；事件经 Hub 订阅 → 每连接
  forwarder（`should_forward` 过滤）→ 有界队列 → 帧循环逐帧发送 `ServerFrame::Event`；
  Subscribe / Unsubscribe 登记订阅；Resume 按 `compute_resume_disposition` 判定：
  Replay 走 `hub.replay(from, through)` 补发，窗口不可用降级
  `SnapshotRequired` + Snapshot，UpToDate 仅回响应；SnapshotRequest 即时构建；
  Ack 记录 `last_ack`（重连握手 resume 上下文用）；任意入站帧刷新心跳，心跳超时
  断线清理但绝不取消 Run。
- 验收测试（`crates/gui-server/tests/multi_gui_runtime.rs`，transport-memory）：
  3 GUI 并发（connection count=3）；CLI Run → 3 GUI 全收 Completed；GUI A Run →
  CLI Hub 观察者 + GUI B 全收；GUI B 审批 → GUI A/C + CLI 收到后续 RunChanged 且
  聚合审批转 Decided；断线重连 → 初始 Snapshot 含 ActiveRuns 当前状态 + Resume 按
  last_global_sequence Replay 补发（含断线期间的取消事件），ring 淘汰时降级
  SnapshotRequired + Snapshot；慢客户端（服务端 send 阻塞 + 事件灌入）标记 Lagged、
  Run 照常完成、快客户端照常收全事件、断线后注销。
- 定向验证：`cargo test -p connection-manager -p snapshot-service -p gui-server` 全绿
  （connection-manager 6 项；snapshot-service 3 项；gui-server 9 项单元 + 6 项集成）；
  `cargo fmt --all -- --check` 与 `cargo clippy -p connection-manager -p snapshot-service
  -p gui-server --all-targets -- -D warnings` 通过。

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-023](../docs/adr/ADR-023-one-core-many-guis.md) · [ADR-026](../docs/adr/ADR-026-gui-disconnect-safe.md) · [ADR-029](../docs/adr/ADR-029-no-peer-gui-sync.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP](../ROADMAP.md)
