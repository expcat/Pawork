# pawork-domain

> 最底层 canonical 领域类型与跨 crate 契约面：纯数据 + 协作式取消，零 IO、零内部 `pawork-*` 依赖，是全仓库依赖方向的根。

## 1. 职责与边界

- 承载全部 canonical 数据形状：事件信封 v1（`AgentEventEnvelope` + `AgentEvent` 32 变体）、Provider 契约（`ModelProvider` / `CanonicalModelRequest` / `ProviderStreamEvent` 13 变体）、Tool 契约（`ToolDescriptor` / `AgentTool`）、消息模型、类型安全 ID、降级事件、Agent Profile、Phase 16 工作流事件载荷。
- 唯一的"行为"是基于标准库的协作式取消（`CancellationToken`），其余全部是可 serde 往返的纯数据与纯函数。
- **不做**：IO、数据库、HTTP、Git、GUI framework（含 GPUI/Tauri）、OS Keychain、任何具体 Provider 名称分支。R1 起（ADR-039）原 `pawork-api` 的 `provider_api` / `tool_api` 并入本包，纯净红线不变。
- Feature：`typegen`（可选 `ts-rs` derive，供 protocol typegen 链使用）；`plugin = []` 为 F41 复活锚（空数组，无实际代码）。

## 2. 模块与文件地图

