# ADR-028：Remote Transport 通过可替换 Adapter 接入

- **状态**：Accepted
- **日期**：2026-08-05

## 背景

远程连接涉及内网穿透、NAT、中继、P2P、加密等大量实现细节，且具体方案可能随时间替换。

## 决策

远程连接能力由 CLI 的 Transport Adapter（`RemoteGuiTransportProvider` / `RemoteGuiConnector`）提供，作为可替换接口接入。MVP 仅提供占位接口与 Mock 实现，完成端到端测试；真实内网穿透库后续接入，不修改 Agent Core。

## 后果

- Remote Transport 可替换而不修改 Agent Core 与 GUI Protocol。
- Transport 不包含 Agent 业务逻辑。

## 相关

- [gui-connection](../features/gui-connection.md) · [workspace-layout](../architecture/workspace-layout.md) · [ADR-027 本地远程同协议](ADR-027-local-remote-same-protocol.md)
