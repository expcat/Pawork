# P17-11：Real Remote Transport（安全远程发布/连接/重连）

> Phase 17 · Ecosystem & Host Compatibility · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P13-6、P13-4、P6-4

**最终目的**：在 P13-6 占位接口之上落地真实远程 Transport——把「CLI 发布远程端点、GUI 远程连接」从 Mock 升级为带认证、加密、断线重连与撤销的生产实现。它只在 Transport 层工作，搬运与 P13 一致的 GUI Connection Protocol 帧，不含业务逻辑，也不修改 Agent Core（[ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md)/[028](../docs/adr/ADR-028-replaceable-remote-transport.md)）。

**涉及范围**：新增 `transport-remote` crate（真实实现），实现 `transport-remote-placeholder`（P13-6）暴露的 `RemoteGuiTransportProvider`/`RemoteGuiConnector`；复用 `transport-api`、`client-auth`、`auth-service`（P6-4）。不动 `agent-engine` / `gui-protocol` / `app-service`。

## 细分步骤

1. **真实 Provider/Connector 实现** —— 目的：实现 P13-6 占位 trait 的真实版本——`transport-remote` 提供可替换接入点，CLI 端 `RemoteGuiTransportProvider` 发布、GUI 端 `RemoteGuiConnector` 连接，帧仍是 opaque `TransportFrame`；具体穿透/中继方案作为可替换子实现。
2. **认证与身份** —— 目的：远程连接经 `client-auth`/`auth-service`（P6-4）做客户端身份认证（配对码 / token / 设备绑定），握手期校验身份与协议版本，认证失败拒绝并审计；Secret 不落日志（[ADR-014](../docs/adr/ADR-014-secret-os-keychain.md)）。
3. **传输加密** —— 目的：所有远程帧在传输层加密（TLS 1.3 或等价），密钥协商与轮换由 Transport 负责，业务层（GUI 协议）不感知明文差异。
4. **断线重连与对齐** —— 目的：连接中断后按 `global_sequence` 重连续传——可重放则补发缺失事件，否则重新 Snapshot（[ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）；重连退避与上限可配，重连不丢事件顺序。
5. **撤销与端点生命周期** —— 目的：`pawork remote publish/unpublish/revoke` 管理端点与凭证撤销；撤销的凭证立即失效，已建立连接按策略断开并审计。
6. **定向 / Mock 网络测试** —— 目的：用本地 loopback / Mock 对端覆盖「发布→认证→加密握手→收发→断线→重连续传→撤销」全链路，断言帧不混入业务逻辑、重连不丢序、撤销即时生效。仅定向 + Mock 网络测试，不要求 workspace 全量门禁。

## 主要产出物

- `transport-remote` crate：真实 Provider/Connector + 认证 + 加密 + 重连 + 撤销
- `pawork remote publish/unpublish/revoke` 完整命令
- 定向测试（认证/加密/重连续传/撤销全链路）

## 验收标准

- [ ] 真实远程 Transport 实现 P13-6 占位 trait，不修改 `agent-engine` / `gui-protocol` / `app-service`
- [ ] 远程连接经认证（P6-4）与传输加密，Secret 不落日志
- [ ] 断线后按 `global_sequence` 重连续传，不丢事件顺序
- [ ] 凭证可撤销且即时失效，端点生命周期可管
- [ ] Transport 不含业务逻辑，本地与远程仍用同一 GUI Connection Protocol
- [ ] 定向 / Mock 网络测试覆盖认证/加密/重连/撤销全链路

**相关文档**：[gui-connection](../docs/features/gui-connection.md) · [auth](../docs/features/auth.md) · [ADR-014 Secret OS Keychain](../docs/adr/ADR-014-secret-os-keychain.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ADR-027 本地远程同协议](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-028 可替换远程 Transport](../docs/adr/ADR-028-replaceable-remote-transport.md) · [ADR-030 Core 单一事实源](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：TLS 优先评估 `rustls`（无 OpenSSL 依赖，三平台友好）；穿透/中继方案（如 quic `quinn` 或既有 STUN/TURN）按 ROADMAP「依赖选型基线」评估后回填，不在此默认引入整套网络框架。Transport 层统一使用 `transport-api` 的 opaque frame。