`src/` 为扁平单层；`lib.rs` 中所有 `mod` 私有，逐一 `pub use` 到 crate 根，消费方一律 `use pawork_domain::…`。

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~40 | 模块声明 + 全量 re-export；crate 级红线文档 |
| `src/ids.rs` | ~110 | `string_id!` 宏生成 37 个 String newtype ID（基础 27 个：`SessionId` / `RunId` / `WorkspaceId` / `EventId` / `ToolCallId` / `ProviderId` / `ProtectedBlobRef`…；Phase 16 追加 10 个：`PlanId` / `GoalId` / `BackgroundTaskId` / `AutomationId` / `MonitorId` / `MemoryId` / `ReviewSessionId` 等）；`Timestamp`（Unix epoch 毫秒，u64） |
| `src/events.rs` | ~510 | `CURRENT_SCHEMA_VERSION = 1`、`EventSequence`、`AgentEventEnvelope`（含 `validate_after` 顺序校验、`with_parent`）、`AgentEvent` 32 变体、`ApprovalDecision`、`ToolOutputStream`、`EventOrderError`、`ProviderTranscriptContinuation` |
| `src/message.rs` | ~300 | `Message` / `MessageRole`（System/User/Assistant/Tool）/ `ContentPart` 7 变体、`ToolResultContent`、`ArtifactReference`、`MessageMetadata`、`TokenUsage`、`Cost`（微单位整数）、`StopReason` 8 变体 |
| `src/provider_api.rs` | ~940 | `CanonicalModelRequest` 及子结构（`ToolDefinition` / `HostedToolRequest` / `ExtensionToolRequest` / `ToolChoice` / `ThinkingConfig` / `ResponseFormat` / `PromptCachePreference` / `RequestBudget`）、`ProviderStreamEvent` 13 变体、trait `ModelProvider` / `ProviderEventSink`、`ModelResponseSummary`、`ResolvedCredential` / `CredentialKind`、`ProviderError` / `ProviderErrorKind` 15 变体、`ModelDefinition` / `ModelCapabilities`（v1 布尔 + P15-8 v2 字段）、`ModelTransport`、能力协商类型（`CapabilityRequirements` / `ResolvedCapabilities` / `CapabilityFallback` / `ReasoningStateDescriptor` / `ReasoningStateCapability` / `ReasoningConfig`）、`clamp_effort_to_thinking_level`、映射错误（`ServerToolMappingError` / `ReasoningMappingError`） |
| `src/tool.rs` | ~380 | Canonical Tool v2：`ToolKind` 3 位点、`ContinuationMode` 2 模式、`ToolCapabilityTag` 14 变体及 `capability_key()`（稳定 `tool:PascalCase` wire key）、`ToolHosting` 3 变体、`ToolCapability` 7 调度分类、`ToolDescriptor`（含 `has_consistent_hosting`） |
| `src/tool_api.rs` | ~250 | trait `AgentTool` / `ToolEventSink`、`ToolRequest`、`ToolExecutionContext`（`workspace_id` + 相对 `working_directory`）、`ToolResult`、`ToolStreamEvent`（OutputDelta / Progress / ArtifactAvailable）、`ToolOutputChannel`、`ToolError` / `ToolErrorKind`（含 `NotLocallyExecutable`） |
| `src/server_tool.rs` | ~400 | P15-5 server tool 归一：`ServerToolEvent` 11 变体、`Citation` / `CitationSourceKind`、`Source`、`ProgramStream`、`TranscriptItem`、`ProviderTranscriptEnvelope`（cursor / continuation_reference） |
| `src/reasoning.rs` | ~90 | `ReasoningEffort` 6 档（None…Max，默认 Medium）、`ReasoningItem`（`protected_blob_ref` 代替明文 continuation） |
| `src/profile.rs` | ~290 | Agent Profile v2（P17-5）：`AgentProfileV2` 全维度（prompt / model / effort / tools / skills / mcp / permissions / hooks / memory / max_turns / background / isolation）、`ProfileToolRules`（deny 优先）、`ProfileRef`（version pin）、`ProfilePrompt` / `ProfileModel`、`ProfileMemory` / `ProfileMemoryAvailability`（fail-closed）、`ProfileIsolation` |
| `src/provider_hints.rs` | ~170 | `provider_hints.<provider>.<key>` 命名空间：`is_provider_hint_key` / `canonical_hint_key`、`LEGACY_HINT_KEY_MAP`（冻结读兼容）、`MAX_HINT_KEY_BYTES = 128` / `MAX_HINT_VALUE_BYTES = 64 KiB`、预定义键（OpenAI summary entries / Anthropic block kind） |
| `src/degrade.rs` | ~250 | 降级可观测契约（R4 T8）：`DegradeEvent` / `DegradeKind` 6 类 / `DegradeSeverity` 3 档 / `DegradeSink` 2 通道、`to_agent_event()` 转 `AgentEvent::Diagnostic` |
| `src/cancel.rs` | ~160 | `CancellationToken`（Arc + AtomicBool + waker 表）与 `CancellationFuture`；cancel 幂等、Drop 时清理 waiter |
| `src/client_session.rs` | ~155 | Client session registry 词汇（S13-F15 下沉）：`CLIENT_ADAPTER_SCHEMA_VERSION = 1`、`ClientSessionId` / `ClientProtocol` / `ClientCapability`、`CapabilitySnapshot`（validate）、`ClientSessionRecord` / `ClientSessionState`、trait `SessionRegistryStore`（CAS 语义）、`RegistryWriteOutcome` / `SessionRegistryError` |
| `src/workflow.rs` | ~540 | Phase 16 canonical 事件载荷：`PlanEvent`（8）、`GoalEvent`（8）、`TaskEvent`（4）、`AutomationEvent`（4）、`MonitorEvent`（4）、`MemoryEvent`（2）、`ReviewEvent`（4）及其状态/快照类型（详见 §3.6） |
| `src/error.rs` | ~40 | `ErrorCategory` 14 变体（跨 crate 稳定错误大类）、`ErrorContext`（可安全跨边界，禁 Secret） |

## 3. 对外 API 面

### 3.1 ID 与时间

`string_id!` 生成的 ID newtype 统一提供 `new` / `as_str` / `into_inner` / `From<String>` / `From<&str>` / `Display`，serde 为透明字符串。全量 37 个：

