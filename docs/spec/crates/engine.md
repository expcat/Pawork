# pawork-engine

> Agent Engine：把 canonical 消息与工具定义装配成 `CanonicalModelRequest`，驱动多轮工具循环并把全过程事件化。位于 Core 执行核层，生产依赖仅 `pawork-domain`（严格单向：domain ← engine ← app/cli）。

## 1. 职责与边界

- **做什么**：请求装配（`assemble_request` / `assemble_request_with_tools`）；单轮流式调用（内部 `run_turn`，`pub(crate)`）；多轮工具循环（`run_session`）；单轮会话事件化（`run_session_turn`）；上下文预算 / 压缩触发 / token 估算 / tool result 裁剪（`context` 子模块）；run 级取消（`CancelHandle`）；手动压缩（`run_manual_compaction`）。
- **不做什么**：不重试、不落库（落库由调用方在 `AgentEventSink` 里 persist-first）、不选通道、不读 Secret、不执行工具（经 `LoopContext` 回调宿主）、不杀进程树（经 `ProcessTreeCleaner` 注入）、不按 Provider 名称分支。
- **宿主注入点**：`ModelProvider`（模型流）、`AgentEventSink`(事件出口)、`LoopContext`（工具执行 / 审批 / 压缩 / 快照回调）、`ProcessTreeCleaner`（取消时杀树）、`TokenEstimator` 与 `ContextLimits`（经 `TurnContext`）。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~410（非测试 ~95） | crate 门面与 re-export；`assemble_request(_with_tools)` 冻结默认值装配；`pub(crate) run_turn` 单轮原语（预取消检查 + `provider.stream`，13 变体原样透传） |
| `src/tool_loop/mod.rs` | ~360 | `run_session` 编排；`LoopContext` trait；`ApprovalGate` / `PendingToolInvocation` / `WriteCheckpoint` / `CompactionOutcome`；`DEFAULT_MAX_TOOL_ROUNDS` |
| `src/tool_loop/round.rs` | ~110 | 单轮 `run_turn` 收集（`collect_stream_round`）、助手消息装配、usage 饱和加法 |
| `src/tool_loop/approval.rs` | ~110 | `wait_and_apply`：`request_approval` await 后补发 `ToolApprovalResponded`；gate 数不匹配 fail-closed Denied；ApprovedForRun 跨轮记忆 |
| `src/tool_loop/exec.rs` | ~210 | 待执行调用解析、写快照、`execute_tools`、结果对齐与 `MessageCommitted` |
| `src/tool_loop/compaction.rs` | ~470 | 注入层、输入估算、软限压缩 / 硬限截断、`run_manual_compaction` |
| `src/tool_loop/tests.rs` | ~2180 | 原 `tool_loop.rs` 内联测试整文件迁入（`#[cfg(test)]`） |
| `src/session_turn.rs` | ~640（非测试 ~195） | `SessionTurn`（会话轮次标识 + start_sequence）；`run_session_turn` 单轮事件化（无工具循环）；`now_timestamp` |
| `src/appender.rs` | ~330 | `AssembledTurn`：把 `ProviderStreamEvent` 流折叠成一条助手 `Message`（text / thinking / reasoning / tool_calls / summary）；`PendingToolCall`；`ToolCallResult`；`tool_results_message` |
| `src/cancel.rs` | ~190 | `CancelHandle`（原子幂等 cancel：取消根 token → 触发 cleaner 杀树）；`CancelReason` / `CancelReceipt`；`ProcessTreeCleaner` trait 与 `NoopProcessTreeCleaner` |
| `src/event.rs` | ~245 | `EngineError`；`AgentEventSink` trait；`EventEmitter`（pub(crate)，sequence 分配 + 信封封装）；`LoopEventEmitter`（工具流事件入口）；`LoopSink`（pub(crate)，Provider 事件双写：映射转发 + 缓冲）；`map_provider_event` |
| `src/context/mod.rs` | ~65 | `context` 门面；`ContextLimits`（硬预算 + 历史软限）；`InjectedLayer`（宿主注入的系统提示层）；`TurnContext`（默认全禁用，retained 4） |
| `src/context/budget.rs` | ~90 | `ContextBudget`（`from_context_window` 推导 `max_input_tokens`，serde 形状冻结）；`ContextBudgetBreakdown` 占用明细 |
| `src/context/compaction.rs` | ~150 | `compute_compaction` 触发判定纯函数（硬限优先于软限）；`CompactionReason` / `CompactionTrigger`（serde snake_case）；`AutoCompactionReason`（engine → host 原因，含 `Manual`） |
| `src/context/token.rs` | ~290 | `TokenEstimator` trait（`count_text` 为核心，message / content part / tool schema 计数为默认实现）；`HeuristicEstimator`（非 CJK chars/4，CJK/Kana/Hangul 按 1 字符/token）；`ToolSchema`；`MESSAGE_FRAMING_TOKENS` 由 `count_message` 直接使用；`reply_primer_tokens`（pub(crate)） |
| `src/context/tool_result_trim.rs` | ~420 | tool result 分级裁剪：`TrimThresholds`（2/16/256 KiB）、`ResultSize`（Small/Medium/Large/Huge）、`trim_tool_result(_with)`、`TrimmedToolResult`（`retained_full` 暂存原文）、`byte_len_of_tool_result` |
| `tests/domain_only.rs` | ~90 | 红线断言：解析本包 Cargo.toml，生产 `pawork-*` 依赖必须恰为 `{pawork-domain}`（覆盖 alias 与 target 表） |
| `tests/no_provider_branch.rs` | ~115 | 红线断言：扫描 `src/` 全部 `.rs`，禁止出现任何 provider 名串；名单 = `pawork-providers::CHANNEL_REGISTRY` 派生通道 id + 固化基线别名（openai/anthropic/grok/glm 等） |

