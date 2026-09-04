# pawork-protocol

> CLI/Core 宿主与所有客户端之间的协议层：core-api 信封 + GUI Connection Protocol 帧 + headless-json（NDJSON）+ 外部客户端 adapter + 共享 timeline 投影 + TS typegen；只依赖 `pawork-domain`，不含任何业务执行逻辑。

## 1. 职责与边界

- 定义三条客户端通道共用的 **core-api** 词汇：`AppCommandEnvelope`（28 个 `AppCommand`）、`AppQueryEnvelope`（15 个 `AppQuery`）、`AppResponseEnvelope`、`AppEventEnvelope`（22 个 `AppEvent`），及配套 registry（wire 名 ↔ 变体双射、GUI / headless / ACP 三通道开关、幂等性、`since` 版本）。
- 定义 **GUI Connection Protocol** wire：`ClientFrame` 11 变体 / `ServerFrame` 10 变体、长度前缀 codec、握手与版本协商、resume 三态、snapshot、artifact 分块、token 认证。
- 定义 **headless-json**（stdin/stdout NDJSON）wire、翻译层与 `run_loop`（含背压 fail-closed），供 SDK / CI 脚本接入。
- 定义 **外部 Agent 客户端 adapter 契约**（`ClientAdapter` trait、canonical 帧、`SessionRegistry` 会话登记、外部身份与租户绑定）。
- 提供 **projection**：把持久化 `AgentEventEnvelope` / 广播 `AppEventEnvelope` 归一为 `TimelineItem` / `TimelineEntry` 的共享 reducer，desktop 与 CLI 历史臂共用（CR08-08 两端一致性的根治）。
- 提供 **typegen**：把上述类型导出为 TypeScript 声明并检入仓库根 `schemas/`，测试强制与源码同步。
- **不做**：传输 IO 之外的执行逻辑（无 Provider、无数据库、无工具）；GUI 渲染；事件如何产生。协议的服务端实现在 `pawork-app` / `pawork-cli`，GUI 客户端实现在 `pawork-client`。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~320 | `pub mod` 拓扑与 feature 门（`adapter` / `client-auth` / `typegen` 条件编译）；GUI wire 顶层类型：`ClientFrame`（11）/ `ServerFrame`（10）、`HandshakeRequest` / `HandshakeResponse`（Accepted 自 API 1.9 起可选 `host_data_dir`，ADR-051）、`GuiCapability` 5 变体、`ClientAuthentication`（Debug 脱敏）、`SubscribeRequest` / `ResumeRequest` / `ResumeResponse` / `ResumeDisposition` 3 态、`Snapshot` / `SnapshotSection` / `SnapshotSectionKind` 6 节、`ArtifactReadRequest` / `ArtifactChunk`、`ProtocolErrorEnvelope` / `ProtocolError` / `ProtocolErrorCode` 10 变体（1.4 增 `Busy` / `ValidationFailed`，ADR-046）；常量 `MAX_PROTOCOL_FRAME_BYTES = 1 MiB`、`MAX_ARTIFACT_CHUNK_BYTES = 64 KiB`、`MAX_SNAPSHOT_SECTION_DATA_BYTES = 256 KiB` |
| `src/codec.rs` | ~220 | 有界 JSON 编解码 + `u32 LE` 长度前缀分帧（`FRAME_LENGTH_PREFIX_BYTES = 4`）；`encode/decode_{client,server}_frame`、`encode/decode_length_prefixed`；同步 `read_frame` / `write_frame`（及 client/server 变体）与 tokio 异步 `read_frame_async` / `write_frame_async`；`ProtocolCodecError`（FrameTooLarge / DeclaredLengthTooLarge / UnexpectedEof / Serialize / Deserialize / Io / SnapshotSection 校验类） |
| `src/error.rs` | ~70 | `ProtocolError` 逐 code 便捷构造器与 `From<ProtocolCodecError>` 映射（编解码错误 → invalid_frame / frame_too_large / internal） |
| `src/handshake.rs` | ~260 | `negotiate_api_version(_with)`（同 major、取共同 minor 最大）；`ClientAuthenticator` trait + `HandshakeService::accept`（协商 → 认证 → 能力交集 → resume disposition）+ `HandshakeService::with_host_data_dir`（宿主注入认证后可选只读元数据）+ `HandshakeSession`（宿主注入 client_id / connection_id / resume_context / last_global_sequence）；信封版本闸 `ensure_compatible_api_version`、`validate_{client,server}_frame_api_version`、`decode_{client,server}_frame_checked` |
| `src/resume.rs` | ~60 | `ResumeContext{earliest_available, current}` + 纯函数 `compute_resume_disposition`：`UpToDate` / `Replay{from,through}` / `SnapshotRequired`（越界一律回退 snapshot，fail-closed） |
| `src/snapshot.rs` | ~40 | `Snapshot::validate`：每节 `data` 与 `artifact_id` 恰一存在；inline `data` ≤256 KiB，超限必须转 artifact |
| `src/client_auth.rs` | ~400 | feature `client-auth`：`Token`（`getrandom` 32 字节、hex 64 字符、Debug 脱敏、`constant_time_eq`）、`TokenStore`（token 文件生成/加载/删除，Unix 父目录 0700 + 文件 0600）、`TokenAuthenticator`（实现 `ClientAuthenticator`，`TOKEN_SCHEME = "pawork-token"`）、`ClientAuthError` |
| `src/app/mod.rs` | ~20 | 子模块声明；`command` / `event` / `query` / `quota` / `limits` / `version` / `settings` glob 到 `app::*`（registry 不上 glob，走 `app::registry::` 路径） |
| `src/app/settings.rs` | ~400 | Settings `AppResponse::Data` 载荷（CLN-4，serde-only，不进 typegen）：`ApprovalModeWire`（五值 snake_case，无 kebab/`on_failure`）、`GeneralSettingsData` / `TerminalSettingsData` / `PermissionsSettingsData` / `ProviderAuthStatusData`（JSON 键 `default`）/ `ProviderAuthState` / `ProviderCatalogState`（`fixed_fallback`/`unavailable` 保留 `fetched_at: null`）/ `AuthStartData`。可空字段挂 `deserialize_required_option`：键必须出现，`null` 才是未设置。`SetApprovalMode.mode` 仍为 `String`，枚举供 Host/Desktop 消费。 |
| `src/app/version.rs` | ~220 | `ApiVersion`（major 相等即兼容、`bump_minor`）、`V1_0` … `V1_9`、`API_VERSION = 1.9`、`SUPPORTED_API_VERSIONS = [1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9]`、`ApiHandle{instance_id, api_version}`；文档性 `PROTOCOL_CRATE_COMPATIBILITY` 映射表；Control Plane 常量：`CONTROL_PLANE_SCHEMA_VERSION = 1`、默认 tenant / account / principal（`local-*`）、`ControlPlaneScope`（serde 缺省即默认 scope） |
| `src/app/command.rs` | ~870 | `AppCommandEnvelope`（`api_version` / `command_id` / `source` / `identity` / `expected_revision?` / `idempotency_key?` / `issued_at`）；`CommandSource` 6 变体、`ActorIdentity` 6 变体（`canonical_principal()` 供审计）；`AppCommand` 28 变体（清单见 §3.1）；`ApiKeySecret`（API key 明文 newtype：wire 透明字符串、Debug 恒 `[REDACTED]`、无 Display，ADR-046）；`ApprovalDecision`（approve_once / approve_for_run / deny / cancel）；`WorkspaceRelativePath`（构造即校验：拒绝绝对路径、`..`、Windows 盘符/UNC、反斜杠、控制字符；`FromStr` / serde 同路径校验）；IDE 上下文：`ClientContextSnapshot`（序列化 ≤1 MiB / ≤128 文档 / ≤1024 诊断 / URI ≤4 KiB 且拒 `..` / 诊断消息 ≤4 KiB，`validate()` fail-closed）及 `ClientDocumentContext` / `ClientDiagnostic` / `ClientTextRange` / `ClientTextPosition` / `ClientDiagnosticSeverity` |
| `src/app/query.rs` | ~160 | `AppQueryEnvelope`（`api_version` / `request_id` / `source` / `identity` / `issued_at`）；`AppQuery` 15 变体（清单见 §3.1）；timeline 分页形状 `TimelinePage{items, next_sequence?, head_sequence, complete}`、`TimelineItem`（sequence / event_id / kind / run_id? / text? / tool_name? / status? / detail? / timestamp）、`TimelineItemKind` 14 变体；`AppResponseEnvelope` + `AppResponse` 4 变体 |
| `src/app/event.rs` | ~620 | `AppEventEnvelope`（`global_sequence` 全序 + `stream` / `stream_sequence` 子流序，`validate_after` 双重单调校验）；`EventStream` 6 变体（Global / Workspace / Session / Run / Terminal / GuiClient，serde `tag="type", content="id"`）；`EventSource` 6 变体（Core / Command / Provider / Tool / Plugin / Mcp）；`AppEvent` 22 变体（清单见 §3.1）；`RunState` 12 态；`DiagnosticLevel` 3 档 + `from_degrade_severity_str` + `From<&DegradeEvent>`（DegradeEvent → `AppEvent::Diagnostic` 实时帧）；`AppEventOrderError`；Team 协作镜像类型（`TeamEvent` 18 变体、`TeamBoardTask` / `TeamPresence` / `TeamRecipients` / `TeamMemberRole` / `TeamTaskState` / `TeamPlanStepSnapshot` 等——源码注释称镜像自 `teams` crate，该 crate 已随 R0 归档，本处现为仓内唯一定义） |
| `src/app/quota.rs` | ~540 | 配额 canonical 视图：`QuotaOverviewQuery`（tenant / account / provider / credential 过滤 + `is_default_scope()`）、`QuotaOverviewView` / `QuotaScopeView` / `QuotaSnapshotView` / `QuotaFailureView` / `WindowReadView` / `WindowReadEntry`、`QuotaWindow` / `QuotaUnit` / `QuotaMeasure` / `QuotaConfidence` / `QuotaReset` / `QuotaAdapterKind` / `QuotaProvenanceView`；告警 `QuotaAlert`（`QuotaAlertKind` 5 变体：threshold / recovered / stale / reauthorization_required / partial_failure；severity 2 档）；`mask_credential_hint`（只留首尾各 2 字符，≤4 字符全掩码，空串 None）；默认 scope 常量 `DEFAULT_QUOTA_TENANT` 等 |
| `src/app/limits.rs` | ~150 | 租户策略 / RBAC 协议镜像：`PrincipalRole` 4 变体（Admin / User / Service / Viewer）、`PolicyGate` 9 变体（RouteCandidate / LeaseAcquire / AgentSpawn / RequestAdmission / SessionQuery / UsageQuery / AuditQuery / AuditExport / Retention）、`PolicyDecisionKind` 4 变体（Allow / Deny / Limit / Fallback）、`TenantPolicyView` / `PermissionProfileView` / `PrincipalRoleBinding` / `AuditExportPolicyView` / `PolicyDecisionEventView`（审计导出行形状） |
| `src/app/registry.rs` | ~520 | 单一授权事实源：`RegistryEntry`（`wire_name` / `gui: GuiChannelAccess{available, required_capability}` / `headless: Option<SdkCapability>` / `acp: bool` / `idempotent` / `since`）；静态表 `COMMANDS`（28 行）与 `QUERIES`（15 行）；穷尽 match `command_wire_name` / `query_wire_name` + 反查 `*_by_wire_name`（未登记 fail-closed 返回 None）+ `command_entry` / `query_entry`（表缺条目 panic fail-fast）；`GUI_INTRINSIC_CAPABILITIES = [Events, Snapshots]` 与派生函数 `gui_supported_capabilities()` |
| `src/adapter/mod.rs` | ~970 | feature `adapter`：外部 Agent 客户端接入契约——`AdapterWireFrame`、`CanonicalClientRequest` 5 变体（Command / Query / Attach / Reattach / Disconnect）、`CanonicalCoreFrame` 4 变体（Response / Event / SessionState / Error）、`AdapterErrorFrame` / `AdapterError`；trait `ClientAdapter`（decode / encode / capabilities）+ `ClientAdapterFactory`；`AdapterSessionContext`；`SessionRegistry` + `InMemorySessionRegistryStore`（attach / reattach / disconnect 的 CAS 所有权状态机，冲突回最新记录）；`MockClientAdapter(Factory)` 测试替身 |
| `src/adapter/identity.rs` | ~250 | 外部身份与租户绑定：`ExternalAgentIdentity`（session / agent / parent-agent id，`validate()` 长度与字符集闸、`is_subagent()`）、`TrustedTenantContext`（仅宿主注入）、`bind_tenant()` → `TenantBinding`（拒绝空段 / 越权字段，fail-closed）、`IdentityError` |
| `src/headless/mod.rs` | ~50 | headless-json 门面：re-export wire 类型；`stdio` 子模块由 feature `headless` 门控（wire / translate / json_mapping 恒可用） |
| `src/headless/wire.rs` | ~320 | NDJSON wire：`MAX_FRAME_BYTES = 4 MiB`；`SdkCapability` 5 变体（sessions / runs / streaming / compat_import / compat_history）；`HeadlessRequest` 5 变体 + `HelloRequest`；`HeadlessResponse` 6 变体；`ProtocolErrorKind` 9 变体（UnknownRequestType / UnsupportedCapability / IncompatibleApiVersion / NotHandshaked / MalformedFrame / TooLarge / CompatRejected / Backpressure / Internal）；`HeadlessError{kind, message}`；compat 词汇：`CompatSource` 4 变体（claude / codex / grok / cursor）、`CompatImportOptions` / `CompatImportReport` / `CompatHistoryEntry`、`CompatImportRequest` / `CompatHistoryQuery`；内部分发形状 `TranslatedRequest` |
| `src/headless/translate.rs` | ~170 | 纯翻译层（无 IO）：`parse_request_line`（大小闸 → JSON 解析 → 类型分发）、`translate_request(_line)`（信封 api_version 兼容闸）、`encode_response_line` / `encode_event_line` / `encode_protocol_response` / `encode_request`、`error_frame`；错误一律折叠为 `HeadlessResponse::Error` 帧而非中断 |
| `src/headless/stdio.rs` | ~230 | feature `headless`：`run_loop(reader, writer, handler, config)`——逐行读、握手前只接受 hello、`Handler` trait（handshake / handle / poll_event）回调宿主、`LoopConfig`（批模式 / 帧上限）；`StdioWriter` 有界写队列，队列满即发 `backpressure` 错误帧并终止（fail-closed，不静默丢帧） |
| `src/headless/json_mapping.rs` | ~340 | `JSON_TO_HEADLESS_EVENT_MAP`：`AgentEvent` 持久化 `type` 名 → headless `AppEvent` tag 的静态映射表 + `app_event_tag_for_json_type()`；内嵌测试保证映射目标是真实 `AppEvent` tag、表内事件名唯一 |
| `src/projection/mod.rs` | ~960 | 共享投影 reducer（消费 `app::` 的 `TimelineItem` / `TimelineItemKind`）：`project_event(&AgentEventEnvelope) -> Option<TimelineItem>`（历史臂降维）；`TimelineProjection` 状态机（`apply_event` live 臂 / `apply_item` 历史臂 / `apply_resume_disposition` / `reset_baseline`）与渲染态 `TimelineEntry`（`TimelineEntryKind` 5 类：UserMessage / AssistantMessage / ToolCall{name, status, detail} / RunState / Error）：去重、有序插入、assistant delta 合并、`MessageCommitted` 替换 delta 累积体、run 终态文案两端一致 |
| `src/typegen.rs` | ~320 | feature `typegen`：`generate()` / `check()` / `run()`——把 core-api / gui-protocol / headless-json 三组类型经 `ts-rs` 导出 `.d.ts`，与检入的 `schemas/{core-api,gui-protocol,headless-json}/` 逐文件 diff；`write_core_api_versions`（API 版本 TS 常量，检入后为 `core-api/versions.d.ts`）；`find_workspace_root` 定位仓库根；`TypegenError` |
| `src/bin/typegen.rs` | ~15 | `pawork-protocol-typegen` 二进制入口（生成或 `--check`），`required-features = ["typegen"]` |

