# 事件持久化与重放

所有 Agent 事件可落盘、可重放。两套「版本号」不要混用。

## 两套版本

| 契约 | 常量 | 位置 |
| --- | --- | --- |
| 事件信封（磁盘/线上 JSON） | `pawork_domain::events::CURRENT_SCHEMA_VERSION = 1` | `crates/domain/src/events.rs` |
| Session SQLite 迁移 | `pawork_storage::session::CURRENT_SCHEMA_VERSION = 11` | `crates/storage/src/session/migration.rs` |

信封 golden：`crates/domain/tests/`（32 变体）。DDL 只追加；v11 是 `command_ledger`（不进 export）。R6 预期 v12 做原生 branch lineage。

## 写入

1. Engine / host emit `AgentEvent`，包进 `AgentEventEnvelope`（`session_id` + 递增 `sequence`）。
2. `SessionStore` 经 SQLite Actor 串行 append；`opaque_metadata` 走 Secret 扫描与 `provider_hints` 规范化（旧键只读不写）。
3. GUI 侧另有全局 `EventHub` 序列；Lagged 经 hub，禁止 seq-0 旁路。

## 读取 / 投影

- 会话恢复：`AppCore::resume_messages*` 重放信封。
- Timeline：`protocol::projection::project_event` → `TimelineProjection`（无 serde，不在线上）。
- 幂等命令：`CommandLedger`（`(tenant, client_scope, command_id)`）；`InFlight` 须有界等待并以 SQLite 为权威。

## 不要做的

- 把 Secret、未脱敏 body 写进 envelope / `ErrorContext` / `DegradeEvent.details`。
- 在存储层维护 Provider 键名清单（已改为 hints 命名空间规则）。
- 修改 v1–v10 DDL 形状。

模块图：[domain](../../../crates/domain/MODULE.md) · [storage](../../../crates/storage/MODULE.md) · [protocol](../../../crates/protocol/MODULE.md) · [app](../../../crates/app/MODULE.md)