## 3. 对外 API 面

### 3.1 请求装配

- `assemble_request(request_id, model, messages) -> CanonicalModelRequest`：以冻结契约默认值填满其余字段（tools/hosted/extensions 空、`ToolChoice::Auto`、`ResponseFormat::Text`、`PromptCachePreference::Automatic`、`RequestBudget::default()`、thinking/reasoning/temperature/max_output_tokens 为 None）。
- `assemble_request_with_tools(...)`：同上但 `tools` 取入参。

### 3.2 多轮循环 `run_session`

`run_session(provider, request, turn: SessionTurn, events: &dyn AgentEventSink, cancel: CancellationToken, loop_ctx: &dyn LoopContext, max_tool_rounds: u64, context: TurnContext) -> Result<ModelResponseSummary, EngineError>`

- 是否继续下一轮以 `AssembledTurn::has_tool_calls` 为准，不看 `StopReason`。
- `turn.start_sequence` 必须 ≥ 1（session_events CHECK），否则立即 `EngineError::Sink`。
- `DEFAULT_MAX_TOOL_ROUNDS = 20`：达到上限发 `RunFailed`（`ErrorCategory::ResourceExhausted`）并返回 `EngineError::MaxToolRounds`，不再开下一轮 stream。
- 成功返回的 `ModelResponseSummary.usage` 为整个 run 的累计（饱和加法），失败 / 取消事件的 usage 为累计值（全零时省略）。

### 3.3 `LoopContext` trait（宿主回调，逐方法语义）

