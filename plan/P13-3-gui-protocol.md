# P13-3：GUI Connection Protocol

> Phase 13 · CLI Host 与多 GUI 协议 · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-8、P13-1

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

- [x] 本地与远程 GUI 使用同一协议
- [x] GUI 不能直接访问 Core 数据库
- [x] 大 payload 仅传 Artifact ID

## 实现记录（2026-08-10）

- 运行时完整化：`gui-protocol` 拆为 `codec` / `handshake` / `resume` / `snapshot` /
  `error` 模块；握手服务端逻辑（`HandshakeService` + `ClientAuthenticator` trait
  注入 + capabilities 筛选 + 协商接入）、Snapshot 校验（data/artifact_id 互斥且
  data 有界）、`compute_resume_disposition`（Replay/SnapshotRequired/UpToDate）、
  u32 LE 长度前缀分帧读写 API（`write_frame`/`read_frame`/`write_*_frame`/
  `read_*_frame`/`encode_length_prefixed`/`decode_length_prefixed`）落地；
  `transport-api` 不依赖 `gui-protocol`，只搬运字节。
- 版本校验：信封 api_version 与协商结果经 `ensure_compatible_api_version` /
  `validate_*_frame_api_version` / `decode_*_frame_checked` 校验，不兼容产生
  `IncompatibleVersion` 线上错误；协议文档见 [ADR-036](../docs/adr/ADR-036-gui-protocol-versioning.md)。
- 定向验证：`cargo test -p gui-protocol -p core-api -p schema-typegen`（gui-protocol
  48 个测试：全帧型 round trip / 分帧边界 / 协商空列表·全不兼容·major 不同 /
  encode 侧帧上限 / golden JSON 7 个 fixture）；`cargo run -p schema-typegen -- --check`
  通过；`cargo fmt` 与 `cargo clippy -p gui-protocol -p core-api -p schema-typegen
  --all-targets -- -D warnings` 通过。

### Deferred items（建议/跟踪，本任务不做）

- gui-server 运行时（P13-4 起）按协商 minor 裁剪行为并接入 `HandshakeService`；
  本任务只提供协议侧服务端逻辑与校验路径。
- 心跳/慢客户端隔离、Snapshot 生成与事件重放的持久化接线随 P13-5 落地。

**相关文档**：[GUI Connection Protocol](../docs/architecture/api-surface.md) · [gui-connection](../docs/features/gui-connection.md) · [ADR-022](../docs/adr/ADR-022-gui-connects-via-cli.md) · [ADR-027](../docs/adr/ADR-027-local-remote-same-protocol.md) · [ADR-030](../docs/adr/ADR-030-core-sole-source-of-truth.md) · [ROADMAP](../ROADMAP.md)
