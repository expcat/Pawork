# pawork-testkit

> 可编程 Mock Provider / Mock Tool 与 Provider 流断言辅助：只依赖 `pawork-domain`，被 engine / tools / app / client 以 dev-dependency 引用，不进入任何生产二进制闭包。

## 1. 职责与边界

- 为依赖 `ModelProvider` / `AgentTool` 契约的测试提供确定性替身：脚本化的 `MockProvider`、固定结果的 `MockTool`、记录型事件 sink，以及一组 Provider 流形状断言。逻辑回归以 Mock 驱动，真实 Provider API 只承担冒烟（见 [../verification.md](../verification.md)）。
- **不做**：真实网络/进程/文件 IO、fixture 管理、golden 比对框架；不依赖 `pawork-engine` 等上层包（避免循环）。P15 的 capability / server_tool / citation / reasoning 断言未迁入本包（`contract.rs` 头注释明示）。
- 本包不是协议 golden 的家——契约 golden 分别在 `pawork-domain` / `pawork-protocol` / `pawork-storage`。

## 2. 模块与文件地图

| 路径 | 行数量级 | 承载内容 |
| --- | --- | --- |
| `src/lib.rs` | ~750（约后半为 `#[cfg(test)]` 自测） | `MockScript`（Provider 事件脚本 builder）、`MockProvider`（实现 `ModelProvider`；Replay/Sequence 两种脚本源 + 调用记录）、`MockProviderCallRecord`、`MockTool`（实现 `AgentTool`）、`MockToolCallRecord`、`RecordingProviderSink` / `RecordingToolSink`（记录型 sink）、`assert_provider_request_order`；re-export `contract` 断言到 crate 根 |
| `src/contract.rs` | ~140 | Provider 流最小断言（不绑定具体 Provider，不按 Provider 名分支）：`assert_text_stream`、`assert_single_tool_call`、`assert_parallel_tool_calls`、`count_variant` |

无 `tests/` 目录；自测全部内嵌于两个源文件。

## 3. 对外 API 面

### 3.1 MockScript（脚本 builder，链式）

| 方法 | 语义 |
| --- | --- |
| `response_started(id)` | 追加 `ResponseStarted{response_id}` |
| `text(t)` / `thinking(t)` | 追加 `TextDelta` / `ThinkingDelta` |
| `tool_call(name, args)` | 自动编号 `mock-tool-call-N`，参数一次成串（Started → 单条 ArgumentsDelta → Completed） |
| `tool_call_chunks(id, name, chunks)` | 显式 `ToolCallId` + 多片 JSON 分片，保序发出 |
| `usage(TokenUsage)` | 追加 `UsageUpdated`（同时进 summary） |
| `provider_metadata(Value)` | 追加 `ProviderMetadata`（同时进 summary） |
| `complete()` / `complete_with(stop_reason)` | 追加 `ResponseCompleted`（默认 `StopReason::Completed`） |
| `fail(ProviderError)` | 走到此步立即以该错误终止（其后步骤不执行） |
| `wait_for_cancellation()` | 挂起等待 token 取消，然后返回 `Cancelled` 错误 |

### 3.2 MockProvider / MockTool

- **`MockProvider`**：`new(script)` 同一脚本可重复 replay；`sequence(vec![脚本])` 逐请求原子消耗，耗尽后返回 `ProviderErrorKind::StreamInterrupted`（message 含 "mock script sequence exhausted"）；`with_id(ProviderId)`（默认 `"mock"`）、`with_models(Vec<ModelDefinition>)`（供 `list_models` 返回，默认空）；`calls()` 返回 `MockProviderCallRecord{request_id, model, event_count, cancelled, completed}` 快照。
- **`MockTool`**：`new(name, ToolResult)` 生成默认 descriptor——`ToolCapability::ReadOnly`、`ToolKind::ClientFunction`、`ToolHosting::Local`、`requires_approval: false`、`read_only: true`、`supports_concurrency: true`、`default_timeout_ms: Some(1000)`、`max_output_bytes: 64 KiB`、`allowed_in_untrusted_workspace: true`、`input_schema: {"type":"object"}`；`failing(name, ToolError)` 固定失败；`with_descriptor` 整体替换；`calls()` 返回 `MockToolCallRecord{tool_call_id, input, workspace_id, run_id, cancelled}`；`assert_called_with(&[input])` 断言输入序列。
- **Sink**：`RecordingProviderSink::events()` / `RecordingToolSink::events()` 返回捕获的事件向量（内部 `Arc<Mutex<Vec<_>>>`，克隆共享同一存储）。

### 3.3 断言函数