- 基础 27 个：`ActorId` `AgentId` `ArtifactId` `AccountId` `CheckpointId` `CredentialId` `CommandId` `ConnectionId` `CoreInstanceId` `EventId` `GuiClientId` `MessageId` `ModelId` `PluginId` `PrincipalId` `ProtectedBlobRef` `ProviderId` `QueryId` `ReasoningItemId` `RequestId` `RunId` `SessionId` `TenantId` `TerminalSessionId` `ToolCallId` `ToolExecutionId` `WorkspaceId`；
- Phase 16 追加 10 个：`PlanId` `PlanStepId` `PlanVersionId` `GoalId` `BackgroundTaskId` `AutomationId` `MonitorId` `MemoryId` `ReviewSessionId` `ReviewFindingId`。

`Timestamp(pub u64)` 序列化为整数毫秒（Unix epoch），保证跨语言无损。

### 3.2 事件（持久化契约）

`AgentEventEnvelope` 字段：`schema_version`（构造时自动填 `CURRENT_SCHEMA_VERSION = 1`）、`event_id`、`session_id`、`run_id`、`sequence: EventSequence`、`timestamp`、可选 `parent_event_id`（缺省不序列化）、`payload: AgentEvent`。`validate_after(previous)` 强制同 session 且 `sequence` 严格 +1，违规返回 `EventOrderError::{DifferentSession, NonContiguousSequence}`。

`AgentEvent` 32 变体（serde `tag="type", content="data", snake_case`），逐变体载荷：

| 变体 | 载荷要点 |
| --- | --- |
| `RunStarted` | `trigger_message_id` |
| `ContextPrepared` | `message_count`、`estimated_input_tokens` |
| `ProviderRequestStarted` | `request_id`、`provider_id`、`model` |
| `UsageUpdated` | `usage: TokenUsage`——流式用量快照，监督器据此保证失败/取消时已发生用量不丢 |
| `AssistantTextDelta` / `AssistantThinkingDelta` | `message_id` + `delta` |
| `ToolCallStarted` | `tool_call_id`、`name` |
| `ToolCallArgumentsDelta` | `tool_call_id`、`json_delta`（分片 JSON） |
| `ToolApprovalRequested` | `tool_call_id`、`reason` |
| `ToolApprovalResponded` | `tool_call_id`、`decision: ApprovalDecision`、`comment?` |
| `ToolExecutionStarted` | `tool_call_id` |
| `ToolOutputDelta` | `tool_call_id`、`stream: ToolOutputStream`、`delta` |
| `ToolExecutionCompleted` | `tool_call_id`、`result: ToolResultContent` |
| `MessageCommitted` | 完整 `Message`（delta 累积体的原子提交点） |
| `ProviderTranscriptContinued` | `calls: Vec<ProviderTranscriptContinuation>`——Hosted/Extension 成功 dispatch，仅在本轮全部为 Provider-owned 调用时发出 |
| `ServerTool` | `ServerToolEvent`（见 §3.5；与本地 `ToolCall*` 并列但语义分离） |
| `TranscriptEnvelope` | `ProviderTranscriptEnvelope`（provider-neutral、持久化前脱敏） |
| `CompactionStarted` | `source_event_count` |
| `CompactionCompleted` | `summary_message_id`、`compacted_through: EventSequence` |
| `CheckpointCreated` | `checkpoint_id`、`artifacts`（缺省空） |
| `CheckpointRolledBack` | `checkpoint_id` |
| `RunCompleted` | `stop_reason: StopReason`、`usage: TokenUsage` |
| `RunCancelled` | `reason?`、`usage?`（附加式，旧行可解码） |
| `RunFailed` | `error: ErrorContext`、`usage?` |
| `Plan` / `Goal` / `Task` / `Automation` / `Monitor` / `Memory` / `Review` | Phase 16 包装（载荷见 §3.6） |
| `Diagnostic` | `code: String`、`details: Value`——向前兼容通道，未知 Provider 元数据不得污染 canonical 分支 |

配套枚举：`ApprovalDecision`（`approved_once` / `approved_for_run` / `denied` / `cancelled`——注意与 protocol 侧命令动词 `approve_once`… 拼写不同，两套 wire 各自冻结）；`ToolOutputStream`（stdout / stderr / structured 语义由 `tool_api::ToolOutputChannel` 对应）。

