# 稳定契约规格

> 基线日期：2026-08-25。本文是契约目录和演进规则；精确字节形状以源码、检入 schema 与 golden 为准，[docs/architecture.md](../architecture.md) §3.2 是冻结契约事实源。

## 1. 契约原则

1. 磁盘、wire、JSON、公开安全枚举和可重放事件不得静默破坏。
2. 兼容演进优先采用可选字段、默认值、只读 alias 或新版本；删除/重解释已有字段需要 ADR。
3. golden/升级 fixture 必须先于生产实现变化，消费者与生产者在同一任务内对齐。
4. 未登记命令、未知 capability、未知凭证或损坏持久化数据必须 fail-closed。
5. 本 Spec 不复制完整 serde/DDL；需要逐字段判断时直接读对应源码、schema 和 golden。

## 2. 契约目录

| ID | 契约 | 当前版本/形状锚 | 生产者 → 消费者 | 精确事实源 |
| --- | --- | --- | --- | --- |
| CON-PROVIDER-01 | Canonical Provider | `ModelProvider`、`CanonicalModelRequest`、`ProviderStreamEvent`、`ModelResponseSummary`、`ResolvedCredential`、`ProviderError` | providers adapter → engine/app | [domain provider API](../../crates/domain/src/provider_api.rs)；[architecture §3.2](../architecture.md#32-冻结契约激活即采用完整形状golden-先于实现改动) |
| CON-EVENT-01 | Agent 事件信封 | `AgentEventEnvelope.schema_version = 1`；append-only、全局 sequence、parent link | engine/app → storage/protocol/projection | [domain events](../../crates/domain/src/events.rs)；事件 golden；[storage](../../crates/storage/src/session) |
| CON-STORAGE-01 | Session SQLite | `CURRENT_SCHEMA_VERSION = 14`；v11 增 `command_ledger`，v12 原生 branch lineage，v13 持久化 Session→Workspace 归属，v14 持久项目注册表 `workspaces` | app/storage → resume/fork/compact/import | [migration](../../crates/storage/src/session/migration.rs)；[升级 fixtures](../../crates/storage/src/session/fixtures) |
| CON-EXPORT-01 | Session 导出 | `EXPORT_SCHEMA_VERSION = 3` | `sessions export` → import/外部备份 | [session import/export](../../crates/storage/src/session/import) |
| CON-BLOB-01 | Artifact/Protected Blob | `PWB1_MAGIC`，`PWB1_VERSION = 1`；protected 使用 AEAD | checkpoint/reasoning → artifact/protected stores | [blob](../../crates/storage/src/blob)；[PWB1 golden](../../crates/storage/tests/golden) |
| CON-POLICY-01 | Policy 决策 | `PolicyDecision` 四变体；`ApprovalMode` 五档，默认 `ReadOnly` | tools/app → CLI/Desktop/exec | [policy](../../crates/policy/src)；[security.md](security.md) |
| CON-CONFIG-01 | 配置 schema/层级 | `Builtin < Global < Profile < Workspace < Session < Run`；`ProviderConfig` 无 `api_key` | workspace loader → app/providers | [workspace config](../../crates/workspace/src/config) |
| CON-GUI-01 | GUI Connection Protocol | API `1.9`；支持 `1.0/1.1/1.2/1.3/1.4/1.5/1.6/1.7/1.8/1.9`；Accepted 握手可选 `host_data_dir`；`ClientFrame`/`ServerFrame`；上限 1 MiB | app GUI host ↔ client/Desktop | [protocol](../../crates/protocol/src)；[schemas/gui-protocol](../../schemas/gui-protocol)；protocol fixtures/golden |
| CON-REGISTRY-01 | Command/Capability Registry | 28 `AppCommand`、15 `AppQuery`；GUI/headless/ACP 可用性同源 | protocol registry → app/cli/client | [registry](../../crates/protocol/src/app/registry.rs) |
| CON-HEADLESS-01 | Headless JSON | 与 GUI 帧正交的 request/response JSONL；stdout-only | CLI stdio ↔ SDK/automation | [headless protocol](../../crates/protocol/src/headless)；[schemas/headless-json](../../schemas/headless-json) |
| CON-ACP-01 | ACP 映射 | ACP adapter 只接 registry 允许的能力，未登记拒绝 | IDE/ACP client ↔ CLI/AppCore | [CLI ACP](../../crates/cli/src/channels/acp)；ACP fixtures |
| CON-USAGE-01 | Usage 与审计 | usage `dedup_key`；audit 为 JSONL | app/control-plane → usage ledger/audit | [control-plane](../../crates/control-plane/src)；对应 golden |
| CON-AUTH-01 | 本机 Secret 文件 | auth format v1；`0600`、原子 rename、损坏 fail-closed；MCP 独立文件/前缀 | CLI/app/tools ↔ auth backend | [auth backend](../../crates/auth/src) |
| CON-TRANSPORT-01 | 本机字节传输 | `[u32 LE payload_len][payload]`，上限 1 MiB | transport ↔ protocol codec | [transport](../../crates/transport/src)；[protocol codec](../../crates/protocol/src/codec.rs) |

## 3. 架构契约

下列约束虽然不都是 wire 格式，但同样属于不可静默放宽的产品契约：

| ID | 约束 | 守护方式 |
| --- | --- | --- |
| ARC-01 | `pawork` 是 Core 唯一正式宿主；CLI/Core 同进程同二进制。 | workspace 依赖图、apps/pawork composition root。 |
| ARC-02 | Desktop 独立进程，生产 `pawork-*` 依赖只能是 `pawork-client`。 | Desktop deny-list 断言。 |
| ARC-03 | `pawork-domain` 不依赖 GUI、SQLite、HTTP、OS Keychain、Git 或具体 Provider。 | Cargo 依赖/源码守护。 |
| ARC-04 | Engine 的唯一生产 `pawork-*` 依赖是 domain，禁止 Provider 名称特例。 | engine domain-only / no-provider-branch 测试。 |
| ARC-05 | 所有 Agent 事件可持久化、可重放；GUI 投影不自造第二份业务状态。 | event golden、storage replay、共享 projection reducer。 |
| ARC-06 | Secret 不进数据库、事件、日志和可提交配置。 | Secret 扫描、脱敏回归、auth/config 分域。 |
| ARC-07 | GUI 不直连 Provider、数据库、工具、Git 或 PTY。 | 依赖 deny-list + host registry。 |

## 4. 兼容性要求

### 4.1 事件与存储

- 事件只能 append；已经发布的 envelope 字段含义不得重解释。
- SQLite 迁移按版本追加。v1–v12 不回写；v12 的重建/回填失败必须整批 fail-closed；v13 只追加可空归属列，不回填历史会话；v14 只建空 `workspaces` 注册表（stable id + `root_path` UNIQUE），不据历史归属猜 root。
- branch 可见性由 storage lineage 单点决定；父支晚写/晚压缩不得污染旧 fork，兄弟分支互不可见。
- `command_ledger` 保证相同 command 重试可 replay，InFlight/record 失败不得造成永久挂死或重复副作用。
- export/import 必须显式声明版本；未知/损坏/含 Secret 输入不得静默生成残缺可信会话。

### 4.2 GUI、headless 与 ACP

- 握手协商必须落在双方支持版本交集；不支持的版本显式拒绝。
- API 1.9 的 `HandshakeResponse::Accepted.host_data_dir` 是认证成功后由 Host 可选声明的只读本机元数据；缺失保持向后可解码，消费者不得从 endpoint 推断替代值，也不得把它升级为文件操作输入。
- capability 的“宣告 = 授权 = 实现”由 registry 派生；禁止在三个通道维护平行名字表。
- request-scoped error 只能路由给匹配 request；连接级 error 不得被事件泵吞掉。
- `ArtifactStreaming` 枚举可保留，但生产宿主当前不得宣告。
- `WorkspaceRelativePath` 拒绝绝对路径与 `..`；客户端不因 UI 便利绕过 host Policy。

### 4.3 配置与凭证

- 六层配置后层覆盖前层；新增层级或改变优先级属于契约变化。
- `PaworkConfig`/`ProviderConfig` 不允许 `api_key`；Secret 只进入 auth backend 或受控 env fallback。
- auth 文件读旧 alias 可以有期限；写出必须使用当前词汇。兼容期结束需单独任务和回归。
- MCP Secret 使用 `mcp-auth.json` 与 `pawork.mcp.*`，不得混入主 Provider auth 域。

## 5. 契约变更流程

1. 在任务书列出受影响的生产者、消费者、版本、旧数据和回滚路径。
2. 若改变已有含义、架构红线或不兼容形状，先起草 ADR 并等待用户 Accepted。
3. 先提交/更新 golden、升级 fixture 或 schema diff，让预期变化显式失败。
4. 实现生产者和全部当前消费者；旧版本读写策略必须明确。
5. 运行该契约的定向测试；真实客户端/旧库升级/人工 UI 证据按风险补齐。
6. 同批更新 `docs/architecture.md`、本文件、对应包级 Spec（`docs/spec/crates/`）、ROADMAP/任务书和生成 schema。

当前 Settings 活动线不授权借 UI 实现静默演进 schema/wire。GUI auth status、非重放 Secret 写入、OAuth 进度与默认项 mutation 已经 [ADR-046](../adr/ADR-046-settings-auth-wire-and-secret-transit.md)（Accepted）拍板，按 D1–D6 实施；此外的新演进仍按本节流程与 ADR 闸门单独处理。

## 6. headless `--json` 与正式 headless 协议映射

本节是 CON-HEADLESS-01 的展开。单向 `--json`（`pawork run --json` / `pawork chat --prompt --json`）与双向 `pawork headless --json-stdio` 共用同一套 `HeadlessResponse` 词表；差别只在 `--json` 无 stdin 命令、无 `hello` 握手。机器可读对照表为 [`headless::json_mapping::JSON_TO_HEADLESS_EVENT_MAP`](../../crates/protocol/src/headless/json_mapping.rs)；TypeScript 检入物见 [schemas/headless-json](../../schemas/headless-json) 与 [schemas/core-api](../../schemas/core-api)。历史迁移过程（S1 unstable 裸信封 → S10 收口切换）不在本节，属 V2 归档史。

### 6.1 三层信封不得混用

| 层 | 类型 | 说明 |
| --- | --- | --- |
| 磁盘/重放 | `AgentEventEnvelope`（`schema_version = 1`，session 内 `sequence`） | CON-EVENT-01；不出现在 headless 输出 |
| 应用/多订阅者 | `AppEventEnvelope`（`api_version` + `global_sequence` + `stream`） | GUI 帧与 headless `event` 共用同一信封 |
| 传输包装 | GUI 为 `ClientFrame`/`ServerFrame`（CON-GUI-01/CON-TRANSPORT-01）；headless 为 `HeadlessRequest`/`HeadlessResponse`（JSONL） | Desktop 与 SDK/脚本各走一层；GUI/Desktop 不得 fallback 到 `--json` |

### 6.2 通道行为

| 通道 | 帧 | 关键语义 |
| --- | --- | --- |
| 单向 `--json` | stdout 每行一个 `HeadlessResponse`，顶层 `type` ∈ `event`/`response`/`error`；无 hello | stdout 只承载 JSONL，文本/日志/进度走 stderr；审批不可交互，fail-closed（`DenyAllApprovals`） |
| `headless --json-stdio` | 请求 `hello`/`command`/`query`/`compat_import`/`compat_history`；响应 `hello_ack`/`response`/`event`/`compat_*_result`/`error`；`command`/`query`/`event` 直接嵌 `App*Envelope`，不另造 RPC 名空间 | `hello_ack` 之前的 `command`/`query` → `not_handshaked`；未知 `type` → `unknown_request_type`；单帧上限 4 MiB（`MAX_FRAME_BYTES`）；未映射到 capability 的命令 fail-closed（`UnsupportedCapability`，S13-F33 拍板） |
| 其它子命令 `--json`（`sessions`/`auth`/`models`/`diff`/`mcp`/`import` 等） | 各自快照 JSON | 不是协议帧，也不是 headless `query` 响应 |

### 6.3 信封字段与事件 tag 对照

`event.envelope` 相对磁盘信封：`schema_version` 不出现；`sequence` 换为 `stream_sequence` + `global_sequence`（线上排序以 `global_sequence` 为准）；`session_id`/`run_id` 收进 `stream: {type, id}`；`parent_event_id` 不上线；新增必填 `api_version`/`instance_id`/`source`；`payload` 从细粒度 `AgentEvent` 换为更粗的 `AppEvent`。

| 磁盘 `payload.type` | headless `envelope.payload.type` |
| --- | --- |
| `run_started` / `run_completed` / `run_cancelled` / `run_failed` | `run_changed`（终态细节在 `state`） |
| `assistant_text_delta` / `assistant_thinking_delta` | `assistant_delta` / `thinking_delta` |
| `tool_call_started` / `tool_output_delta` / `tool_execution_completed` | `tool_started` / `tool_output` / `tool_completed` |
| `tool_approval_requested` | `tool_approval_required`（远程决议走 `AppCommand::ToolApprove`） |
| `diagnostic` | `diagnostic`（字段形状为 `{level, code, message}`） |
| 其余磁盘事件（`context_prepared`、`usage_updated`、`tool_call_arguments_delta`、`tool_approval_responded`、`tool_execution_started`、`message_committed`、`provider_*`、`compaction_*`/`checkpoint_*`、workflow 域事件 `plan`/`goal`/`task`/`automation`/`monitor`/`memory`/`review` 等） | 无 AppEvent 镜像，不出现在 headless 事件流（仍在磁盘，可重放） |

仅存在于 AppEvent 侧的事件（`core_ready`、`workspace_changed`、`session_changed`、`gui_client_*` 等）无磁盘细事件来源，原样下发。消费纪律：按 `\n` 切行、先读顶层 `type`，未知 type 当协议错误而非 domain envelope。协议版本与 crate 版本解耦：版本对照以 CON-GUI-01 与 `PROTOCOL_CRATE_COMPATIBILITY` 为准，不进入握手 JSON。