- `execute_tools(calls: Vec<PendingToolInvocation>, events: LoopEventEmitter, cancel) -> Vec<ToolCallResult>`：执行已放行的调用；执行期间可经 `events.emit_tool_event` 转发 `ToolStreamEvent::OutputDelta`（映射为 `AgentEvent::ToolOutputDelta`；`Progress` / `ArtifactAvailable` 被忽略）。返回结果不要求与 `calls` 等长同序——engine 用 `align_tool_results` 按 invocation 顺序对齐，缺失项回填 `NotFound` 失败结果。
- `request_approval(calls: &[PendingToolInvocation], already_approved_for_run: bool, events, cancel) -> Result<Vec<ApprovalGate>, EngineError>`：对整批调用逐个给出闸门，返回向量必须与 `calls` 等长。`ApprovalGate::NotRequired` = 策略已放行不发审批事件；`ApprovalGate::Asked(decision)` = 用户可见审批。**实现契约（K-02）**：每次阻塞等待决策前必须先 emit `AgentEvent::ToolApprovalRequested`（含 batch 已批准的短路路径），reason 逐字为 ``tool `{name}` requires approval``；engine 只补发 `ToolApprovalResponded`。`already_approved_for_run = true` 时不应再询问用户。
- `next_message_id() -> MessageId` / `next_request_id() -> RequestId`：为助手消息、工具消息、摘要消息与每轮新请求分配 id（内部摘要请求也从这里取 request_id）。
- `compact_history(reason: AutoCompactionReason, summary_text: &str, cancel) -> Result<Option<CompactionOutcome>, EngineError>`：压缩回调，host（app）负责 session 侧 fork/snapshot 后回传元数据（`source_event_count` + `compacted_through`）。默认实现返回 `Ok(None)`（无持久化宿主时 engine 仍完成消息层压缩）；宿主侧失败**必须**返回 `Err`，engine 将终止当前 run，不静默吞掉。
- `snapshot_write_tools(calls, events, cancel) -> Vec<WriteCheckpoint>`：写工具执行前由宿主拍快照，默认空；engine 只对每个返回项发 `AgentEvent::CheckpointCreated`，不依赖 blob/git。快照失败时宿主可经 `events`（`LoopEventEmitter::emit`）发 `AgentEvent::Diagnostic{code:"checkpoint.snapshot_failed"}`（P2 片 2B，写入继续，不阻断 run）。

### 3.4 手动压缩与单轮会话

- `run_manual_compaction(provider, request, turn, events, cancel, loop_ctx, context) -> Result<Vec<Message>, EngineError>`：REPL `/compact` 等入口。不是 run：不发 `RunStarted` / `RunCancelled`，事件序直接 `CompactionStarted → MessageCommitted(summary) → CompactionCompleted`（复用自动链同一内部函数，reason 为 `AutoCompactionReason::Manual`）。`messages.len() <= retained_messages` 时返回 `Err`（nothing to compact）。返回重建后的消息列表（summary + retained tail）。
- `run_session_turn(provider, request, turn, events, cancel)`：单轮事件化（无工具循环、无 TurnContext）：`RunStarted → MessageCommitted(user) → ContextPrepared(estimated=0) → ProviderRequestStarted → 流式事件 → MessageCommitted(assistant) → RunCompleted`。半轮取消 / 失败不提交未完成的助手消息。
- `SessionTurn { session_id, run_id, provider_id, model, start_sequence, trigger_message, timestamp }`；`SessionTurn::new` 以 `now_timestamp()` 取当前时间。

### 3.5 事件与错误

- `AgentEventSink::emit(AgentEventEnvelope) -> Result<(), EngineError>`：唯一事件出口，调用方 persist-first 再渲染。sequence 由 engine 内部 `EventEmitter` 从 `start_sequence` 起原子递增分配，信封 `event_id` 格式 `evt-{run_id}-{sequence}`。
- `EngineError` 三变体：
  - `Provider(ProviderError)`：provider 侧错误透传（`is_cancelled()` 判定其中的 Cancelled）；
  - `Sink(String)`：事件出口 / 前置校验失败（persist 失败、start_sequence 非法、nothing to compact 等）；
  - `MaxToolRounds(u64)`：工具轮数超限。
- `map_provider_event(&ProviderStreamEvent, &MessageId) -> Option<AgentEvent>`：单轮映射（TextDelta / ThinkingDelta / ToolCallStarted / ToolCallArgumentsDelta / UsageUpdated / ServerTool / TranscriptEnvelope）；未列出变体（ReasoningItem / ToolCallCompleted / ResponseStarted / ResponseCompleted / ProviderMetadata / Error）只缓冲给 `AssembledTurn` 不映射。
- `LoopEventEmitter`：`execute_tools` 期间发工具流事件（可 Clone，复制 sequence 与 sink 引用）。

`run_session` 事件发射总表（按可能出现的顺序）：