### 3.3 Provider 契约

- trait `ModelProvider`：`id()`、`list_models(credential?)`、`stream(request, sink, cancel) -> ModelResponseSummary`；trait `ProviderEventSink::emit(event)`。`ModelResponseSummary` 含 `stop_reason` / `usage` / `response_id?` / `provider_metadata`。
- `CanonicalModelRequest` 关键字段：
  - `request_id` / `model` / `messages`；
  - 三类工具声明分列——`tools`（`ToolDefinition`，ClientFunction）、`hosted_tools`（`HostedToolRequest`：canonical 名 + `ToolCapabilityTag`，不携带 Provider 名）、`extensions`（`ExtensionToolRequest`：外部引用 + `requires_approval`）；
  - `tool_choice: ToolChoice`（None / Auto / Required / Named）；
  - `thinking: Option<ThinkingConfig>`（`ThinkingLevel` Off/Low/Medium/High，旧 P6 兼容）与 `reasoning: Option<ReasoningConfig>`（P15-8 权威，显式 effort 优先）；
  - `temperature` / `max_output_tokens` / `stop_sequences` / `response_format`（Text / Json / JsonSchema{schema}）/ `prompt_cache`（Automatic / Disabled / Required）/ `budget: RequestBudget`（timeout_ms / max_cost_micros / max_input_tokens）/ `trace_id`；
  - `provider_options: BTreeMap`——Provider-specific wire 透传；adapter 只合并非保留键（model / messages / stream / tools / tool choice / auth header 等会破坏 wire 不变量的键必须忽略）。
- `ProviderStreamEvent` 13 变体（声明序即 golden 序）：`ResponseStarted{response_id?}`、`TextDelta`、`ThinkingDelta`、`ReasoningItem`（敏感 continuation 已换成 `ProtectedBlobRef`）、`ToolCallStarted{id, name}`、`ToolCallArgumentsDelta{id, json}`、`ToolCallCompleted{id}`、`UsageUpdated(TokenUsage)`、`ResponseCompleted(StopReason)`、`ProviderMetadata(Value)`、`ServerTool(ServerToolEvent)`、`TranscriptEnvelope(ProviderTranscriptEnvelope)`、`Error(ProviderError)`。
- `ProviderError`：`kind`（15 变体：Authentication / Authorization / RateLimited / QuotaExceeded / InvalidRequest / ModelNotFound / ContextTooLarge / ContentFiltered / Network / Timeout / ProviderUnavailable / StreamInterrupted / MalformedResponse / Cancelled / Unknown）+ `retryable`（`new()` 按 kind 给默认：RateLimited / Network / Timeout / ProviderUnavailable / StreamInterrupted 可重试）+ `retry_after_ms?` / `provider_request_id?` / `http_status?` / `redacted_details?` / `diagnostics`；`category()` 映射 `ErrorCategory`，与 `ErrorContext` 双向 `From`。
- `ResolvedCredential`：字段私有，`Debug` 输出 `[REDACTED]`，**不实现 Serialize**；`expose_secret()` 仅 Provider adapter 构造认证请求时读取。
- 能力面：`ModelDefinition{id, display_name, context_window_tokens, max_output_tokens, capabilities}`；`ModelCapabilities` v1 布尔基线（text / image_input / tool_calls / parallel_tool_calls / thinking / structured_output / prompt_cache）+ v2 字段（`transport: ModelTransport`、`hosted_tool_tags: BTreeSet<ToolCapabilityTag>`、`citations`、`reasoning: ReasoningStateCapability`），v2 全部 `#[serde(default)]` fail-closed；`ModelTransport`（Responses / Messages / ChatCompletions 默认，`is_modern()`）；协商输入 `CapabilityRequirements` → 输出 `ResolvedCapabilities`（不变量 `requested == supported ∪ unsupported`，逐项 `CapabilityFallback` 记录 ClientTool / LegacyTransport / ClampedEffort / Reject 原因）。

### 3.4 Tool 契约

