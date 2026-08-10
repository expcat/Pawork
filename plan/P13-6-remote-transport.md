# P13-6：Remote Transport 占位与可替换 Adapter

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P13-4
> **边界与扩展**：本任务交付 Remote Transport 的**可替换 Adapter 占位 trait + Mock 实现 + `pawork remote publish/unpublish` + 端到端测试**，不含真实内网穿透库。**真实 Remote Transport**（安全远程发布/连接/重连、真实内网穿透）**不在本任务范围**，由 [P17-11](P17-11-real-remote-transport.md) 独立承接；替换实现不改 Agent Core 或 GUI Protocol（[ADR-028](../docs/adr/ADR-028-replaceable-remote-transport.md)）。勿把占位/Mock 完成误判为远程穿透可用。

**最终目的**：提供远程 GUI 连接的可替换 Adapter 占位接口，使本地与远程 GUI 复用同一 GUI Connection Protocol，差异只在传输层。MVP 仅提供 `RemoteGuiTransportProvider`（CLI 端发布）与 `RemoteGuiConnector`（GUI 端连接）占位 + Mock 实现，完成端到端测试；真实内网穿透库后续接入，不修改 Agent Core（[ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md)/[028](../docs/adr/ADR-028-replaceable-remote-transport.md)）。

**涉及范围**：`transport-remote-placeholder`、`gui-server`、`gui-client`

## 细分步骤

1. **Provider / Connector 占位 trait** —— 目的：可替换远程接入点。
2. **Mock 实现** —— 目的：端到端可测。
3. **`pawork remote publish/unpublish`** —— 目的：远程端点生命周期可管。
4. **Transport 不含业务逻辑校验** —— 目的：边界清晰。
5. **端到端测试** —— 目的：远程 Mock 链路可用。

## 主要产出物

- `transport-remote-placeholder` 占位接口 + Mock + remote 命令

## 验收标准

- [x] Remote Transport 可通过 Mock 实现完成端到端测试
- [x] 本地与远程 GUI 使用同一协议
- [x] 替换远程实现不需要修改 Agent Core 或 GUI Protocol

## 实现记录（2026-08-10）

- `transport-remote-placeholder`（新建）：`RemoteGuiTransportProvider` trait
  （`describe` / `publish(RemotePublishRequest) -> RemotePublishHandle` /
  `unpublish(handle_id)`）、`RemoteGuiConnector` trait
  （`connect(endpoint, options) -> Box<dyn GuiConnection>`）；`RemotePublishHandle`
  携带 id 与 `TransportEndpoint::Remote` 端点。`MockRemoteTransport` 以内存
  channel 对实现 transport-api 的 `GuiTransportServer` / `GuiTransportClient` /
  `GuiConnection`（只接受 `TransportEndpoint::Remote`，locality `Remote`，帧大小
  按 `max_frame_bytes` 校验）；`MockRemoteTransportProvider` publish 预占地址槽位、
  unpublish 移除槽位（之后 connect 失败），`MockRemoteConnector` 转发连接并拒绝
  非 `mock` adapter。Transport 只搬运有界字节帧，无 Agent 业务逻辑。
- CLI 接线（`cli-command` / `cli-host`）：`pawork remote publish [--name]` 调用
  provider publish，输出 endpoint 与状态（JSON 模式含 `handle_id` / `endpoint`）；
  `pawork remote unpublish --handle <id>` 调用 provider unpublish；未装配 provider
  时返回结构化错误（`ok=false`，kind `remote`）。
- 端到端测试：mock provider publish → connector connect → gui-server 完整握手 →
  command 往返 → 宿主侧 SessionHandle 推送 `ServerFrame::Event` 被 GUI 端收到 →
  query 往返仍可用；与本地端到端测试使用完全相同的协议帧（ADR-027/028）。
- 定向验证：`cargo test -p transport-remote-placeholder -p cli-command -p cli-host`
  全绿（placeholder 9 项单元 + 1 项端到端；cli-command 4 项含 remote 解析；
  cli-host 9 项含 remote publish/unpublish 与无 provider 结构化错误）；
  `cargo fmt --check` 与相关包 `cargo clippy --all-targets -- -D warnings` 通过。
- 说明：真实内网穿透不在本任务范围（P17-11）；替换实现仅需实现
  Provider/Connector trait，不修改 Agent Core 与 GUI Protocol。

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-028](../docs/adr/ADR-028-replaceable-remote-transport.md) · [ROADMAP](../ROADMAP.md)
