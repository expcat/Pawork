# P13-3：GUI Connection Protocol

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟡未开始 · 依赖：P0-8、P13-1

**最终目的**：冻结并实现 GUI 与 CLI/Core 之间的 GUI Connection Protocol（Command/Query/Event/Snapshot）：握手、ClientFrame/ServerFrame、事件信封、Snapshot（snapshot_sequence）、重连（last_global_sequence）。它是 GUI 接入 Core 的唯一线上契约（[ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md)/[027](../docs/adr/ADR-027-local-remote-same-protocol.md)/[030](../docs/adr/ADR-030-core-sole-source-of-truth.md)）。协议类型在 P0-8 冻结，本任务实现编解码与版本协商。

**涉及范围**：`gui-protocol`、`core-api`

## 细分步骤

1. **帧编解码与握手** —— 目的：稳定线上格式。
2. **Query/Command/Event/Snapshot 映射** —— 目的：覆盖全部交互。
3. **版本协商与结构化错误** —— 目的：可演进、可诊断。
4. **大 payload 走 Artifact ID** —— 目的：事件轻量（[ADR-018](../docs/adr/ADR-018-large-payload-artifact-id.md)）。
5. **测试** —— 目的：编解码与重连可验证。

## 主要产出物

- `gui-protocol` 编解码、握手、版本协商

## 验收标准

- [ ] 本地与远程 GUI 使用同一协议
- [ ] GUI 不能直接访问 Core 数据库
- [ ] 大 payload 仅传 Artifact ID

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [gui-connection](../docs/features/gui-connection.md) · [ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP](../ROADMAP.md)