- `ToolKind` 决定唯一续接方式：`ClientFunction → ContinuationMode::CoreSuppliedResult`（唯一本地执行位点），`ProviderHosted` / `ProviderExtension → ProviderTranscript`；调用方不能在结果对象上覆写。
- `ToolCapabilityTag` 14 变体：WebSearch / WebFetch / FileOrCollectionSearch / XSearch / CodeExecution / HostedShell / ProviderApplyPatch / ComputerUse / ImageGeneration / ServerSideMcp / ToolSearch / Memory / ProgrammaticToolCalling / ServerSideMultiAgent；`capability_key()` 产出稳定 `tool:PascalCase` wire 名，穷举 match 守卫新增变体。
- `ToolCapability` 7 调度分类：ReadOnly（唯一允许并发）/ WorkspaceWrite / GitWrite / Process / Network / UserInteraction / ExternalPlugin（F41 预留）。
- `ToolDescriptor`：`name` / `description` / `input_schema` / `capability` / `kind` / `hosting`（须与 kind 一致，`has_consistent_hosting()` 校验）/ `capabilities` / `requires_approval` / `read_only` / `supports_concurrency` / `default_timeout_ms` / `max_output_bytes` / `allowed_in_untrusted_workspace`。v2 新增字段全带 serde 默认，旧 JSON 缺省解为 ClientFunction/Local。
- trait `AgentTool`：`descriptor()` + `execute(request, context, sink, cancel)`。`ToolExecutionContext` 只携带 `workspace_id` + 相对 `working_directory`，绝对路径由可信 Workspace 服务解析（路径安全红线）。`ToolResult` 仅表示 ClientFunction 结果（success/failure 构造器、`is_error()`、`truncated` 标记）；`ToolError::not_locally_executable()` 供 hosted/extension 被误调用时 fail-closed。
- `ToolStreamEvent`：`OutputDelta{channel, delta}` / `Progress{message}` / `ArtifactAvailable{artifact}`，经 `ToolEventSink::emit` 流出。

### 3.5 消息与 server tool

- `Message{id, role, content, metadata}`；`ContentPart` 7 变体：Text / Image（`ImageSource`）/ Thinking / Reasoning（引用 `ReasoningItem`）/ ToolCall / ToolResult / ArtifactRef。`MessageMetadata`：`model?` / `provider?` / `usage?` / `cost?` / `timestamp?` / `artifacts` / `stop_reason?` / `incomplete` / `trace_id?` / `provider_metadata`（BTreeMap，键须过 provider_hints 语法）。`TokenUsage{input_tokens, output_tokens, cache_read_tokens, cache_write_tokens}`（cache 字段 serde 默认 0）。`StopReason` 8 变体：Completed / StopSequence / MaxTokens / ToolUse / ContentFiltered / Cancelled / Error / Other(String)。
- `ServerToolEvent` 11 变体（全部携带 `tool_call_id()`；`type_name()` 给持久化 event_type）：

| 变体 | 载荷要点 |
| --- | --- |
| `Started` | `name`、`arguments?` |
| `ArgumentsDelta` | `json_delta`（分片 JSON） |
| `Progress` | `message?` |
| `Completed` | `summary?`、`artifacts: Vec<ArtifactId>` |
| `Failed` | `message?`、`code?` |
| `CitationAdded` | `citation: Citation`（缺省字段为空不猜值，`CitationSourceKind` 默认 Unknown） |
| `SourceAdded` | `source: Source`（原始来源元数据） |
| `ComputerActionRequested` | `action: Value`（computer-use 动作请求） |
| `ComputerScreenshot` | `artifact: ArtifactId`、`media_type?`——截图只存工件引用（ADR-018） |
| `ProgramStarted` | `command?` |
| `ProgramOutput` | `stream: ProgramStream`（stdout/stderr）、`delta?` 与 `artifact?` 互斥 |

  `TranscriptItem`（ServerTool / Text）与 `ProviderTranscriptEnvelope`（output items + cursor + continuation_reference）构成 transcript 续传的归一形状。

