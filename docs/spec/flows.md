# 跨包核心链路（flows）

> 四条跨越多包的热路径速览：Agent loop、GUI 连接、事件持久化与重放、凭证与脱敏。进入单包工作前读对应 [包级 Spec](README.md#包级-spec)；本文只回答「一条链路经过哪些包、语义在哪里定死」。冲突以源码为准。

## 1. Agent loop

一次对话轮次如何跑完工具循环。Engine 不落库、不选通道、不按 Provider 名分支。

调用链：

1. CLI `chat` / `run` 或 GUI `run_start` → `AppCore::chat_turn*`（实现在 `crates/app/src/services/run.rs`）。
2. 宿主装配 `SessionLoopCtx`（`crates/app/src/loop_ctx.rs`）实现 `pawork_engine::LoopContext`。
3. `pawork_engine::run_session`（`crates/engine/src/tool_loop.rs`）调 `ModelProvider::stream`，把 `ProviderStreamEvent` 映射为 `AgentEvent`。
4. 收集到 tool call 后：`request_approval`（**等待前**必须 emit `ToolApprovalRequested`）→ `execute_tools` → `ToolScheduler`（`pawork-tools`）→ 各 `AgentTool`。
5. 轮数上限 `DEFAULT_MAX_TOOL_ROUNDS = 20`。压缩走 `LoopContext::compact_history`（host 负责 session fork/snapshot）。

审批：

- 预判：`PolicyEngine::decide`（`pawork-policy`）。`Allow` 不再进 scheduler 的 resolve 钩子。
- `AskUser`：CLI 终端宿主 / GUI `GuiApprovalHost`。`--json` 或非 TTY → `DenyAllApprovals`。
- GUI resume 保留待审批（`resume_messages_keep_pending`）；CLI resume seal `Denied`。
- 文件路径：工具 JSON 相对路径 + `workspace_id` → `policy::resolve_workspace_path`。

不要做的：

- 在 engine 里 `match provider_id`（见 `crates/engine/tests/no_provider_branch.rs`）。
- 让 engine 依赖 tools / exec / storage。
- 把 hosted / extension 工具的结果收成 `ToolResult`（那是 Provider transcript）。

相关包：[engine](crates/engine.md) · [app](crates/app.md) · [tools](crates/tools.md) · [policy](crates/policy.md)

## 2. GUI 连接

Desktop（及 probe）如何连上 Core，而不加载 Core crate。

```text
pawork-desktop  ──framed bytes──►  pawork gui serve
     │                                  │
 pawork-client                    pawork-cli → pawork-app
     │                                  │
 protocol 编解码                    GuiServer + GuiHostAdapter
 transport Local (UDS / pipe)      transport Local
```

- 传输：`pawork-transport` 只搬 `[u32 LE len][payload]`，上限 1 MiB。
- 编解码：`pawork-protocol` 的 `ClientFrame` / `ServerFrame`。
- 鉴权：`gui serve` 写 `gui.token`（`TokenStore`）；desktop `platform.rs` 读同名文件，缺 token fail-closed。
- 命令/查询：三通道可用性来自 `protocol::app::registry`，host 分发表与 `gui.available` 双射。未登记 fail-closed。

Desktop 四层：`ui` → `controller`（只调 `GuiClient`）→ `projection`（无 gpui/tokio）→ `platform`（socket/token）。生产 `pawork-*` 依赖必须恰好 `{pawork-client}`。

断线：`ConnectionManager` 心跳清理连接（host idle 30s；desktop 泵循环约 15s 空闲发 heartbeat），**不**取消进行中的 Run。Resume：`Replay` / `SnapshotRequired` / `UpToDate`（`ResumeDisposition`）。Timeline 投影 reducer 在 `protocol::projection`，host 与 desktop 同源。

Headless / ACP：

- Headless：`pawork headless --json-stdio`，stdout 仅 JSONL；SDK 在 `pawork-client::headless`。
- ACP：`pawork acp serve`；`AcpHost` 不消费 GUI 帧、不持有凭证、不构造第二个 Core。

相关包：[desktop](crates/desktop.md) · [client](crates/client.md) · [transport](crates/transport.md) · [protocol](crates/protocol.md) · [app](crates/app.md) · [cli](crates/cli.md)

## 3. 事件持久化与重放

所有 Agent 事件可落盘、可重放。两套「版本号」不要混用：

| 契约 | 常量 | 位置 |
| --- | --- | --- |
| 事件信封（磁盘/线上 JSON） | `pawork_domain::events::CURRENT_SCHEMA_VERSION = 1` | `crates/domain/src/events.rs` |
| Session SQLite 迁移 | `pawork_storage::session::CURRENT_SCHEMA_VERSION = 12` | `crates/storage/src/session/migration.rs` |

信封 golden：`crates/domain/tests/`（32 变体）。DDL 只追加；v11 是 `command_ledger`（不进 export）；v12（R6）`messages` 整表重建去 `DEFAULT 'main'`、回填即校验，升级 golden 检入 `crates/storage/src/session/fixtures/`。

写入：

1. Engine / host emit `AgentEvent`，包进 `AgentEventEnvelope`（`session_id` + 递增 `sequence`）。
2. `SessionStore` 经 SQLite Actor 串行 append；`opaque_metadata` 走 Secret 扫描与 `provider_hints` 规范化（旧键只读不写）。
3. GUI 侧另有全局 `EventHub` 序列；Lagged 经 hub，禁止 seq-0 旁路。

读取 / 投影：

- 会话恢复：`AppCore::resume_messages*` 重放信封。
- Timeline：`protocol::projection::project_event` → `TimelineProjection`（无 serde，不在线上）。
- 幂等命令：`CommandLedger`（`(tenant, client_scope, command_id)`）；`InFlight` 须有界等待并以 SQLite 为权威。

不要做的：

- 把 Secret、未脱敏 body 写进 envelope / `ErrorContext` / `DegradeEvent.details`。
- 在存储层维护 Provider 键名清单（已改为 hints 命名空间规则）。
- 修改 v1–v10 DDL 形状。

相关包：[domain](crates/domain.md) · [storage](crates/storage.md) · [protocol](crates/protocol.md) · [app](crates/app.md)

## 4. 凭证与脱敏

明文 token 的允许停留点：`SecretBackend` 内部、adapter 瞬时 `expose_secret()`、受保护 AEAD 信封。其它地方必须是引用或 `[REDACTED]`。

解析链：

1. `pawork-auth::locator`：env 名（`api_key_env_name`）、service 前缀 `pawork` / `pawork.mcp.`、文件名 `mcp-auth.json`。
2. `resolve_provider_credential`：`CredentialSource::{AuthFile, EnvFallback, None}`。env 仅 headless/CI fallback。
3. 宿主 `provider_assembly` 把 `ResolvedCredential` 注入通道适配器。`Debug` 脱敏、无 `Serialize`。
4. OAuth（ChatGPT PKCE / xAI Device）参数来自 `CHANNEL_REGISTRY` 的 `OAuthPreset`，不在 adapter 再写一份。client secret 不进仓库。

`FileBackend`：`$PAWORK_HOME/auth.json` 否则 `~/.pawork/auth.json`；`0600` + 原子写；损坏 fail-closed。OS Keychain 已删除。

日志与事件：

- 二进制：`apps/pawork/src/redact.rs` 的 `RedactingFmtLayer` 覆盖全部 tracing 字段。
- Provider HTTP：错误路径不得拷贝 request body 进 `ProviderError.message`。
- Reasoning：事件只带 `ProtectedBlobRef`；明文在 PWB1（`storage` `protected` feature，宿主 `app/protected.rs` 注入）。
- MCP：独立后端文件，禁止与主 auth 混用。

配置：`PaworkConfig` / `ProviderConfig` **无 `api_key` 字段**；`extra` 会剥离该键。compat 导入遇到明文 Secret 拒绝。

相关包：[auth](crates/auth.md) · [providers](crates/providers.md) · [storage](crates/storage.md) · [pawork](crates/pawork.md)