| AgentEvent | 发射时机 |
| --- | --- |
| `RunStarted` | run 开始，携带 trigger_message_id |
| `MessageCommitted` | 用户触发消息 / 每轮助手消息 / 每轮工具结果消息 / 压缩摘要消息 |
| `Diagnostic(resources.injected)` | 注入层非空时一次 |
| `ContextPrepared` | 每轮请求前（硬限截断后重发一次） |
| `CompactionStarted / CompactionCompleted` | 软限压缩链或手动压缩 |
| `Diagnostic(context_hard_truncated)` | 硬限截断发生时 |
| `ProviderRequestStarted` | 每轮调用 provider 前 |
| `AssistantTextDelta / AssistantThinkingDelta / ToolCallStarted / ToolCallArgumentsDelta / UsageUpdated / ServerTool / TranscriptEnvelope` | 流式转发（LoopSink 实时映射） |
| `ToolApprovalRequested` | LoopContext 实现等待前 emit；gate 长度违约时由 engine 补发 |
| `ToolApprovalResponded` | engine 对每个 Asked 决策补发 |
| `CheckpointCreated` | `snapshot_write_tools` 每个快照 |
| `ToolExecutionStarted / ToolOutputDelta / ToolExecutionCompleted` | 放行调用执行前 / 执行中 / 结果回填时 |
| `Diagnostic(sandbox.fallback)` | 工具结果 metadata 声明沙箱回退时 |
| `Diagnostic(checkpoint.snapshot_failed)` | 宿主在 `snapshot_write_tools` 内经 `LoopEventEmitter` 发（快照失败但写入继续） |
| `RunCompleted / RunCancelled / RunFailed` | 终态三选一（persist 失败时不补发） |

### 3.6 取消

- `CancelHandle::new(run_id, Arc<dyn ProcessTreeCleaner>)`；`token()` / `child_token()` 返回同一根 `CancellationToken`（传给 Provider stream 与 Tool execute）。
- `cancel(reason: CancelReason) -> CancelReceipt`：原子门控幂等——首次调用 ① 取消根令牌 ② 触发 `cleaner.cleanup(run_id)` 杀树并记录 `processes_killed`；重复调用返回 `already_cancelled: true` 且不重复清理。`CancelReason::{User, Budget, System, Shutdown}` 仅供调用方写事件 / 日志，不进 `RunCancelled` 信封。
- 生产杀树由宿主注入 `pawork-exec` 侧实现；engine 默认 `NoopProcessTreeCleaner`，不自建 `run_id → 进程` 登记表。

### 3.7 context 子模块

- `ContextBudget::from_context_window(window, output_reserve, thinking_reserve)`：`max_input_tokens = window - reserves`（饱和到 0）；默认 128k/4k/0。serde 形状与 V1 一致（冻结）。
- `compute_compaction(&ContextBudgetBreakdown, history_soft_limit) -> Option<CompactionTrigger>`：`estimated_input_tokens > max_input_tokens` → `InputBudgetExceeded`（优先）；否则 `history_tokens > soft` → `HistorySoftLimit`；否则 None。
- `TokenEstimator`：`count_text` 唯一必须实现的核心（另有 `estimator_kind`）；`count_message`（+4 framing）/ `count_content_part`（图片 85 placeholder；Reasoning 只数 summary）/ `count_tool_schemas`（JSON 序列化 + 每工具 +8）均为默认实现。`HeuristicEstimator::new(chars_per_token)`（默认 4；CJK 类字符恒按 1 字符/token）。
- `trim_tool_result(_with)(&ToolResultContent, &TrimThresholds, [TrimStrategy])`：按字节分级——Small 完整保留；Medium 头尾各 2 KiB + 截断说明；Large 摘要 + `ArtifactRef` 占位（占位 id `artifact:trimmed-tool-result`）；Huge 仅 `ArtifactRef`。原文经 `TrimmedToolResult::retained_full` 暂存，写 Blob 由调用方负责。
- `TurnContext { limits, estimator, retained_messages（默认 4）, injected_layers }`：`Default` 全禁用，行为与未接线时完全一致（估算 0、不压缩、不截断、不注入）。

## 4. 核心行为与数据流

### 4.1 一次 `run_session` 的完整工具循环