## 3. 对外 API 面

### 3.1 core-api 信封与 registry（三通道共用词汇）

- **命令**：`AppCommandEnvelope` 携带 `api_version`（兼容闸）、`command_id`（幂等键）、`source: CommandSource`（local_cli / local_gui / remote_gui / automation / plugin / mcp）、`identity: ActorIdentity`（local_user / authenticated_client / automation / plugin / mcp_server / system）、可选 `expected_revision` / `idempotency_key`。命令与查询 serde 均为 `tag="method", content="params", snake_case`。
- **`AppCommand` 28 变体关键载荷**：`CoreInitialize`；`WorkspaceAdd{root_path}` / `WorkspaceTrust{workspace_id, trusted}`；`SessionCreate{workspace_id, title?}` / `SessionOpen{session_id}` / `SessionFork{session_id, parent_event_id}` / `SessionCompact{session_id}` / `SessionClientContextReplace{session_id, context}`；`RunStart{session_id, user_message, model?, provider?}` / `RunCancel{run_id}` / `RunRetry{run_id}` / `RunTool{run_id, tool_name, input}`；`AuthStart{provider_id, flow}` / `AuthRemove{provider_id}`；`AuthSetApiKey{provider_id, api_key: ApiKeySecret}` / `AuthCancel{provider_id}` / `SetDefaultModel{provider_id, model_id}`（ADR-046，since 1.4）；`SetProxyUrl{proxy_url: Option<String>}`（ADR-047，since 1.5；字段必填，显式 null 清除）；`SetApprovalMode{mode: String}`（ADR-048，since 1.6；mode 必填 snake_case 串，会话内生效不持久化）；`SetTerminalSettings{shell: Option<String>, columns: u16, rows: u16}`（ADR-050，since 1.8；三字段必填、全态写，`shell: null` 显式清除回平台默认）；`McpTest{name: String}` / `McpServerRemove{name: String}`（ADR-049，since 1.7；name 必填；回执 Data 形状同 mcp_list 的 servers 数组）；`ToolApprove{run_id, tool_call_id, decision}`；`GitStage{workspace_id, paths: Vec<WorkspaceRelativePath>}`；`TerminalCreate{working_directory?}` / `TerminalWrite{terminal_session_id, data}` / `TerminalResize{terminal_session_id, columns, rows}` / `TerminalClose{terminal_session_id}`（ADR-045，since 1.3）。
- **`AppQuery` 15 变体**：`WorkspaceList` / `SessionGet{session_id, timeline_after_sequence?, timeline_limit?}` / `RunStatus{run_id}` / `ModelList{provider_id?}` / `DiffListFiles{workspace_id}` / `DiffGet{workspace_id, path, cursor?}` / `ArtifactRead{artifact_id, offset, limit}` / `QuotaOverview{query}` / `SnapshotFetch` / `PluginList` / `McpList` / `ProviderAuthStatus{provider_id?}`（ADR-046，since 1.4） / `GeneralSettings`（ADR-047，since 1.5） / `PermissionsSettings`（ADR-048，since 1.6；响应 `Data` 四元组 approval_mode / workspace_trusted / trust_workspaces_global / workspace_id——末者为实现期修订增补的 Host 权威 attached id） / `TerminalSettings`（ADR-050，since 1.8；响应 `Data` 形状 `{ shell, columns, rows }`——shell 为 Global 持久值（null = 平台默认），columns/rows 为生效值（未设置 = 80/24））。
- **响应**：`AppResponse::Accepted{command_id, run_id?}`（`run_id` 仅 `RunStart` 回带，并发来源各自携带）/ `Data(Value)`（形状由 query 决定；`SessionGet` 返回 `TimelinePage`）/ `Artifact{artifact_id, byte_length, media_type}`（大结果转工件引用）/ `Error(ErrorContext)`（复用 domain 安全错误形状）。
- **registry 三通道登记全表**（GUI 列 = 可用性（+ 命令级所需能力）；headless 列 = 所需 `SdkCapability`，`—` 表示未映射、授权 fail-closed；未登记 wire 名一律拒绝）。命令 28 条：