### 3.6 Phase 16 工作流事件（`workflow.rs`）

全部作为 `AgentEvent` 包装变体持久化，重放红线适用：

- `PlanEvent` 8 变体（Plan Mode + 评审 gate）：配 `PlanStepStatus` / `PlanReviewStatus` 状态机、`PlanStepSnapshot`、`PlanCommentAnchor`（行锚点）。
- `GoalEvent` 8 变体（Goal Mode）：配 `GoalStatus`、`SuccessCriterionSnapshot` / `CriterionKind`。
- `TaskEvent` 4 变体（Background Task Manager）：配 `TaskKind` / `TaskStatus`。
- `AutomationEvent` 4 变体（Scheduled Automation）：配 `AutomationTriggerKind`（cron / interval / once / event）。
- `MonitorEvent` 4 变体（Persistent Process / Monitor）：配 `MonitorSourceKind`。
- `MemoryEvent` 2 变体（Long-term Memory）：配 `MemoryPrivacy`；`Recorded` 的 `embedding` / `confidence` 为附加式可选字段。
- `ReviewEvent` 4 变体（Review Engine）：配 `ReviewAnchor` / `ReviewSeverity` / `ReviewResolution` / `SuggestedPatch`；`FindingOpened` 的 `evidence` / `fingerprint` 为附加式可选字段。

### 3.7 其余主题

- **reasoning**：`ReasoningEffort`（none / low / medium / high / x_high / max，serde 名稳定；`requires_reasoning_support()`）；`ReasoningItem` 只持 `protected_blob_ref` 与非敏感 metadata。
- **profile**：`ProfileToolRules::policy()` deny 优先返回 `Denied / Allowed / Unrestricted`；`ProfileMemory::availability()` 存在 `unavailable` 标注时无条件 `Unavailable`（绝不虚假可用）；`ProfileIsolation`（None / Restricted / Container）。
- **hints**：合法键 = `provider_hints.` 前缀 + 小写 provider 段 + ASCII 键段且总长 ≤128B；`canonical_hint_key` 只查旧拼写映射（规范键/未知键返回 None），写路径永不产出旧拼写。
- **degrade**：`DegradeKind` 6 类（HomeDirFallback / MissingCredential / EventStreamLagged / TasksFinishFailed / IdempotencyConflict / AcpState），`code()` = `degrade.<suffix>` 逐字冻结；`default_sink()` 只有 `TasksFinishFailed` 落事件流，其余走帧 + stderr；`to_agent_event()` 在 details 上合并 kind / severity / message 三键（冲突以契约为准），非 object details 包进 `"context"`。
- **cancel**：`CancellationToken::cancel()` 幂等并唤醒全部 waiter；`cancelled()` 返回可 await 的 `CancellationFuture`（Drop 自动注销 waiter，不泄漏）。
- **client_session**：`CapabilitySnapshot::validate()` 校验 schema 版本与非空字段；`SessionRegistryStore` 定义原子 ownership CAS（`insert` / `compare_and_swap` / `remove_if_owner`），冲突返回最新权威记录供重同步；内存实现在 pawork-protocol，SQLite 实现在 pawork-storage。
- **error**：`ErrorCategory` 14 变体（Provider / Tool / Internal / Cancelled / RateLimit / Timeout / Authentication / Authorization / InvalidRequest / NotFound / Conflict / ResourceExhausted / Unavailable / MalformedData）；`ErrorContext{category, message, retryable, retry_after_ms?, diagnostics}`。

## 4. 核心行为与数据流