1. 校验 `start_sequence >= 1`；创建 `EventEmitter`（原子 sequence 分配器）。
2. 发 `RunStarted { trigger_message_id }` → `MessageCommitted`（用户触发消息）。已取消则发 `RunCancelled` 返回。
3. 注入资源层：`injected_layers` 非空时拼为一条 `System` 消息（固定 id `msg-resources`，格式 `[kind] resource_id\ncontent` 以空行连接）插到消息最前（幂等：先移除同 id 旧条目），并发 `Diagnostic { code: "resources.injected" }`（含每层 byte_len）。
4. **每轮循环**：
   1. 取消检查（命中 → `RunCancelled` + `Err(cancelled)`）。
   2. 估算输入 token 并发 `ContextPrepared { message_count, estimated_input_tokens }`（estimator 未配置时 estimated 恒 0）。
   3. 上下文收敛（须同时配置 limits 与 estimator）：软限命中先走压缩链（见 4.2）并重建消息 + 重注入资源层 + 重估算；压缩后仍超硬限、或纯硬限（软限未命中，压缩无收益）时 `truncate_for_budget` 从最旧非 System 消息开始丢弃（永不丢最后 `retained_messages` 条），发 `Diagnostic { code: "context_hard_truncated" }` 并重发 `ContextPrepared`。
   4. 发 `ProviderRequestStarted` → 内部 `run_turn`（`LoopSink` 把每个 `ProviderStreamEvent` 映射为 AgentEvent 实时转发 + 原样缓冲；sink persist 失败被记录并优先返回，不再补终态事件）。
   5. 成功：`AssembledTurn` 折叠缓冲事件为助手消息（metadata 带 usage / stop_reason / provider / model），发 `MessageCommitted(assistant)`，usage 累计入 run。无 tool call → 发 `RunCompleted`（usage 为 run 累计）并返回。
   6. 有 tool call：调 `request_approval(invocations, run_approved, ...)`。返回后再查取消。**gate 数与调用数不匹配 → 协议违约，fail-closed**：全部按 `Denied` 处理、不执行任何调用，且由 engine 补发每个调用的 `ToolApprovalRequested`；正常路径由 `apply_approval_gates` 求出放行集（`NotRequired` 直接放行；`Asked(ApprovedOnce | ApprovedForRun)` 放行，`ApprovedForRun` 置位 run 级记忆，此后同 run 的非拒绝决策自动升级为 `ApprovedForRun`）。对每个 `Asked` 决策发 `ToolApprovalResponded { decision }`。
   7. `snapshot_write_tools(to_run)` → 每个快照发 `CheckpointCreated`。
   8. 每个放行调用发 `ToolExecutionStarted` → `execute_tools`（空集则跳过）→ 再查取消。Denied/Cancelled 且未被执行的调用回填拒绝结果（`ErrorCategory::Authorization`，文本 "tool call denied by user"）→ `align_tool_results` 按序对齐补缺 → 逐个发 `ToolExecutionCompleted`；结果 metadata 带 `sandbox.fallback = true` 时追加 `Diagnostic { code: "sandbox.fallback" }`。
   9. 构建 `Tool` 角色消息并发 `MessageCommitted(tool)`；把助手消息 + 工具消息追加进请求、换取新 `request_id`，`tool_rounds += 1`；达 `max_tool_rounds` → `RunFailed` + `Err(MaxToolRounds)`。
   10. provider 返回 Cancelled → `RunCancelled`（usage 合并流内最后一条 `UsageUpdated`）；其它 ProviderError → `RunFailed`（`ErrorContext::from(error)`）。
5. 循环直到无 tool call、超限、取消或出错。

### 4.2 压缩链（自动软限 / 手动共用 `compact_messages`）