| wire 名 | GUI | headless | ACP | 幂等 | since |
| --- | --- | --- | --- | --- | --- |
| `core_initialize` | ✗（host 专用） | — | ✗ | ✓ | 1.0 |
| `workspace_add` | ✓ | — | ✗ | ✗ | 1.0 |
| `workspace_trust` | ✓（需 Approvals；1.6 起 GUI 开放，ADR-048） | — | ✗ | ✓ | 1.0 |
| `session_create` | ✓ | sessions | ✓ | ✗ | 1.0 |
| `session_open` | ✓ | sessions | ✗ | ✓ | 1.0 |
| `session_fork` | ✓ | sessions | ✗ | ✗ | 1.0 |
| `session_compact` | ✗ | sessions | ✗ | ✗ | 1.0 |
| `session_client_context_replace` | ✗（S7 起 GUI 维持 PermissionDenied） | sessions | ✗ | ✓ | 1.0 |
| `run_start` | ✓ | runs | ✓ | ✗ | 1.2（`provider` 字段随 1.2 引入） |
| `run_cancel` | ✓ | runs | ✓ | ✓ | 1.0 |
| `run_retry` | ✗ | runs | ✗ | ✗ | 1.0 |
| `run_tool` | ✗ | runs | ✗ | ✗ | 1.0 |
| `auth_start` | ✓（1.4 起 GUI 开放，ADR-046） | — | ✗ | ✗ | 1.4（ADR-046） |
| `auth_remove` | ✓（1.4 起 GUI 开放，ADR-046） | — | ✗ | ✓ | 1.4（ADR-046） |
| `auth_set_api_key` | ✓ | — | ✗ | ✓ | 1.4（ADR-046） |
| `auth_cancel` | ✓ | — | ✗ | ✓ | 1.4（ADR-046） |
| `set_default_model` | ✓ | — | ✗ | ✓ | 1.4（ADR-046） |
| `set_proxy_url` | ✓ | — | ✗ | ✓ | 1.5（ADR-047） |
| `set_approval_mode` | ✓ | — | ✗ | ✓ | 1.6（ADR-048） |
| `set_terminal_settings` | ✓ | — | ✗ | ✓ | 1.8（ADR-050） |
| `mcp_test` | ✓ | — | ✗ | ✗ | 1.7（ADR-049） |
| `mcp_server_remove` | ✓ | — | ✗ | ✗ | 1.7（ADR-049） |
| `tool_approve` | ✓（需 Approvals） | runs | ✓ | ✓ | 1.0 |
| `git_stage` | ✗ | — | ✗ | ✓ | 1.0 |
| `terminal_create` | ✓（需 TerminalStreaming） | — | ✗ | ✗ | 1.0 |
| `terminal_write` | ✓（需 TerminalStreaming） | — | ✗ | ✗ | 1.0 |
| `terminal_resize` | ✓（需 TerminalStreaming） | — | ✗ | ✓ | 1.0 |
| `terminal_close` | ✓（需 TerminalStreaming） | — | ✗ | ✗（重复 close 报 not_found） | 1.3（ADR-045） |