1. **Provider 流式调用**：Engine 构造 `CanonicalModelRequest` → `ModelProvider::stream` 内 adapter 翻译为具体协议 → 逐事件 `sink.emit(ProviderStreamEvent)` → 结束返回 `ModelResponseSummary`；出错以 `Err(ProviderError)` 收尾（可先 emit `ProviderStreamEvent::Error`）。
2. **协作式取消**：同一 `CancellationToken` 克隆传给 Provider 与 Tool；`cancel()` 后 `is_cancelled()` 立即可见、`cancelled().await` 全部唤醒；Provider/Tool 应返回 `ProviderError::cancelled` / `ToolError::cancelled`（映射 `ErrorCategory::Cancelled`）。
3. **三类工具位点**：ClientFunction 走 `AgentTool::execute` 产生 `ToolResult`（唯一被 adapter 翻译为 function-result 的路径）；ProviderHosted/Extension 由 Provider 服务端执行，Core 只把 adapter 归一的 `ServerToolEvent` / `ProviderTranscriptEnvelope` 持久化，续接凭 `ProviderTranscriptContinued` + cursor / continuation_reference，不生成本地 `ToolResult`、不进 scheduler。
4. **事件持久化顺序**：写入方以 `AgentEventEnvelope::new`（自动打 v1 版本）构造，追加前用 `validate_after` 保证同 session 内 sequence 严格连续；`parent_event_id` 支撑因果重放。
5. **能力协商**：目录侧声明 `ModelCapabilities` → 请求侧给 `CapabilityRequirements` → 协商产出 `ResolvedCapabilities`，不满足项逐个记录 fallback 或 Reject；缺失 v2 字段一律按"不支持"处理（fail-closed），显式 `clamp_effort_to_thinking_level` 才允许降档。
6. **降级双通道**：接点构造 `DegradeEvent` → 按 `default_sink()` 分流——可重放接点 `to_agent_event()` 落 `AgentEvent::Diagnostic`（persist-first），启动期/流受损接点只发 protocol 实时帧 + stderr（帧转换 `From<&DegradeEvent> for AppEvent` 定义在 pawork-protocol）。

## 5. 契约与不变量

- **信封版本独立**：`CURRENT_SCHEMA_VERSION = 1` 是磁盘/线上信封契约版本，与 session-store 的 SQLite migration 链版本（`crates/storage/src/session/migration.rs`，当前至 version 13）相互独立：加迁移不必动信封版本，反之亦然。
- **字节级 golden**（形状漂移即测试失败，演进须 ADR + 显式重建）：
  - `crates/domain/tests/fixtures/agent_event_envelope_variants.jsonl`（32 变体信封逐行字节比对）与 `agent_event_envelope_parent.json`（parent_event_id 序列化）；
  - `crates/domain/tests/fixtures/provider_stream_event_13.jsonl`、`canonical_model_request_full.json`、`provider_error_full.json`、`tool_result_pair.jsonl`。
- **Secret 红线**：`ResolvedCredential` 无 Serialize、Debug 脱敏；`ReasoningItem` 不存 encrypted_content / signature 明文（只存 `ProtectedBlobRef`）；`ErrorContext` / `DegradeEvent.details` / 事件 payload 不得携带 Secret；旧 `ThinkingContent.signature` 反序列化时直接丢弃。
- **Provider-neutral 断言**：`CanonicalModelRequest` 三类工具声明与 `ProviderTranscriptEnvelope` 不携带 Provider 名称 / api_key / secret 字段（测试逐字符串扫描）；Engine 不得按 Provider 名走特例（架构红线）。
- **冻结词表**：`ToolCapabilityTag::capability_key()` 穷举守卫；`DegradeKind` 的 6 个 `degrade.*` code 逐字冻结；`LEGACY_HINT_KEY_MAP` 只追加不改行；`ReasoningEffort` serde 名稳定（`x_high` 等）。
- **重放兼容**：新增字段一律 `#[serde(default)]` 附加式（如 `RunCancelled.usage`、`MemoryEvent::Recorded.embedding`、`ReviewEvent::FindingOpened.evidence`、`ToolResultContent.artifacts`）；Goal / Automation / Monitor 等事件类型**保留**（重放红线），对应 reducer 已在 R0 归档，不是现行产品面。

## 6. 依赖关系