- `assert_text_stream`：至少一条非空 `TextDelta` 且末尾为 `ResponseCompleted`。
- `assert_single_tool_call`：存在 `ToolCallStarted` 且同 id 被 `ToolCallCompleted` 闭合。
- `assert_parallel_tool_calls`：≥2 个 `Started` 且各自闭合（可交错）。
- `count_variant(events, predicate)`：按谓词计数。
- `assert_provider_request_order(provider, &["request-1", …])`：按 `request_id` 比对 Provider 调用顺序。

## 4. 核心行为与数据流

1. **脚本回放**：`MockProvider::stream` 先登记调用（`calls` 追加记录），再取脚本（Replay 克隆同一份；Sequence 按 `AtomicUsize` 取下一份，耗尽即报错）。
2. **逐步执行**：每步之前检查 `cancel.is_cancelled()`——已取消则标记 `cancelled = true` 并返回 `ProviderError::cancelled`；`Event` 步先按事件更新 `ModelResponseSummary`（ResponseStarted → response_id、UsageUpdated → usage、ResponseCompleted → stop_reason 并标记 `completed`、ProviderMetadata → provider_metadata），再 `sink.emit` 并累加 `event_count`；`Fail` 步立即返回脚本错误；`WaitForCancellation` 步 `cancel.cancelled().await` 挂起。
3. **收尾校验**：脚本走完但没有 `ResponseCompleted` → 返回 `StreamInterrupted`（"mock script ended without ResponseCompleted"），强迫测试脚本闭合，模拟真实 Provider 的流完整性要求。
4. **MockTool 执行**：`execute` 先记录调用（含取消位），已取消则返回 `ToolError::cancelled`；否则克隆返回预设 `Ok(ToolResult)` / `Err(ToolError)`。不经 sink 发任何 `ToolStreamEvent`。

## 5. 契约与不变量

- Mock 行为严格贴合 domain 契约（见 [domain.md](domain.md) §3.3/§3.4）：事件顺序即脚本顺序、tool call 三段闭合（Started → ArgumentsDelta* → Completed）、取消与错误语义与真实 Provider 一致，Engine 测试可据此做形状断言。
- 断言只关心 canonical 流形状，不按 Provider 名称分支（与架构红线一致）。
- 无 golden / fixture / schema 资产；本包自身不承载冻结契约，契约事实源在 `pawork-domain`。
- 默认 `MockTool` descriptor 是"最宽松安全形状"（只读、免审批、允许 untrusted workspace），需要红线场景（审批、写权限、并发禁用）时用 `with_descriptor` 显式覆盖。

## 6. 依赖关系

- **内部**：仅 `pawork-domain`。
- **外部**：`async-trait`、`serde_json`；dev：`tokio`（macros / rt / sync / time，自测用）。无 feature。
- **下游（全部为 dev-dependencies）**：`pawork-engine`、`pawork-tools`、`pawork-app`、`pawork-client`——本包永不进入 `pawork` 二进制依赖闭包，也不得被写进 `apps/pawork` 依赖。
- 全景见 [../../architecture.md](../../architecture.md)；相关跨包链路见 [../flows.md](../flows.md)。

## 7. 测试与验证资产

无独立 `tests/` 目录；两文件各含 `#[cfg(test)]` 自测：

| 位置 | 覆盖点 |
| --- | --- |
| `src/lib.rs` tests | `single_script_text_tool_call_and_complete`（全链路 + 同脚本重复 replay）；`sequence_plays_two_scripts_then_exhausts`（顺序消耗与耗尽错误）；`tool_call_chunks_keep_partial_json_in_order`（分片保序 + 并行闭合）；`mock_tool_success_failure_and_cancellation`（三态 + 默认 descriptor 全字段断言）；`provider_cancellation_is_recorded`；`fail_returns_scripted_error_immediately` |
| `src/contract.rs` tests | 三个断言函数与 `count_variant` 的通过路径 |

默认验证命令：`cargo test -p pawork-testkit --offline --lib --tests`。

## 8. 注意事项与已知限制

- `MockProvider::sequence` 耗尽后的错误 kind 是 `StreamInterrupted`（该 kind 默认 retryable），"预期失败"的测试要按 message 匹配而非只看 kind。
- `MockScript::fail` 之后的步骤不会执行；把 `fail` 放中间即可模拟"流中断"。
- `MockTool` 忽略 `ToolEventSink`（`_sink`）；需要测 `ToolStreamEvent` 流出时须自写工具替身。
- 断言函数 panic 式失败（`assert!`），仅供测试上下文使用。
- P15 引入的 capability / server_tool / citation / reasoning 断言辅助未迁入，需要时在使用方测试内自写。
- 相关文档：[domain.md](domain.md) · [protocol.md](protocol.md) · [../README.md](../README.md) · [AGENTS.md](../../../AGENTS.md)。