查询 15 条（幂等恒 ✓、ACP 恒 ✗）：

| wire 名 | GUI | headless | since |
| --- | --- | --- | --- |
| `workspace_list` | ✓ | — | 1.0 |
| `session_get` | ✓ | sessions | 1.1（timeline 分页随 1.1） |
| `run_status` | ✓ | runs | 1.0 |
| `model_list` | ✓ | — | 1.0 |
| `diff_list_files` | ✓ | — | 1.0 |
| `diff_get` | ✓ | — | 1.0 |
| `artifact_read` | ✗（GUI 走专用 ArtifactRead 帧，AppQuery 变体未接线） | — | 1.0 |
| `quota_overview` | ✓ | — | 1.0 |
| `snapshot_fetch` | ✗（GUI 走专用 SnapshotRequest 帧） | — | 1.0 |
| `plugin_list` | ✗ | — | 1.0 |
| `mcp_list` | ✓ | — | 1.0 |
| `provider_auth_status` | ✓ | — | 1.4（ADR-046） |
| `general_settings` | ✓ | — | 1.5（ADR-047） |
| `permissions_settings` | ✓ | — | 1.6（ADR-048） |
| `terminal_settings` | ✓ | — | 1.8（ADR-050） |

- **事件**：`AppEventEnvelope{api_version, instance_id, event_id, global_sequence, stream, stream_sequence, timestamp, source, payload}` 双序号——`global_sequence` 全局单调（resume/ack 基准），`stream + stream_sequence` 子流单调；`EventStream` 定位归属（Global / Workspace(id) / Session(id) / Run(id) / Terminal(id) / GuiClient(id)），`EventSource` 标注产生方（Core / Command / Provider / Tool / Plugin / Mcp）。`AppEvent` 22 变体（GUI/headless 的**展示词汇**，与持久化 `AgentEvent` 32 变体不同层：粒度更粗、可裁剪、不承诺重放）：

| 变体 | 载荷要点 |
| --- | --- |
| `CoreReady` | `handle: ApiHandle` |
| `WorkspaceChanged` / `SessionChanged` | id + `revision`（客户端据此拉增量） |
| `RunChanged` | `run_id`、`state: RunState`（12 态：Created / PreparingContext / WaitingForProvider / StreamingResponse / CollectingToolCalls / WaitingForApproval / ExecutingTools / AppendingToolResults / Completed / Cancelled / Failed / Interrupted） |
| `AssistantDelta` / `ThinkingDelta` | `run_id`、`message_id`、`delta` |
| `ToolStarted` | `run_id`、`tool_call_id`、`name` |
| `ToolOutput` | `run_id`、`tool_call_id`、`delta`、`truncated`、`artifact_id?`（超限内容转工件） |
| `ToolApprovalRequired` | `run_id`、`tool_call_id`、`reason` |
| `ToolCompleted` | `run_id`、`tool_call_id`、`success` |
| `DiffChanged` | `workspace_id` |
| `TerminalOutput` | `terminal_session_id`、`delta` |
| `TerminalExited` | `terminal_session_id`、`exit_code?`、`signal?`、`reason: TerminalExitReason`（exited / killed / failed；ADR-045，1.3 起按协商 minor 门控推送） |
| `AuthChanged` | `provider_id`、`state: AuthChangeState`（pending / succeeded{method, masked_credential} / failed{error} / cancelled / expired / removed；ADR-046 起，全态脱敏） |
| `ProviderStatus` | `provider_id`、`status`（Ready / Degraded / Unavailable / AuthenticationRequired） |
| `PluginError` | `plugin_id`、`error: ErrorContext` |
| `Diagnostic` | `level: DiagnosticLevel`、`code`、`message`（DegradeEvent 实时帧落点） |
| `GuiClientConnected` / `GuiClientDisconnected` | `client_id`、`connection_id` |
| `QuotaChanged` / `QuotaAlert` | `Box<QuotaOverviewView>` / `Box<QuotaAlert>` |
| `TeamEvent` | `Box<TeamEvent>`（18 变体：TeamCreated / MemberAdded / MemberRemoved / TeamDissolved / TaskPosted / TaskClaimed / TaskReleased / TaskAdvanced / MailboxPosted / MailboxDelivered / MailboxRead / PresenceChanged / PeerMessageRouted / FanOutDenied / PlanSubmitted / PlanApproved / PlanRejected / PlanCommented） |
- **registry 消费方式**：宿主以 `command_by_wire_name` / `query_by_wire_name` 路由（未登记返回 None → 拒绝）；`command_entry` / `query_entry` 取变体登记（表缺失即 panic fail-fast）；`gui_supported_capabilities()` 派生 GUI 宣告向量，波 A 基线冻结为 `{Events, Snapshots, TerminalStreaming, Approvals}`（ArtifactStreaming 从未实现，K-08 / R0 D13 停止宣告、枚举冻结保留）。
- **quota 视图**（`QuotaOverview` 查询与 `QuotaChanged` / `QuotaAlert` 事件的共用形状）：入参 `QuotaOverviewQuery{tenant_id, account_id, provider_id?, credential_id?, model_id?, windows, unit?}`（`is_default_scope()` 判定本地默认租户）；出参 `QuotaOverviewView{scope, windows: Vec<WindowReadEntry>, generated_at, from_cache}`，逐窗口 `QuotaSnapshotView{scope, window, unit, values, reset, confidence, provenance, served_stale}` 或 `QuotaFailureView`（含 `adapter_kind?`）；告警 `QuotaAlert{tenant_id, account_id, provider_id, model_id?, window, unit, kind?, severity, source?, message, snapshot?, credential_hint?}`。所有 `credential_hint` 必须经 `mask_credential_hint` 脱敏后才可进视图。
- **limits / RBAC 视图**：`TenantPolicyView` / `PermissionProfileView` / `PrincipalRoleBinding` 描述租户策略与角色绑定；`PolicyDecisionEventView` 是策略决策审计导出的行形状（gate + decision kind + 主体 + 时间），供 control-plane 侧查询与导出复用。