- **上游（外部）**：`serde` / `serde_json` / `thiserror` / `async-trait`；可选 `ts-rs`（feature `typegen`）。dev：`tokio`（macros / rt / rt-multi-thread / time，仅测试）。
- **上游（内部）**：无——本包是依赖根。
- **下游（生产依赖方，16 包）**：auth、control-plane、engine、git、orchestration、policy、protocol、providers、storage、testkit、tools、workflow、workspace、app、cli、client。
- **不直接依赖本包**：`pawork-exec`（ADR-052 只依赖 policy，domain 仅经 policy 传递）、`pawork-transport`（字节/进程层不感知领域类型）。
- 全景依赖方向见 [../../architecture.md](../../architecture.md) 与 [../../design.md](../../design.md) §2；跨包链路见 [../flows.md](../flows.md)。

## 7. 测试与验证资产

| 资产 | 覆盖点 |
| --- | --- |
| `tests/events_golden.rs` | 32 变体计数守卫 + 逐条 round-trip + 与检入 jsonl 字节比对；parent envelope 字节比对；重建入口为 ignored 测试 `write_event_envelope_golden`，须 `PAWORK_WRITE_EVENT_GOLDEN=1` |
| `tests/contract_golden.rs` | `ProviderStreamEvent` 13 变体 / `ProviderError` / `CanonicalModelRequest` / `ToolResult` 字节 golden（`GOLDEN_UPDATE=1` 重建）；行数、字节、回读三重断言 |
| `tests/fixtures/`（6 个） | `agent_event_envelope_variants.jsonl` · `agent_event_envelope_parent.json` · `provider_stream_event_13.jsonl` · `canonical_model_request_full.json` · `provider_error_full.json` · `tool_result_pair.jsonl` |
| `src/cancel.rs` tests | 取消幂等、多 waiter 唤醒、Drop 注销 |
| `src/events.rs` tests | 信封顺序校验、legacy 行解码（缺省字段） |
| `src/message.rs` tests | ContentPart 全变体往返、legacy `signature` 丢弃 |
| `src/provider_api.rs` tests | `ResolvedCredential` 脱敏 Debug、错误映射、no-provider-branch 扫描、能力协商不变量 |
| `src/tool.rs` tests | `capability_key` 穷举、hosting 一致性 |
| `src/profile.rs` tests | deny 优先、memory fail-closed |
| `src/provider_hints.rs` tests | 键语法、长度界、冻结映射 |
| `src/degrade.rs` tests | `degrade.*` code 逐字 pin、sink 分流表、`to_agent_event` details 合并 |
| 其余内嵌 tests（`reasoning` / `server_tool` / `tool_api` / `workflow`） | effort serde 名与降档、`ServerToolEvent` 往返与 `type_name` 稳定、`ToolError` 分类映射、Phase 16 载荷 serde 往返 |

默认验证命令：`cargo test -p pawork-domain --offline --lib --tests`。

## 8. 注意事项与已知限制

- 两套审批枚举并存且拼写不同：本包 `ApprovalDecision`（`approved_once`）用于持久化事件，protocol 侧 `ApprovalDecision`（`approve_once`）用于命令；不要互换。
- `ModelCapabilities` v1 布尔字段与 v2 结构并存：v1 是 P6 兼容基线，`thinking: bool` 仅作派生源；判定现代能力一律走 v2 字段。`clamp_effort_to_thinking_level` 是旧 P6 adapter 的显式降级入口（XHigh/Max → High），不形成双轨。
- `ToolCapability::ExternalPlugin` / `PluginId` / feature `plugin` 均为 F41 生态预留，当前无运行时消费。
- `client_session.rs` 只定义 trait 与记录形状，不含任何存储逻辑；两个实现分别在 protocol（内存）与 storage/session（SQLite）。
- 源码注释中引用的 "26 帧 golden"（`degrade.rs`）是 R4 时点的历史计数，protocol 侧 golden 夹具现已扩至 32 个文件；以 `crates/protocol/tests/golden/` 实际内容为准。
- 相关文档：[protocol.md](protocol.md) · [testkit.md](testkit.md) · [../README.md](../README.md) · [../../../ROADMAP.md](../../../ROADMAP.md)。