1. `messages.len() <= retained_messages` → 不压缩（自动路径静默跳过，手动路径在入口即报错）。
2. 切分：前段 = 被压缩区间，尾段 = 最后 `retained_messages` 条。
3. `summarize_history`：向 provider 发**内部**摘要请求（`assemble_request`、无 tools、固定 User 指令前缀）；该请求不进 `AgentEventSink`、usage 不计入 run。失败或空摘要时降级结构性摘要（首条 User 消息截 2000 chars + 最后一条截 500 chars）。
4. `LoopContext::compact_history(reason, summary_text)`：`Err` → 终止当前 run；`Ok(outcome)` 提供持久化水位。
5. 事件三连：`CompactionStarted { source_event_count }`（host 回传值，无 outcome 时用被压缩消息数）→ `MessageCommitted`（summary 为 User 角色新消息）→ `CompactionCompleted { summary_message_id, compacted_through }`（无 outcome 时 `compacted_through = 0`，fail-safe：无持久化水位不折叠任何已投影消息）。
6. 重建消息列表 = `[summary] + retained tail`（自动路径随后重注入资源层前缀）。

### 4.3 取消传播

`CancelHandle.cancel()` → 根 token 取消 + cleaner 杀树；`run_session` 在轮首、审批返回后、工具执行后三处检查并发 `RunCancelled`；`run_turn` 在调用 provider 前做预取消检查（已取消则不调 provider）。工具内部与 provider 流内取消由同一把 token 传播。

### 4.4 流式折叠规则（`AssembledTurn::apply`）

- `TextDelta` / `ThinkingDelta` 追加进 text / thinking 缓冲；`ReasoningItem` 追加进列表。
- `ToolCallStarted` 建立 `PendingToolCall` 并记录顺序（重复 id 忽略）；`ToolCallArgumentsDelta` 追加 raw JSON——若早于 Started 到达则容错补建空名调用；`ToolCallCompleted` 置 completed 标记。
- `UsageUpdated` / `ResponseStarted` / `ResponseCompleted` / `ProviderMetadata` 增量合并进 `ModelResponseSummary`；`ServerTool` / `TranscriptEnvelope` / `Error` 不参与折叠。
- `into_message` 产出顺序固定：Thinking → Reasoning items → Text → ToolCall（按出现顺序）；参数 JSON 解析失败降级 `Value::Null`。

## 5. 契约与不变量

- **审批事件对（K-02，冻结）**：`ToolApprovalRequested` 由 `request_approval` 实现方在每次阻塞等待前 emit（reason 逐字 ``tool `{name}` requires approval``）；`ToolApprovalResponded` 由 engine 补发。等待审批期间取消 → 有 Requested 无 Responded 是合法事件序。审批经宿主的 ApprovalResolver 体系 await，engine 不感知具体审批 UI。
- **事件可持久化可重放**：所有事件带连续 sequence（从 `start_sequence` 起）；sink persist 失败后 engine 不再补发终态事件（磁盘停在最后一条成功 append，恢复由重放完成）。
- **不按 Provider 名称分支**：`src/` 出现任何已知 provider 名串即红线违规（`tests/no_provider_branch.rs` 守护，名单自 `CHANNEL_REGISTRY` 派生 + 基线别名）。压缩 = 重写前缀 = 缓存失效，不做任何厂商 cache 特例。
- **依赖红线**：生产依赖唯一 `pawork-*` 为 `pawork-domain`（`tests/domain_only.rs` 守护）；不依赖 tools / exec / storage / policy；`ProcessTreeCleaner` / 工具执行 / 落库全部由宿主注入。
- **`run_turn` 透传**：`ProviderStreamEvent` 13 变体全部由 provider 发射、sink 原样接收，engine 不滤不删；预取消时不调 provider 直接 `ProviderError::cancelled`。
- **压缩失败即失败**：`compact_history` 的宿主错误必须终止当前 run；无持久化 outcome 时水位只能为 0，不得拿摘要事件自身 sequence 代替。
- **fail-closed 审批**：gate 向量长度与调用数不匹配时全部按 Denied 处理，不执行任何工具。
- **截断保底**：硬限截断永不丢最后 `retained_messages` 条，也不丢 System 消息。
- **usage 口径**：run 级累计为饱和加法；内部摘要请求的 usage 不计入；全零 usage 在终态事件中省略为 None。

## 6. 依赖关系