### 3.2 GUI Connection Protocol

- **帧**（serde 统一 `tag="type", content="data", snake_case`；`HandshakeResponse` 例外，tag 为 `status`：accepted / rejected）：

| `ClientFrame`（11） | 载荷 | `ServerFrame`（10） | 载荷 |
| --- | --- | --- | --- |
| Handshake | `HandshakeRequest` | Handshake | `HandshakeResponse` |
| Command | `AppCommandEnvelope` | CommandAccepted | `request_id`、`command_id` |
| Query | `AppQueryEnvelope` | Response | `AppResponseEnvelope` |
| Subscribe | `SubscribeRequest{subscription_id, streams}` | Event | `AppEventEnvelope` |
| Unsubscribe | `request_id`、`subscription_id` | Snapshot | `Snapshot` |
| Resume | `ResumeRequest{last_global_sequence}` | Resume | `ResumeResponse{disposition}` |
| SnapshotRequest | `request_id` | ArtifactChunk | `ArtifactChunk` |
| Ack | `global_sequence` | Error | `ProtocolErrorEnvelope{request_id?, error}` |
| ArtifactRead | `ArtifactReadRequest{artifact_id, offset, limit}` | Heartbeat | `nonce` |
| Heartbeat / Pong | `nonce` | Pong | `nonce` |
- **codec**：`encode_*_frame` 序列化并检查 ≤1 MiB；`write_frame*` 加 `u32 LE` 长度前缀；`read_frame*` 先读长度、超限即拒（不读 payload）。同步版走 `std::io`，异步版走 tokio `AsyncRead/Write`。
- **握手**：`HandshakeRequest`（`client_name` / `client_version` / `supported_api_versions` / `capabilities` / 可选 `authentication`）交给 `HandshakeService::accept(request, HandshakeSession)`；`HandshakeSession` 由宿主提供 `client_id` / `connection_id` / 可选 `resume_context` 与服务端记录的 `last_global_sequence`（客户端上次确认序号不由客户端自报）。成功返回 `Accepted{request_id, selected_api_version, handle: ApiHandle, client_id, connection_id, resume, capabilities, host_data_dir?}`；其中 `host_data_dir` 由宿主通过 builder 注入，只在认证成功响应发布，缺字段保持向后解码。失败 `Rejected{error}`——版本协商失败 → `incompatible_version`，配置了 `ClientAuthenticator` 但凭据缺失或验证失败 → `authentication_failed`。授予能力 = 客户端请求 ∩ 服务端支持（服务端支持向量通常来自 `gui_supported_capabilities()`）。
- **resume**：`compute_resume_disposition(earliest_available, current, last_seen)` 三态——`UpToDate{current_sequence}` / `Replay{from_sequence, through_sequence}`（闭区间）/ `SnapshotRequired{earliest_available_sequence}`；last_seen 早于可用历史起点或超前 current 一律回退 snapshot；握手时无 `resume_context` 或服务端无该客户端记录也直接 `SnapshotRequired`（fail-closed）。
- **snapshot / artifact**：`Snapshot{instance_id, snapshot_sequence, generated_at, sections}` 按 `SnapshotSectionKind` 6 节（workspaces / session_tree / active_runs / pending_tool_approvals / terminal_sessions / provider_status）组织，每节 `data` 与 `artifact_id` 恰一存在、inline `data` ≤256 KiB；`ArtifactChunk{offset, data, eof}` ≤64 KiB 逐块传输。
- **错误**：`ProtocolError{code, message, retryable}`；`ProtocolErrorCode` 10 变体（incompatible_version / invalid_frame / authentication_failed / permission_denied / request_not_found / replay_unavailable / frame_too_large / busy / validation_failed / internal；1.4 增 busy / validation_failed，ADR-046）。
- **认证**（feature `client-auth`）：`TokenStore` 生成 32 字节随机 token（hex 64 字符）写入 0600 文件（父目录 0700）；`TokenAuthenticator` 校验 scheme 必须为 `pawork-token` 且 proof 常数时间比较。

### 3.3 headless-json（NDJSON）

- 每行一个 JSON 帧，行 ≤4 MiB。请求 5 型：`hello`（`HelloRequest{client_name, client_version, supported_api_versions, capabilities}`）、`command` / `query`（复用 core-api 信封）、`compat_import`（Claude / Codex / Grok / Cursor 会话导入；`CompatImportOptions{dry_run}` → `CompatImportReport{source?, session_id, original_id?, imported_events/messages/tool_calls/tool_results/usages/reviews, raw_records, deduplicated, unknown_fields}`）、`compat_history`（统一历史清单，游标分页，条目 `CompatHistoryEntry{session_id, source, original_id?, imported_events, imported_at_unix_ms}`）。
- 响应 6 型：`hello_ack{instance_id, negotiated, granted}`（能力交集）、`response{envelope}` / `event{envelope}`（事件推送需 `streaming` 能力）、`compat_import_result{request_id, report}` / `compat_history_result{request_id, entries, cursor?}`、`error{request_id?, kind, message}`（`ProtocolErrorKind` 9 变体）。
- `translate.rs` 是无 IO 纯函数层（宿主可单独复用做单测或嵌入其他 loop）：`parse_request_line` 三段闸（大小 → JSON → 类型分发），`translate_request` 加信封版本闸，`encode_response_line` / `encode_event_line` / `error_frame` 组装出站行。
- `stdio.rs::run_loop(reader, writer, handler, config)` 提供参考事件循环：`Handler` trait 三个 async 方法——`handshake(HelloRequest) -> HeadlessResponse`（决定 hello_ack 或 error）、`handle(TranslatedRequest) -> Vec<HeadlessResponse>`（一请求可多响应）、`poll_event() -> Option<HeadlessResponse>`（拉取要推送的事件）；`LoopConfig{batch_mode, max_frame_bytes}`（batch 模式读到 EOF 即收尾，交互模式常驻）。
- `json_mapping.rs` 维护持久化 `AgentEvent` type 名 → headless `AppEvent` tag 的静态映射（如 `assistant_text_delta` → `assistant_delta`），供宿主把重放历史翻成 headless 事件流。

### 3.4 adapter 与外部身份（feature `adapter`）

