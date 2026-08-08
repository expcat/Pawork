# P13-6：Remote Transport 占位与可替换 Adapter

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P13-4
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

- [ ] Remote Transport 可通过 Mock 实现完成端到端测试
- [ ] 本地与远程 GUI 使用同一协议
- [ ] 替换远程实现不需要修改 Agent Core 或 GUI Protocol

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-028](../docs/adr/ADR-028-replaceable-remote-transport.md) · [ROADMAP](../ROADMAP.md)
