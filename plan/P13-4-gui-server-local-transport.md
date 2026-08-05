# P13-4：GUI Server 与 Local Transport

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P13-3

**最终目的**：在 CLI 进程内实现 GUI 协议服务器（`gui-server`），接受 GUI 连接、完成握手与认证、提供 Query/Command、广播 Event、提供 Snapshot 与流式终端/Artifact 传输；并通过 `transport-local` 提供 Unix Domain Socket（macOS/Linux）与 Named Pipe（Windows）端点。GUI 经此连接 Core（[ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md)/[027](../docs/adr/ADR-027-local-remote-same-protocol.md)）。

**涉及范围**：`gui-server`、`transport-local`、`transport-memory`、`client-auth`

## 细分步骤

1. **GUI Server 生命周期** —— 接受连接、握手、认证、收发帧、关闭。目的：CLI 内部协议服务器。
2. **Local Transport** —— Unix Socket / Named Pipe 绑定与监听。目的：本地 GUI 接入。
3. **进程内 Transport（测试）** —— `transport-memory`。目的：无需真实 socket 的测试。
4. **客户端认证** —— 目的：身份与权限可控。
5. **Endpoint 发现** —— `pawork gui endpoint`。目的：GUI 可发现连接点。

## 主要产出物

- `gui-server` + `transport-local` + `transport-memory` + `client-auth`

## 验收标准

- [ ] 本地 GUI 可通过 Unix Socket / Named Pipe 连接 CLI
- [ ] 连接需握手与认证
- [ ] 进程内 Transport 可用于测试

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [GUI Connection Protocol](../docs/architecture/api-surface.md) · [ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ROADMAP](../ROADMAP.md)