- `ClientAdapter` 把外部客户端 wire 解码为 `CanonicalClientRequest` 5 变体——`Command(AppCommandEnvelope)` / `Query(AppQueryEnvelope)` / `Attach(ClientSessionRecord)` / `Reattach{client_session_id, ownership_epoch, revision, connection_id, state, updated_at}` / `Disconnect{client_session_id, ownership_epoch, revision, updated_at}`——并把 `CanonicalCoreFrame` 4 变体（`Response(AppResponseEnvelope)` / `Event(AppEventEnvelope)` / `SessionState(ClientSessionRecord)` / `Error(AdapterErrorFrame{code, message, capability?})`）编码回外部 wire；`ClientAdapterFactory` 按 `ClientProtocol` 实例化；`AdapterSessionContext` 携带会话期上下文。
- `SessionRegistry`（配 domain 的 `SessionRegistryStore`；本包内置 `InMemorySessionRegistryStore`）：attach 建 `ClientSessionRecord`、reattach 走 CAS 所有权转移、冲突返回最新权威记录供客户端重同步。
- 身份与租户：`ExternalAgentIdentity{session_id?, agent_id?, parent_agent_id?}` 是客户端自报的外部归属，`validate()` 做长度与字符集闸、`is_subagent()` 判定子代理；`TrustedTenantContext{tenant_id, principal_id}` 仅由宿主注入（唯一租户事实源，不信任客户端自报）；`bind_tenant()` 把两者合成 `TenantBinding{identity, tenant, session_id, agent_id?, parent_agent_id?}`，空段/非法段 fail-closed 返回 `IdentityError`。

### 3.5 projection（两臂共用 reducer）

- 历史臂：`project_event` 把持久化信封逐条降维为 `Option<TimelineItem>`（非 UI 事件返回 None）；`TimelineItemKind` 14 变体：user_message / assistant_delta / assistant_message / tool_started / tool_output / tool_completed / approval_requested / approval_responded / run_started / run_completed / run_cancelled / run_failed / diagnostic / other。
- live 臂：`TimelineProjection::apply_event` 直接消费广播 `AppEventEnvelope`。两臂产出统一 `TimelineEntry`（**纯数据渲染态，非 wire 类型、不进帧**）：`sequence` / `event_id`（去重键）/ `kind` / `fork_boundary` / `timestamp` / `run_id`；按序插入、`AssistantMessage` delta 增量合并、`MessageCommitted` 原子替换累积体、`RunState` 终态文案统一（Completed / Cancelled / Failed）。两臂只把运行提示类 Diagnostic（`sandbox.fallback`、`checkpoint.snapshot_failed`，P2 片 2B）显示为运行提示；`resources.injected` 等信息诊断不进入 timeline，历史重放不得把它们误标为 Error。
- `TimelineEntryKind` 5 变体（渲染态词汇，区别于 wire 侧 `TimelineItemKind` 14 变体）：`UserMessage{text}` / `AssistantMessage{text}` / `ToolCall{name, status, detail?}` / `RunState(String)` / `Error(String)`。
- `ForkBoundary` 3 变体（Completed / Cancelled / Failed）：R6（ADR-040 D5）规定 fork 只许切在闭合 turn 边界，reducer 单点判型——仅历史 `RunCompleted/RunCancelled/RunFailed` 与 live `RunState` 三终态打此标记；`TimelineEntry::is_fork_boundary()` 是 Desktop 唯一判界入口，禁止对 `kind` 文案做字符串匹配。
- 入口汇总：`TimelineProjection::{apply_item, apply_event, entries, reset_baseline}` + `apply_resume_disposition`——`UpToDate` 保持、`Replay` 保留基线等待补帧、`SnapshotRequired` 触发 `reset_baseline`（丢弃 live 增量、以快照重建）。

### 3.6 typegen（feature `typegen`）

`cargo run -p pawork-protocol --features typegen --bin pawork-protocol-typegen` 重新生成 `schemas/{core-api,gui-protocol,headless-json}/`；`--check`（及 `tests/typegen.rs`）做逐文件 diff——缺文件、多余文件、内容漂移均失败。三组导出的类型闭包：

- `core-api`：`AppCommandEnvelope` / `AppQueryEnvelope` / `AppResponseEnvelope` / `AppEventEnvelope` 四信封的 `export_all` 闭包 + `versions.d.ts`（`API_VERSION` / `SUPPORTED_API_VERSIONS` TS 常量），面向 SDK；
- `gui-protocol`：`ClientFrame` / `ServerFrame` 闭包，面向 Desktop GUI；
- `headless-json`：`HeadlessRequest` / `HeadlessResponse` 闭包，面向 headless 消费方。

每组另生成 `index.d.ts` re-export barrel 与 `serde_json/JsonValue.d.ts`（`serde_json::Value` 的 TS 对应）；`versions.d.ts` 仅存在于 `core-api` 组。当前检入规模：`core-api` 81 / `gui-protocol` 98 / `headless-json` 88 个 `.d.ts`（ADR-045 新增 `TerminalExitReason` 后）。所有产物带 `// @generated by pawork-protocol-typegen; do not edit.` 头。

## 4. 核心行为与数据流

1. **GUI 连接生命周期**：连接建立 → 客户端发 `ClientFrame::Handshake` → `HandshakeService::accept`（版本协商 → 认证 → 能力交集 → 按宿主记录的 `last_global_sequence` 与 `ResumeContext` 算 resume disposition）→ `Accepted` 可附 Host 注入的 `host_data_dir`，随后客户端 `Subscribe`（按 `EventStream` 过滤）→ 服务端推 `Event` 流、客户端周期 `Ack{global_sequence}` 推进服务端记录的已确认序号 → `Heartbeat` / `Pong`（nonce 回显）保活；入站/出站信封逐帧过 `validate_*_frame_api_version` 版本闸。
2. **断线重连**：重握手（服务端凭自己记录的确认序号算 disposition）或客户端显式发 `Resume{last_global_sequence}` → `Replay` 则服务端按 `[from_sequence, through_sequence]` 补发、`SnapshotRequired` 则客户端 `SnapshotRequest` 取全量分节快照（大节走 artifact 分块）→ 投影 `apply_resume_disposition` 决定保留基线还是重建。
3. **headless run_loop**：逐行读 stdin → 未握手时仅 `hello` 合法（其余回 `not_handshaked` 错误帧）→ `hello` 经 `Handler::handshake` 协商 → 后续行经 `parse_request_line`（超限 `too_large`、非法 JSON `malformed_frame`、未知类型 `unknown_request_type`）→ `TranslatedRequest` 交 `Handler::handle`；事件由 `Handler::poll_event` 拉取写出；`StdioWriter` 队列满 → 发 `backpressure` 错误帧并终止循环（fail-closed）。
4. **三通道闸门**：宿主收到 wire 名 → registry 查 `RegistryEntry` → GUI 通道 `available=false` 或缺 `required_capability`、headless 缺对应 `SdkCapability`、ACP `acp=false`，任一不满足即拒绝；`since` 高于协商版本的形状对旧客户端不可用。穷尽 match 保证新增 `AppCommand` / `AppQuery` 变体时 registry 不补齐则编译失败。
5. **投影归一（CR08-08）**：同一事件序列无论走「历史分页（`project_event` → `apply_item`）」还是「live 广播（`apply_event`）」，终态 `entries()` 必须逐字段一致；golden 夹具同时钉住分页交错、Lagged→Snapshot、fork 切支三个难点。
6. **外部客户端接入**：外部 wire → `ClientAdapter::decode` → `Attach`（校验 `ExternalAgentIdentity` + `bind_tenant`）→ `SessionRegistry` 建档/CAS 转移 → 后续 Command/Query 复用 core-api 信封；核心侧回程帧经 `encode` 翻回外部 wire；断连走 `Disconnect` 或记录保留待 `Reattach`（`ownership_epoch` + `revision` 双计数防旧持有者写回）。
7. **typegen 生成/校验**：`find_workspace_root` 定位仓库根 → 在 scratch 目录经 `ts-rs` 导出三组类型 → `collect_declarations` 收集 `.ts` 产物、生成 `index.d.ts` barrel → 生成模式 `write_declarations` 覆写 `schemas/<组>/`、校验模式 `check_declarations` 逐文件 diff——任何差异即 `TypegenError`；CI 语义由 `tests/typegen.rs` 承载。
8. **GUI token 生命周期**（feature `client-auth`）：宿主 `TokenStore::generate` → 确保父目录存在并置 0700 → `getrandom` 取 32 字节熵转 64 位十六进制 → 以 `create_new` 写入（文件已存在报 `AlreadyExists`，绝不覆盖）并置 0600；客户端持 token 以 `scheme = "pawork-token"` 发起握手 → 服务端 `TokenAuthenticator` 先核 scheme、再 `constant_time_eq` 比对 proof，失败返回 `HandshakeResponse::Rejected{request_id, error: ProtocolError}`（`AuthenticationFailed`）；`TokenStore::load` 对缺失/空文件/非 UTF-8 分别报 `NotFound` / `Malformed`。
9. **降级实时帧**：宿主拿到 domain `DegradeEvent` 后，走 `From<&DegradeEvent> for AppEvent` 转成 `AppEvent::Diagnostic{level, code, message}` 广播（`DiagnosticLevel::from_degrade_severity_str` 映射严重度）；与持久化路径（`to_agent_event()` 落 `AgentEvent::Diagnostic`）互补，见 [domain.md](domain.md) §4。

