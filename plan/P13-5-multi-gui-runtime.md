# P13-5：多 GUI 运行时（连接管理 / 订阅 / 快照重放 / 慢客户端隔离）

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P13-4

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

- [ ] 一个 CLI/Core 实例可同时连接至少三个 GUI 测试客户端
- [ ] CLI 发起的 Run 同步到所有 GUI；GUI A 发起的 Run 同步到 CLI 和 GUI B
- [ ] 任一 GUI 的审批同步到其他 GUI 和 CLI
- [ ] GUI 断线重连后可恢复完整状态
- [ ] 慢 GUI 不阻塞 Agent 或其他 GUI

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-023](../docs/adr/ADR-023-one-core-many-guis.md) · [ADR-026](../docs/adr/ADR-026-gui-disconnect-safe.md) · [ADR-029](../docs/adr/ADR-029-no-peer-gui-sync.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP](../ROADMAP.md)
