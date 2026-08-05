# P0-8：Core Command/Event 协议与 GUI 接入协议类型

> Phase 0 · 架构与协议冻结 · 状态：🟡未开始 · 依赖：P0-3

**最终目的**：冻结面向 CLI 与 GUI 的应用 API（Command/Query/Event/Handle），以及 GUI Connection Protocol 与 Transport 抽象的类型。它是 CLI 与 GUI 共享的唯一稳定契约，Rust 类型即 schema source（[ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md)/[017](../docs/adr/ADR-017-gui-no-direct-access.md)/[022](../docs/adr/ADR-022-gui-connects-via-cli.md)），先冻结才能让后续实现有明确对外边界。

**涉及范围**：`core-api`（应用 Command/Event/Query 类型）、`gui-protocol`（GUI 接入协议帧/信封/Snapshot 类型）、`transport-api`（GuiTransportServer/Listener/Connection/Client 抽象 trait）

## 细分步骤

1. **定义 AppCommand / AppQuery 枚举** —— create_session/send_message/cancel/run_tool 等。目的：CLI 与 GUI 对 Core 的全部请求入口。
2. **定义 AppEvent 枚举** —— message_delta/tool_event/run_state/gui.client.* 等。目的：Core 对外的流式与状态广播。
3. **定义统一命令信封与来源** —— `AppCommandEnvelope`（command_id/source/identity/expected_revision/idempotency_key/issued_at）与 `CommandSource`（LocalCli/LocalGui/RemoteGui/Automation/Plugin/Mcp）。目的：所有状态变更可追溯来源与去重。
4. **定义事件信封** —— `AppEventEnvelope`（instance_id/event_id/global_sequence/stream/stream_sequence/timestamp/source/payload）。目的：CLI 与 GUI 看到相同顺序的状态演进。
5. **定义 GUI 接入协议类型** —— 握手、ClientFrame/ServerFrame、Snapshot（含 snapshot_sequence）、重连（last_global_sequence）。目的：GUI 与 CLI 之间的线上契约。
6. **定义 Transport 抽象 trait** —— GuiTransportServer/GuiListener/GuiConnection/GuiTransportClient。目的：本地与远程 GUI 复用同一协议，差异只在传输层。
7. **定义 Handle / API version / request ID** —— 调用方稳定句柄、全局 api_version、每请求 id。目的：可关联请求/响应，支持版本协商。

## 主要产出物

- `core-api`：`AppCommand`/`AppQuery`/`AppEvent` + `AppCommandEnvelope`/`CommandSource` + `AppEventEnvelope`
- `gui-protocol`：握手/帧/Snapshot/重连类型
- `transport-api`：Transport 抽象 trait
- API version 与 request ID 约定

## 验收标准

- [ ] 类型即 schema source（可生成 TS）
- [ ] 覆盖关键命令、查询与事件
- [ ] 命令与事件均带来源、身份、idempotency 与 sequence
- [ ] 含 GUI 接入协议帧与 Transport 抽象 trait
- [ ] 含 request id 与 API version

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [GUI 连接与多客户端](../docs/features/gui-connection.md) · [ADR-006](../docs/adr/ADR-006-tauri-via-app-service.md) · [ADR-017](../docs/adr/ADR-017-gui-no-direct-access.md) · [ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md) · [ADR-024](../docs/adr/ADR-024-shared-app-service-event-hub.md) · [ROADMAP](../ROADMAP.md)