## 5. 契约与不变量

- **wire 冻结**：所有帧/信封 serde 形状由 golden 锁定——`crates/protocol/tests/golden/` 64 个 fixture，漂移即 `tests/golden.rs` 失败；重建需显式 `GUI_PROTOCOL_UPDATE_GOLDEN=1`。构成：
  - 客户端帧 33 个（`client_*.json` 28 个 + `session_get_timeline.json` + `provider_auth_status.json` + `general_settings.json` + `permissions_settings.json` + `terminal_settings.json`）：handshake / command（含 terminal_create、terminal_create_working_directory、terminal_resize、terminal_write、terminal_close 5 个终端专样，auth_start、auth_remove、auth_set_api_key、auth_cancel、set_default_model 5 个 Settings 认证专样——ADR-046，set_proxy_url / set_proxy_url_clear 2 个通用设置专样——ADR-047，set_approval_mode / workspace_trust 2 个权限审批专样——ADR-048，mcp_test / mcp_server_remove 2 个 MCP 专样——ADR-049，与 set_terminal_settings / set_terminal_settings_clear 2 个终端设置专样——ADR-050）/ session_get、provider_auth_status、general_settings、permissions_settings 与 terminal_settings query / subscribe / unsubscribe / resume / snapshot_request / ack / artifact_read / heartbeat / pong；
  - 服务端帧 30 个（`server_*.json`）：handshake_accepted（主样例含 ADR-051 `host_data_dir`，up_to_date 变体保留字段缺失兼容）/ handshake_rejected / command_accepted / response（含 terminal_create、auth_set_api_key、provider_auth_status、general_settings、set_proxy_url 清除回执 `{"proxy_url":null}` 专样，permissions_settings（`trust_workspaces_global` null / 布尔各一）、set_approval_mode 回执 `{"approval_mode":…}`、workspace_trust 回执 `{"workspace_trusted":…}` 4 个权限审批专样——ADR-048，mcp_test / mcp_server_remove 回执——ADR-049，与 terminal_settings 响应 / set_terminal_settings 清除回执 `{"shell":null,…}`——ADR-050）/ event（含 terminal_output、terminal_exited、auth_changed 专样）/ snapshot / resume 三态各一 / artifact_chunk / error / heartbeat / pong；
  - 类型样本 1 个：`timeline_page.json`（`TimelinePage` 形状）。
- **headless fixture**：`crates/protocol/tests/fixtures/headless/` 四个 JSON 用例集驱动翻译层双向断言：
  - `translate_cases.json`（6 例：`input_line` → 期望 `TranslatedRequest`）；
  - `error_cases.json`（4 例：非法行 → 期望 `ProtocolErrorKind`）;
  - `event_cases.json`（3 例：事件行 → 期望 payload 类型）；
  - `compat_response_cases.json`（3 例：compat 导入响应与 `request_id` 回填）。
- **projection golden**：`crates/protocol/tests/fixtures/projection/` 三组 `.jsonl`（每行 domain / wire / item 三视图）+ `.expected.json` 终态快照（paged_interleave / lagged_to_snapshot / fork_branch_switch）。
- **schemas 检入产物**：仓库根 `schemas/{core-api,gui-protocol,headless-json}/*.d.ts` 是 GUI/SDK 的 TS 契约事实源，`tests/typegen.rs` 强制与源码同步；三组各含 `index.d.ts` + `serde_json/JsonValue.d.ts`，`headless-json` 组额外覆盖 `HeadlessRequest` / `HeadlessResponse` / `SdkCapability` / `CompatImport*` / `ProtocolErrorKind`，`gui-protocol` 组覆盖 `ClientFrame` / `ServerFrame` / 握手与快照类型；schema/wire 演进只允许出现在 R6/R7 且须 ADR Accepted。
- **版本纪律**：`API_VERSION = 1.9`，`SUPPORTED_API_VERSIONS = [1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9]`；同 major 兼容、加变体/字段须 bump minor 并在 registry 标 `since`（先例：session_get 分页 1.1、run_start.provider 1.2、terminal_close + TerminalExited 随 ADR-045 进 1.3、Settings 认证词汇随 ADR-046 进 1.4、通用设置词汇随 ADR-047 进 1.5、权限与审批词汇随 ADR-048 进 1.6、工具与 MCP 词汇随 ADR-049 进 1.7、终端设置词汇随 ADR-050 进 1.8、Accepted 握手可选 Host data directory 随 ADR-051 进 1.9）；`CONTROL_PLANE_SCHEMA_VERSION = 1` 与 GUI wire 版本独立。
- **Control Plane scope 兼容**：`ControlPlaneScope` 各字段 serde 缺省即本地默认租户（`local-*` 常量），单租户旧数据无需迁移即可解码；显式多租户字段只能由宿主写入。
- **大小上限**（超限一律拒绝而非截断）：帧 1 MiB、artifact 块 64 KiB、snapshot 节 inline 256 KiB、headless 行 4 MiB、client context 序列化 1 MiB。
- **安全不变量**：`ClientAuthentication` / `Token` / `ApiKeySecret` Debug 脱敏、token 文件 0600 且父目录 0700、常数时间比较；`WorkspaceRelativePath` 拒绝路径逃逸（`GitStage` 等文件命令只收相对路径，serde 反序列化同样过校验、不能绕过构造器）；`ClientContextSnapshot.validate()` 限量限长、URI 拒 `..`；租户上下文只信宿主注入；`mask_credential_hint` 保证 quota 视图不外泄凭据 ID；registry 未登记 wire 名 fail-closed。
- **双序号不变量**：`AppEventEnvelope.validate_after` 要求 `global_sequence` 严格递增、同流 `stream_sequence` 严格 +1，违规返回 `AppEventOrderError`；resume 语义完全建立在 `global_sequence` 全序之上。
- **registry 穷举守卫**：`command_wire_name` / `query_wire_name` 是无通配符穷举 match——新增 `AppCommand` / `AppQuery` 变体而不补 registry 表则编译失败；wire 名与 serde `method` tag 的双射由逐变体 round-trip 测试钉死。
- **fork 边界单点判型**：`ForkBoundary` 标记只由 projection reducer 产生（闭合 turn 三终态），Desktop 判界一律走 `is_fork_boundary()`；对 `TimelineEntryKind` 文案做字符串匹配属违约用法。

