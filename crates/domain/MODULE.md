# pawork-domain

canonical 领域类型与跨 crate 契约面。无内部 `pawork-*` 依赖。

## 职责

承载纯数据、协作式取消与冻结契约形状：事件信封 v1、`ModelProvider` / `AgentTool`、ID newtype、降级事件。不执行 IO，不依赖 GUI / SQLite / HTTP / Keychain / Git / 任何具体 Provider（R1 起原 `pawork-api` 的 `provider_api` / `tool_api` 并入本包，纯净红线不变）。

## 模块树

扁平 `src/*.rs`；`lib.rs` 全部 `mod` 私有，再 `pub use` 到 crate 根。

| 文件 | 内容 |
| --- | --- |
| `ids.rs` | 类型安全 ID（`SessionId` / `RunId` / `WorkspaceId` 等）与 `Timestamp` |
| `events.rs` | `AgentEventEnvelope` + `AgentEvent` 32 变体 |
| `provider_api.rs` | `ModelProvider`、`CanonicalModelRequest`、`ProviderStreamEvent`（13 变体） |
| `tool.rs` / `tool_api.rs` | `ToolDescriptor` 与 `AgentTool` 执行面 |
| `provider_hints.rs` | 命名空间 `provider_hints.<provider>.<key>` |
| `message.rs` / `error.rs` / `cancel.rs` | 消息、错误分类、取消令牌 |
| `degrade.rs` / `profile.rs` / `reasoning.rs` | 降级事件、Agent Profile、reasoning 项 |
| `server_tool.rs` / `client_session.rs` / `workflow.rs` | server-tool 归一、客户端会话登记、Plan/Goal/Task 等事件载荷 |

契约字节 golden 在 `tests/`（信封 32 变体、`ProviderStreamEvent` 13 变体、`CanonicalModelRequest` / `ProviderError` / `ToolResult`）。

## 对外入口/API 面

crate 根无 `pub mod`；消费方 `use pawork_domain::…`。要点：

- **事件**：`CURRENT_SCHEMA_VERSION = 1`（磁盘/线上信封版本，**不是** session-store 的 SQLite migration 号）；`AgentEventEnvelope`；`AgentEvent` 含对话/工具/审批/压缩/checkpoint 与 Plan/Goal/Task/Automation/Monitor/Memory/Review/Diagnostic。
- **Provider**：trait `ModelProvider`（`id` / `list_models` / `stream`）；`CanonicalModelRequest`；`ProviderStreamEvent`；`ResolvedCredential`（`Debug` 脱敏，无 `Serialize`）；`ProviderError`。
- **Tool**：trait `AgentTool` / `ToolEventSink`；`ToolExecutionContext`（`workspace_id` + 相对 `working_directory`）；`ToolResult` 仅表示 Core 执行的 ClientFunction。
- **Hints**：`is_provider_hint_key` / `canonical_hint_key`；`LEGACY_HINT_KEY_MAP` 只用于读兼容。
- **降级**：`DegradeEvent` / `DegradeKind`；`code()` 为 `degrade.*` 命名空间。

完整字段与变体以 rustdoc 与 `tests/` golden 为准。

## 依赖与被依赖

- **依赖**：无 `pawork-*`。外部：`serde` / `serde_json` / `thiserror` / `async-trait`；可选 `ts-rs`（feature `typegen`）。feature `plugin = []` 为 F41 复活锚，空数组。
- **被依赖（生产）**：auth、control-plane、engine、git、orchestration、policy、protocol、providers、storage（`session`/`protected` feature）、testkit、tools、workflow、workspace、app、cli、client。
- **不依赖本包**：`pawork-exec`、`pawork-transport`；两个二进制不直连本包。

## 红线与注意事项

- 不得引入 GUI / SQLite / HTTP Client / OS Keychain / Git / 具体 Provider。
- Secret 不得进入事件、`ErrorContext`、`DegradeEvent.details`、日志；`ResolvedCredential::expose_secret()` 仅 adapter 使用。
- Engine 不得按 Provider 名称分支；能力差异走 registry / capability / `provider_hints`。
- 信封 schema v1 与 session DDL 版本相互独立，不得混用。
- Goal/Automation/Monitor 等 **事件类型保留**（重放红线）；对应 reducer 已在 R0 归档，不要当成现行产品面。

## 相关文档

- [docs/design.md](../../docs/design.md) §2 / §3.2（包布局、冻结契约）
- [docs/v2-summary.md](../../docs/v2-summary.md) §4
- [AGENTS.md](../../AGENTS.md) §2
- [代码地图总索引](../../docs/code-map/README.md)