- **生产依赖**：`pawork-domain`（唯一 pawork-*）；`async-trait` / `serde` / `serde_json` / `thiserror`。
- **dev-dependencies**：`tokio`（macros/rt-multi-thread/sync）、`futures`、`pawork-testkit`、`pawork-providers`（仅供 `no_provider_branch` 守护名单从 `CHANNEL_REGISTRY` 派生）。
- **被依赖**：`pawork-app`（宿主装配 sink / LoopContext / provider）、`pawork-cli`（chat 取消与渲染）。desktop 依赖 deny-list 含本包（GUI 不得直接加载）。

## 7. 测试与验证资产

| 资产 | 覆盖点 |
| --- | --- |
| `tests/domain_only.rs` | **红线**：生产依赖 domain-only 断言（含 alias / target 表解析的自测试） |
| `tests/no_provider_branch.rs` | **红线**：src/ 无 provider 名分支；守护名单派生自 `CHANNEL_REGISTRY` 且包含 chatgpt/xai/glm-coding/opencode-go/qwen-token-plan/deepseek 等首发通道 |
| `lib.rs` 内联测试 | 装配默认值冻结断言；`run_turn` 透传（含 Thinking/ToolCallStarted 等变体）、预取消不调 provider、流中取消 |
| `session_turn.rs` 内联测试 | 单轮事件序 golden、预取消 / 流中取消 / provider 错误 / persist 失败中断且续跑接续 sequence |
| `tool_loop/tests.rs`（24 个，原内联测试整文件迁入） | 多轮循环（`mock_provider_completes_multi_turn_tool_loop`）、并行只读工具、工具失败回填续跑、`max_tool_rounds_emits_run_failed_without_extra_stream`、审批事件对与 ApprovedOnce/ForRun/Denied（`approval_event_pair_then_execute_on_approved_once`、`short_approval_gates_fail_closed_without_executing`、`denied_fills_tool_result_and_continues_without_executing`、`approved_for_run_remembers_across_tool_rounds`）、取消（长工具中取消、`cancel_while_waiting_for_approval_emits_requested_without_responded`）、S5 上下文（默认关闭现状、注入层、软限压缩、硬限截断、`compaction_outcome_metadata_flows_into_events`、`compact_history_error_fails_the_run_instead_of_being_swallowed`、手动压缩两例、长对话恒不超硬限）、checkpoint（快照先于执行、回滚追加事件）、sandbox fallback 诊断 |
| `appender.rs` / `cancel.rs` / `context/*` 内联测试 | 流式折叠、取消幂等与杀树计数、预算推导 / 触发优先级 / 估算口径 / 裁剪分级边界 |

默认验证命令：`cargo test -p pawork-engine --offline --lib --tests`。

## 8. 注意事项与已知限制

- `run_turn` 为 `pub(crate)`（R0 D12 公开面收口）；外部只能走 `run_session` / `run_session_turn`。
- `TurnContext::default()` 全禁用：不配置 limits + estimator 时无估算（`estimated_input_tokens = 0`）、无压缩、无截断——上下文管理是 opt-in。
- 精确 tokenizer（tiktoken）刻意不迁入本包；需要时由宿主实现 `TokenEstimator` 注入，默认只有 `HeuristicEstimator`。
- tool result 裁剪（`tool_result_trim`）是独立纯函数工具，`run_session` 本体并不自动调用；由宿主在工具结果入上下文前使用，Blob 写入与真实 `ArtifactId` 替换也由调用方完成。
- `AssembledTurn` 对 `ToolCallArgumentsDelta` 早于 `ToolCallStarted` 到达的乱序容错（补建空名调用）；参数 JSON 解析失败时降级 `Value::Null`，不报错。
- 结构性摘要（降级路径）只截取文本 part，非文本内容不进入摘要。
- `ToolStreamEvent::Progress` 与 `ArtifactAvailable` 目前在 `LoopEventEmitter::emit_tool_event` 中被静默忽略，不映射为 AgentEvent。
- 相关文档：[architecture](../../architecture.md) · [design](../../design.md) · [flows](../flows.md) · [Spec 总览](../README.md) · [AGENTS.md](../../../AGENTS.md)；相邻包：[domain.md](domain.md) · [app.md](app.md) · [providers.md](providers.md)。