## 6. 依赖关系

- **内部**：仅 `pawork-domain`（feature `typegen` 时连带 `pawork-domain/typegen`）。
- **外部**：`serde` / `serde_json` / `thiserror` / `tokio`（io-util / sync / macros / rt / time，主依赖）；可选 `async-trait`（`adapter`、`headless`）、`getrandom`（`client-auth`）、`ts-rs`（`typegen`）。dev：`tempfile`。
- **Feature**：`default = ["adapter", "client-auth", "headless"]`；`typegen` 非默认；`[[bin]] pawork-protocol-typegen` 要求 `typegen`。
- **下游**：`pawork-app`、`pawork-client`（生产依赖）、`pawork-cli`（`features = ["adapter"]`）；`pawork-storage` 仅 dev-dep（`default-features = false, features = ["adapter"]`，测 SQLite 版 SessionRegistryStore）。
- 全景见 [../../architecture.md](../../architecture.md)、[../../design.md](../../design.md) §2；GUI 链路与事件链路见 [../flows.md](../flows.md)；Desktop 侧消费见 [../../gui-design.md](../../gui-design.md)。

## 7. 测试与验证资产

| 文件 | 覆盖点 |
| --- | --- |
| `tests/golden.rs` | 全帧型 golden JSON 字节比对（64 fixture），锁定 serde tag/content/rename 约定；`GUI_PROTOCOL_UPDATE_GOLDEN=1` 重建 |
| `tests/frames.rs` | 全帧型 round-trip + 1 MiB 帧上限拒绝 |
| `tests/codec_framing.rs` | u32 LE 长度前缀、同步/异步读写、DeclaredLengthTooLarge / UnexpectedEof |
| `tests/handshake.rs` | 版本协商矩阵、认证钩子（TokenAuthenticator 成功/失败/缺凭据）、能力交集、Accepted `host_data_dir` present/absent 与旧帧缺字段解码、`decode_*_checked` 信封版本闸 |
| `tests/registry.rs` | 穷尽 match 守卫 28 命令 / 15 查询、wire 名与 serde tag 双射逐变体 round-trip、GUI 宣告向量 V2 快照冻结、三通道开关与 `since` 断言 |
| `tests/headless_protocol.rs` | fixture 驱动翻译往返、错误帧形状、`run_loop` 输出（含握手闸与背压） |
| `tests/projection_golden.rs` | live 臂 vs 历史臂对拍 + 三组 golden 终态快照（分页交错 / Lagged→Snapshot / fork 切支） |
| `tests/projection_semantics.rs` | reducer 语义：assistant 去重合并、分页去重与 committed 替换、live/分页交错、resume 三态基线、run 终态文案两端一致 |
| `tests/resume.rs` | `compute_resume_disposition` 三态边界 |
| `tests/snapshot.rs` | data/artifact_id 互斥与 256 KiB 界 |
| `tests/typegen.rs` | `typegen::check()` 与检入 `schemas/` 零 diff（需 `--features typegen`） |
| `tests/golden/`（64 文件） | GUI 帧与 timeline 类型 golden 夹具 |
| `tests/fixtures/headless/`（4 文件）、`tests/fixtures/projection/`（6 文件） | headless 翻译用例、projection 三视图序列与期望终态 |
| `src/client_auth.rs` tests | token 生成长度与十六进制字符集、文件 0600 / 目录 0700、`generate` 不覆盖已存在文件、常数时间比较、认证成功/失败/错 scheme |
| `src/adapter/{mod,identity}.rs` tests | `InMemorySessionRegistryStore` CAS 状态机（attach / reattach / 冲突返回权威记录）、`ExternalAgentIdentity` 长度与字符集闸、`bind_tenant` fail-closed |
| `src/headless/json_mapping.rs` tests | `AgentEvent` → headless 映射目标合法性与唯一性（禁止两个持久化变体映射到同一 wire 名） |
| `src/app/{command,event,version,quota}.rs` tests | `WorkspaceRelativePath` 逃逸拒绝、`AppEventEnvelope.validate_after` 双序号、`negotiate` 版本矩阵、quota alert wire 名与 `mask_credential_hint` 不泄漏 |

默认验证命令：`cargo test -p pawork-protocol --offline --lib --tests`（typegen 测试另需 `--features typegen`）。

2026-09-03 SET-6g 以 `--features typegen` 运行上述门禁，155/155 通过；64 个 GUI fixture 数量不变，44 个引用当前 API 的 fixture 仅做 minor 8→9 机械更新，typegen 与检入 schema 同步。

## 8. 注意事项与已知限制

- `AppEvent`（22 变体，展示层）与 domain `AgentEvent`（32 变体，持久化层）是**两套词汇**：前者可裁剪不承诺重放，后者才是重放事实源；映射登记在 `headless/json_mapping.rs`。
- 两套 `ApprovalDecision` 拼写不同（本包 `approve_once` vs domain `approved_once`），见 [domain.md](domain.md) §8。
- `app::registry` 故意不 glob 到 crate 根，避免与变体名混淆；引用须写全路径 `app::registry::…`。
- `headless` 的 wire / translate / json_mapping 无条件编译，仅 `stdio`（`Handler` / `run_loop`）在 feature `headless` 之后；tokio 是主依赖而非 feature 门控（源码注释：避免测试再开 feature）。
- `GuiCapability::ArtifactStreaming` 与 `AppQuery::ArtifactRead` / `SnapshotFetch` / `PluginList` 为冻结保留形状：GUI 走专用帧或从未接线，registry 中 GUI 通道标不可用。
- adapter 的 `InMemorySessionRegistryStore` 仅供测试/单进程场景；生产持久化实现位于 storage（session feature）。registry 表中 headless / ACP 列在 R3 波 A 只登记数据，消费切换在波 B 完成——以宿主实现为准核对现状。
- `PROTOCOL_CRATE_COMPATIBILITY` 是文档性映射，不参与 wire 协商。
- `TimelineEntry` / `TimelineEntryKind` / `ForkBoundary` 是纯数据渲染态：不进帧、不参与 typegen（`schemas/` 三组均无对应 `.d.ts`），跨进程传输的时间线形状只有 `TimelinePage` / `TimelineItem`。
- `AppQueryEnvelope` 没有 `expected_revision` / `idempotency_key`（查询天然幂等）；命令去重语义全部挂在 `AppCommandEnvelope` 上。
- Team 镜像类型源码注释仍称"与 `teams::TeamEvent` 同形"，但 `teams` crate 已随 R0 归档、不在当前 21 成员内；protocol 侧形状现为仓内唯一定义，仓内消费方是 `pawork-cli` 的 ACP 映射。
- 相关文档：[domain.md](domain.md) · [testkit.md](testkit.md) · [../README.md](../README.md) · [AGENTS.md](../../../AGENTS.md)。
