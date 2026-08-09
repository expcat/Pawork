# Phase 3 Review：Agent Loop 主干

- **日期**：2026-08-08
- **评审基线**：`main` @ `67d6c4d`（工作树除 `REVIEW-P2.md` 未跟踪外干净）
- **状态**：草案（仅记录结论与建议，未修改任何代码/配置；后续再研究是否采纳）
- **范围**：ROADMAP.md Phase 3「Agent Loop」的 10 个任务（P3-1 ~ P3-10）的完成情况、所引入包是否合适、基线偏差；漏洞与优化点一并列出。Phase 3 是「关键路径」中 Agent Loop 主干，Phase 4（工具/权限）、Phase 5（Session/压缩）、Phase 7（Git）均建在其上，受影响处在文中标注「传播面」。

### 1. 结论摘要

1. **测试全绿，但「绿」的含金量低于 Phase 1/2**：4 个交付 crate（`agent-engine` / `context-engine` / `tool-runtime` / `agent-events`）复跑共 **89 passed / 0 failed**（agent-engine 50 单元 + 1 集成；context-engine 26；tool-runtime 9；agent-events 3）；`clippy -D warnings`、`fmt --check`、`schema-typegen --check` 均干净。但 89 项测试几乎全是**单模块自测**，没有任何一项覆盖「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试」的真实组合。
2. **核心问题：组件齐全，主干未接线**。P3-1~P3-10 各自实现良好，但作为「主干」的 `ProviderLoop` 只真正组合了 3 个兄弟模块：状态机（P3-1）、部分预算（P3-6）、部分事件广播（P3-9）。`MessageQueue`（P3-5）、`RetryController`（P3-7）、`CancelHandle`+进程树清理（P3-8）在 `provider_loop.rs` 中**零引用**——它们被 `pub use` 导出却从未进入循环。模块头注释自称「组合状态机、预算控制、消息队列、事件广播」（[provider_loop.rs:4-5](../../crates/agent-engine/src/provider_loop.rs)），与实现不符。
3. **四项「mock 过得去、真实运行会暴露」的高危缺口**：①取消/预算耗尽两条终止路径只转状态机、不发 `RunCancelled`/`RunFailed` 事件（V1）；②Provider 流式增量被 `LoopSink` 全量缓冲、从不广播，GUI/CLI 拿不到实时 token 流（V2）；③`provider.stream()` 调用没有任何重试包裹，P3-7 重试逻辑在 loop 层完全不生效（V3）；④崩溃恢复重放对「工具轮次」的状态机重建失真，产生虚假 `IllegalTransition`（V4）。
4. **包选型合理，tiktoken-rs 落地正确**：`tiktoken-rs` 0.6.0（P3-2）按基线「仅 OpenAI 系精确、其它启发式」正确实现；`tokio` 的 `broadcast`/`Semaphore`/`Mutex`/`Notify` 是标准原语。**没有「引用面小、应自实现替换」的包**，也没有「自实现却应引包」的反例（状态机/重试/预算/队列按基线属「完全自实现」）。Phase 3 crate 未引入任何新依赖。
5. **基线偏差小但流程偏差与 Phase 2 同病**：无新增「引入未登记」依赖；唯一的「声明未引用」是 `futures`（P2 遗留，agent-engine 成为第三个消费者，登记必要性更强）。10 篇 `plan/P3-*.md` **全部停留 `🟡未开始`、验收框全未勾**，违反 AGENTS.md §4（与 REVIEW-P2 §4 同一问题，提交未触碰 plan/）。
6. **P3-6 多维预算名不副实**：9 个预算维度中，Cost / Duration / Concurrency / ArtifactBytes 四个维度在 loop 中**零记录**（`record_cost`/`set_elapsed`/`set_concurrency`/`record_artifact` 从不调用），相关硬上限永远不可能触发；`soft_warnings` 被计算但从不翻译为事件，P3-6 验收「达预算产生事件、不静默停」对软阈值未满足。

### 2. P3 任务完成情况核对表

| 任务 | 交付 crate/模块 | 状态 | 关键证据 |
| --- | --- | --- | --- |
| P3-1 Run 状态机 | `agent-engine/state.rs` | 🟢 | 12 态全转换 + 非法转换防御 + 事件 hint 映射（[state.rs:155-200](../../crates/agent-engine/src/state.rs)）；8 项测试 |
| P3-2 上下文构建与预算 | `context-engine` | 🟢 | 14 来源确定性排序、tiktoken 精确 + 启发式回退、output/thinking reserve、超限触发 `CompactionTrigger`（[builder.rs](../../crates/context-engine/src/builder.rs)、[token.rs](../../crates/context-engine/src/token.rs)）；26 项测试 |
| P3-3 Provider Loop | `agent-engine/provider_loop.rs` | 🟢（有 V1/V2/V3） | 多轮工具循环、混合审批保序、状态机驱动（[provider_loop.rs](../../crates/agent-engine/src/provider_loop.rs)）；但重试/消息队列/进程清理未接入（见 §6） |
| P3-4 Tool Scheduler | `tool-runtime/scheduler.rs` | 🟢（有 V8/V9/V10） | 只读并发/写串行/同文件串行/Git index 串行/审批暂停/取消传播（[scheduler.rs](../../crates/tool-runtime/src/scheduler.rs)）；9 项测试；但上下文注入假值（V8）且未与 ProviderLoop 桥接（V9） |
| P3-5 消息队列 | `agent-engine/queue.rs` | 🟢（未接线，见 V7） | enqueue/replace_queued/drain、快照恢复、并发不丢（[queue.rs](../../crates/agent-engine/src/queue.rs)）；5 项测试；**ProviderLoop 未使用** |
| P3-6 预算控制 | `agent-engine/budget.rs` | 🟡部分（见 V5/V6） | 9 维预算 + 软/硬阈值；但 loop 仅记录 5 维、soft_warnings 不发事件（[budget.rs](../../crates/agent-engine/src/budget.rs)） |
| P3-7 重试 | `agent-engine/retry.rs` | 🟡部分（见 V3） | `RetryPolicy`/`RetryController` 实现 + 尊重 Retry-After；6 项测试；**ProviderLoop 未调用** |
| P3-8 取消 | `agent-engine/cancel.rs` | 🟡部分（见 V7） | `CancelHandle` + `ProcessTreeCleaner` trait + 原子门控；3 项测试；**ProviderLoop 用裸 CancellationToken，进程清理不触发** |
| P3-9 事件流式分发 | `agent-engine/broadcast.rs` | 🟢（有 V2） | `tokio::broadcast` 有界多订阅 + Lagged 背压 + <2ms 延迟基准（[broadcast.rs](../../crates/agent-engine/src/broadcast.rs)）；但流式增量不进入广播 |
| P3-10 Interrupted Run 恢复 | `agent-engine/recovery.rs` | 🟡部分（见 V4） | `scan_interrupted`/`replay_run`/`group_by_run`，<1s 重放基准（[recovery.rs](../../crates/agent-engine/src/recovery.rs)）；但工具轮次状态重建失真 |

**门禁证据（2026-08-08 复核）**：

- `cargo test -p agent-engine -p context-engine -p tool-runtime -p agent-events`：**89 passed / 0 failed**。
- `cargo clippy -p agent-engine -p context-engine -p tool-runtime -p agent-events --all-targets -- -D warnings`：干净。
- `cargo fmt --all -- --check`：干净（`FMT_EXIT=0`）。
- `cargo run -p schema-typegen -- --check`：TypeScript declarations up to date。
- 各任务 plan 文档（`plan/P3-*.md`）状态与验收勾选**均未同步**（§5、§8.2）。

### 3. 包选型评估

#### 3.1 建议保留（自实现不值得）

| 包 | 版本（Cargo.lock） | 使用点 | 使用面评估 | 结论 |
| --- | --- | --- | --- | --- |
| `tiktoken-rs` | 0.6.0 | P3-2（`token.rs`） | OpenAI 系精确 BPE 计数（`get_bpe_from_model`/`encode_ordinary`/`CoreBPE`），非 OpenAI 自动回退启发式（[token.rs:170-176](../../crates/context-engine/src/token.rs)、[token.rs:208-218](../../crates/context-engine/src/token.rs)），与基线约定完全一致 | **保留**；会拉入 `bstr`/`fancy-regex`/`ndarray` 等较重依赖，但 BPE 分词本就无法轻量自实现 |
| `tokio`（`broadcast`） | 1 | P3-9 | 多订阅者有界广播 + Lagged 背压，是「慢消费者不拖垮核心」的标准答案（[broadcast.rs:62-70](../../crates/agent-engine/src/broadcast.rs)） | **保留** |
| `tokio`（`Semaphore`） | 1 | P3-4 | 全局并发上限，`OwnedSemaphorePermit` drop 即释放 | **保留** |
| `tokio`（`Mutex`/`Notify`） | 1 | P3-5 | 消息队列的异步互斥与唤醒；`Notify` 的 permit 语义正确覆盖「释放锁后再 await」的唤醒竞态 | **保留** |
| `async-trait` / `serde` / `serde_json` / `thiserror` / `tracing` | 基线版本 | 全局 | 基础设施，无争议 | **保留** |

#### 3.2 需要重新评估的项

| 项 | 现状 | 建议 |
| --- | --- | --- |
| `futures` | workspace 声明（[Cargo.toml:68](../../Cargo.toml)），agent-engine 成为继 provider-runtime、provider-openai-compatible 之后**第三个**消费者（[agent-engine/Cargo.toml:14](../../crates/agent-engine/Cargo.toml)），但 ROADMAP「直接采用」基线表仍无此行（REVIEW-P2 §4 已记录） | **回填基线**（P2 遗留，agent-engine 强化其必要性）。零代码级问题，纯文档同步 |

#### 3.3 「自实现替换包」总体判断

针对「引用面小 → 自实现换取可控性」的命题：**P3 范围内没有命中的包**。真正需要关注的不是选型，而是**自实现的模块是否被正确接线**——状态机、重试、预算、消息队列、广播都是按基线「完全自实现（P3-*）」正确落地的，但其中重试/消息队列/取消句柄三块自实现产物在主干循环里是「建成但未通电」状态（见 §6 V3/V7）。这与 Phase 2 的「backon 声明未用 + ExponentialBackoff 死代码」是同一类问题在 Phase 3 的放大：组件质量高，集成度低。

### 4. 基线偏差清单

规则来源：ROADMAP「依赖选型基线」要求新增依赖同步回填基线表（[ROADMAP.md:14](../../ROADMAP.md)、[ROADMAP.md:58](../../ROADMAP.md)）。

| 类型 | 项 | 位置 | 说明 |
| --- | --- | --- | --- |
| 声明未引用（P2 遗留，强化） | `futures = "0.3"` | [Cargo.toml:68](../../Cargo.toml) | agent-engine 新增引用，消费者增至 3 个；ROADMAP 基线表仍缺此行 |
| 新增引入未登记 | — | — | Phase 3 四个 crate 的所有依赖均映射到既有 workspace 条目，**无新增偏差** |
| 流程偏差 | `plan/P3-*.md` 全部未同步 | 10 篇均 `🟡未开始`，验收框全未勾 | 与 REVIEW-P2 §4 同一问题；ROADMAP 状态列已 🟢，属「半同步」 |

**建议**：与 REVIEW-P2 §4 的基线清理任务合并执行——回填 `futures`、同步 10 篇 P3 plan 文档。

### 5. 漏洞与风险

按优先级排序；标号为稳定引用号（V1~V11）。

#### V1 [正确性·高] 取消/预算耗尽两条终止路径不发终态事件

[provider_loop.rs:186-217](../../crates/agent-engine/src/provider_loop.rs) 的 `run()` 有四条终止路径，但只有「通用错误」与「成功」两条发终态事件：

- 取消（预检，L187-190）：`transition(Cancel)` 后直接 `return Err(Cancelled)`，**无 `RunCancelled` 事件**。
- 预算耗尽（L192-196）：`transition(Fail)` 后直接 `return Err(BudgetExceeded)`，**无 `RunFailed` 事件**。
- 流中取消（L201-204、L205-210）：同样 `transition(Cancel)` 后 `return`，**无 `RunCancelled` 事件**。
- 通用错误（L211-217）：正确调用 `emit_terminal_payload(RunFailed)`。

后果：被取消或因预算停止的 Run，其持久化事件流**以非终态事件结尾**，违反「每次转换都有事件」契约（P3-1 验收）与 ADR-016「状态可由事件序列重建」——重建出的状态会停留在 `ExecutingTools`/`StreamingResponse` 等活跃态而非 `Cancelled`/`Failed`。**传播面**：Phase 5 session 投影、Phase 13 GUI 的 Run 状态展示均依赖终态事件。测试 `cancelled_run_emits_cancelled_and_returns_error`（[provider_loop.rs](../../crates/agent-engine/src/provider_loop.rs)）名字声称「emits_cancelled」却只断言 `state==Cancelled`、不断言事件被广播，与该缺口正好叠加（V11）。**建议**：四条终止路径统一经 `emit_terminal_payload` 补发对应事件，并改测试断言订阅者收到 `RunCancelled`。

#### V2 [正确性·高] Provider 流式增量被全量缓冲、从不广播

`LoopSink`（[provider_loop.rs:575-595](../../crates/agent-engine/src/provider_loop.rs)）实现 `ProviderEventSink::emit` 时只把每个 `ProviderStreamEvent` push 进 `Mutex<Vec>`，整轮流结束后由 `AssembledTurn::apply` 一次性消费成一条助手消息，再以单条 `MessageCommitted` 广播。`AgentEvent` 枚举里定义了 `AssistantTextDelta`/`AssistantThinkingDelta`/`ToolCallArgumentsDelta`/`ToolOutputDelta`/`ToolCallStarted`（[agent-events/lib.rs](../../crates/agent-events/src/lib.rs)），**但 ProviderLoop 从不 emit 这些变体**——它们对订阅者是不可见的。后果：GUI/CLI 无法做「逐 token 流式显示」，P3-9「事件流式分发」实际只分发生命周期事件（RunStarted/MessageCommitted/...），<2ms 延迟基准（[broadcast.rs](../../crates/agent-engine/src/broadcast.rs)）测的也不是 token 流。**建议**：让 `LoopSink` 在缓冲的同时把 delta 事件 fan-out 到 `EventBroadcaster`（或引入双写 sink），`AgentEvent` 的 delta 变体即为此设计，缺的只是接线。

#### V3 [正确性·高] 重试逻辑完全未接入 Provider Loop

`retry.rs` 实现了完整的 `RetryPolicy`/`RetryController`/`RetryDecision`（含尊重 `Retry-After`、指数退避、6 项测试），但 `provider.stream(...)` 调用（[provider_loop.rs:259](../../crates/agent-engine/src/provider_loop.rs)）**没有任何重试包裹**——全仓库 `RetryController`/`RetryPolicy` 仅在 `retry.rs` 自身与 `lib.rs` 的 `pub use` 出现，`provider_loop.rs` 零引用。P3-7 验收「断流可重试、上下文不丢」在 loop 层完全不成立：一次 `StreamInterrupted`/`Network`/`Timeout` 错误直接走 L211-217 的通用错误路径，Run 标记 `Failed` 终止，无重试。**建议**：在 `run_turn` 的 `provider.stream()` 外层包 `RetryController`，断流时保持 `messages` 不变重发（断流重试语义），并把每次 `RetryAttempt` 翻译为 `AgentEvent::Diagnostic` 以满足「重试与事件一致性」。

#### V4 [正确性·高] 崩溃恢复重放对「工具轮次」状态机重建失真

[recovery.rs:60-90](../../crates/agent-engine/src/recovery.rs) 的 `replay_run` 用事件流重建状态机，但 `StreamFinished` 转换的 `EventHint` 为 `None`（[state.rs](../../crates/agent-engine/src/state.rs) `event_hint`），循环**不为它持久化任何事件**。因此重放一个含工具的轮次时：

1. `ProviderRequestStarted` → 状态到 `StreamingResponse`（L66）；
2. 助手 `MessageCommitted` 到达时状态仍是 `StreamingResponse`，L79-80 的 `if state==CollectingToolCalls` 判定为假 → 不推进；
3. `ToolApprovalRequested`（L67 → `ApprovalRequested`）在 `StreamingResponse` 上是**非法转换**（`ApprovalRequested` 仅合法于 `CollectingToolCalls`）→ 产生 `IllegalTransition` issue；
4. 后续 `ToolExecutionStarted`/下一轮 `ProviderRequestStarted` 依次全部非法，状态机**永久卡在 `StreamingResponse`**。

后果：任何含工具调用的 Run 重放后会堆积虚假 `IllegalTransition`（误导运维），`recovered_state` 路径错误（仅因「非终态」被归为 `Interrupted`），直接违反 ADR-016「状态可由事件序列完全重建」。当前 6 项 recovery 测试（[recovery.rs](../../crates/agent-engine/src/recovery.rs)）**无一包含工具轮次**（事件夹具里没有 `ToolApprovalRequested`/`ToolExecutionStarted`），缺口被完全遮蔽。**建议**：要么为 `StreamFinished{has_tool_calls:true}` 增加可持久化事件标记，要么让重放从助手 `MessageCommitted` 的消息内容（是否含 `ToolCall` part）推断 `CollectingToolCalls`；并补一条「工具轮次重放无 issue」的回归测试。

#### V5 [正确性·中] 软预算警告从不产生事件

[budget.rs:171-216](../../crates/agent-engine/src/budget.rs) 的 `check()` 会计算 `soft_warnings`（达 80% 默认软阈值的维度），但 `provider_loop.rs` 只在 L193 检查 `report.must_stop()`（硬上限），`soft_warnings` 计算后被丢弃，从不翻译为 `AgentEvent::Diagnostic`。P3-6 验收「达预算产生事件、不静默停」对**软阈值**未满足——用户永远收不到「已用 80% 预算」的预警，只能等到硬上限直接 Failed。**建议**：每轮 `tick_iteration` 后若 `!report.soft_warnings.is_empty()` 则 emit `Diagnostic`，并在首次触发某维度软阈值时记录避免重复刷屏。

#### V6 [正确性·中] 9 维预算中 4 维恒不触发

[provider_loop.rs](../../crates/agent-engine/src/provider_loop.rs) 实际只调用了 5 个预算记录方法：`tick_iteration`（L192，Iterations）、`record_tokens`（L262，Input/OutputTokens）、`record_tool_call`（L339，ToolCalls）、`record_output`（L368，OutputBytes）。其余 4 个——`record_cost`（Cost）、`set_elapsed`（Duration）、`set_concurrency`（Concurrency）、`record_artifact`（ArtifactBytes）——**在 loop 中零调用**。后果：这四个维度的硬上限（如 `max_duration_ms`/`max_cost_micros`）配置了也永远不会触发，`BudgetController` 的多维承诺名不副实。**传播面**：Phase 14 额度监控依赖 cost 维度，当前无法从 loop 获得费用累计。**建议**：loop 入口记录起始 `Instant` 每轮 `set_elapsed`；token 记录处用 model-registry 定价 `record_cost`；artifact 工具结果处 `record_artifact`。

#### V7 [集成·中] MessageQueue 与 CancelHandle 未接入 Provider Loop

P3-5 的 `MessageQueue`（replace queued / 快照恢复 / 并发不丢，5 项测试）与 P3-8 的 `CancelHandle`（根令牌 + `ProcessTreeCleaner` + 原子门控，3 项测试）都是高质量自实现，但 `provider_loop.rs` 对二者**零引用**：loop 用裸 `CancellationToken`（[provider_loop.rs:179](../../crates/agent-engine/src/provider_loop.rs)），不持有 `CancelHandle`；循环是单消息驱动的 `run()`，不消费 `MessageQueue`。后果：①运行中用户新消息无处入队（P3-5「运行中可发送」未落地）；②取消不触发进程树清理，P3-8 验收「Cancel 不留下运行进程」在 loop 层无法成立。**建议**：`ProviderLoop::run` 改为消费 `MessageQueue`（每轮 `drain_one` 决定是否续跑），并接收 `CancelHandle` 取代裸 token，使 `cancel()` 联动 `ProcessTreeCleaner`。

#### V8 [正确性·中] ToolScheduler 向工具注入假 workspace/run 上下文

[scheduler.rs:259-265](../../crates/tool-runtime/src/scheduler.rs) 构造 `ToolExecutionContext` 时硬编码 `WorkspaceId::from("default")`、`RunId::from("default")`、`working_directory: None`。文件类工具（Phase 4 的 read_file/write_file 等）依赖真实 `workspace_id` 解析相对路径、依赖 `working_directory` 确定 cwd，拿到 `"default"` 会解析到错误位置或失败。调度器签名也未暴露注入入口（`execute_named` 无 context 参数）。**建议**：`ToolScheduler::new` 或 `execute_named` 增加 `ToolExecutionContext` 来源（由 Run 携带真实 workspace/run），避免 Phase 4 工具接入时再返工。

#### V9 [集成·中] ProviderLoop 与 ToolScheduler 双轨、从未组合

`ProviderLoop` 通过自定义 `LoopContext` trait 注入工具执行与审批（[provider_loop.rs:36-54](../../crates/agent-engine/src/provider_loop.rs)），而 P3-4 的 `ToolScheduler` 是另一套独立的并发/串行/审批实现（[scheduler.rs](../../crates/tool-runtime/src/scheduler.rs)）。两者**从未组合**：没有「`LoopContext` 适配到 `ToolScheduler`」的桥接，调度器的 capability 串行、同文件串行、Git index 串行策略从未被真实 loop 走过。模块头注释（[provider_loop.rs:7-8](../../crates/agent-engine/src/provider_loop.rs)）自称「既可接 ToolScheduler 也可 Mock 注入」，但该适配器不存在。**建议**：在 app-service 或 agent-engine 内提供 `SchedulerLoopContext` 桥接，并加一条端到端测试（loop + scheduler + 真 capability 冲突场景）。

#### V10 [正确性·低] ToolScheduler.execute() 从 input.name 取工具名，语义错误

[scheduler.rs:233-245](../../crates/tool-runtime/src/scheduler.rs) 的 `execute()` 从 `request.input.get("name")` 反查工具，而工具名本应来自模型 tool_call 的 `.name`（`PendingToolInvocation.name`），不应藏在 `input` JSON 里。这既与 `execute_named` 的语义重复，又会与「工具自身 input schema 合法含 `name` 字段」冲突。**建议**：废弃 `execute()` 或改为只接受显式工具名；统一走 `execute_named`。

#### V11 [健壮性·低] LoopSink 缓冲整轮流 + 测试名过实

- [provider_loop.rs:575-595](../../crates/agent-engine/src/provider_loop.rs)：`LoopSink` 把整轮 `ProviderStreamEvent` 全量缓存进 `Vec`，超长生成（长 reasoning/大 tool arguments）的内存随 token 线性增长至轮结束。与 V2 的「不广播」同源——缓冲是为了事后组装，但组装完成后该 Vec 即可丢弃，当前确实在 `events()` clone 后释放，问题可控，仅记录。
- 测试 `cancelled_run_emits_cancelled_and_returns_error` 名字声称验证「emits_cancelled」，实际只断言 `state==Cancelled` 与 `Err(Cancelled)`，不断言 `RunCancelled` 事件被广播，给 V1 的漏发提供了虚假信心。

### 6. 优化建议（按优先级）

#### P0（建议在 Provider Loop 接入真实 Provider/工具前处理）

1. **V1**：四条终止路径统一补发 `RunCancelled`/`RunFailed` 事件（红线：终态可观察 + 可重建），并修测试断言。
2. **V3**：`provider.stream()` 外层包 `RetryController`，断流重试保持上下文——P3-7 的核心承诺，当前完全悬空。
3. **V4**：重放状态机对工具轮次的重建修复（增加 StreamFinished 事件标记或从消息内容推断），补工具轮次回归测试——ADR-016 重建承诺。

#### P1（近期排期）

4. **V2**：`LoopSink` 双写到 `EventBroadcaster`，让 delta 变体对订阅者可见——P3-9 流式分发的真正落地。
5. **V5 + V6**：soft_warnings 翻译为 `Diagnostic` 事件；补齐 cost/elapsed/concurrency/artifact 四维记录——P3-6 多维预算名副其实。
6. **V7**：`ProviderLoop::run` 消费 `MessageQueue` 并接收 `CancelHandle`——P3-5/P3-8 接入主干。
7. **V8**：`ToolScheduler` 注入真实 workspace/run 上下文——Phase 4 工具接入的前置。
8. **V9**：提供 `LoopContext` → `ToolScheduler` 桥接 + 端到端测试——打通 Phase 3 内部双轨。
9. **文档同步**：10 篇 `plan/P3-*.md` 状态与验收勾选回填（AGENTS.md §4）；修订 `provider_loop.rs` 模块头注释使其与实际组合范围一致（V2/V3/V7 的注释失真）。

#### P2（顺手/评估项）

10. **V10**：废弃 `ToolScheduler::execute()` 的 input.name 取名路径。
11. **V11**：`cancelled_run_emits_cancelled...` 测试补事件断言；评估 `LoopSink` 是否可边组装边丢弃已消费 delta 以降峰值内存。
12. 预算耗尽被记为 `RunFailed`（[provider_loop.rs:194](../../crates/agent-engine/src/provider_loop.rs)）：与「真正的失败」混淆，建议引入独立终态或 `RunStopped{reason: budget}` 语义，便于 GUI 区分「正常预算停止」与「错误失败」。
13. `recovery.rs` 的 `group_by_run` 对每个 envelope `clone`（[recovery.rs:140-148](../../crates/agent-engine/src/recovery.rs)）：大事件流下有内存放大，可改为按 run 分组借用。
14. `tiktoken-rs` 0.6.0 会拉入 `fancy-regex`/`bstr`/`ndarray`：评估是否可惰性加载 tokenizer（仅 OpenAI 模型才需要），减少非 OpenAI 路径的编译/二进制体积。

### 7. 附录

#### 7.1 Phase 3 模块集成矩阵

| 子任务模块 | 被 ProviderLoop 使用？ | 说明 |
| --- | --- | --- |
| P3-1 状态机（`state.rs`） | ✅ | 循环驱动转换（[provider_loop.rs:182-184](../../crates/agent-engine/src/provider_loop.rs) 等） |
| P3-6 预算（`budget.rs`） | ⚠️ 部分 | 仅 5/9 维记录；soft_warnings 不发事件（V5/V6） |
| P3-9 广播（`broadcast.rs`） | ⚠️ 部分 | 仅生命周期事件；流式增量不广播（V2） |
| P3-5 消息队列（`queue.rs`） | ❌ | loop 零引用（V7） |
| P3-7 重试（`retry.rs`） | ❌ | loop 零引用（V3） |
| P3-8 取消（`cancel.rs`） | ❌ | loop 用裸 token（V7） |
| P3-4 调度器（`tool-runtime`） | ❌ | 经 `LoopContext` trait 隔离，无桥接（V9） |
| P3-10 恢复（`recovery.rs`） | ➖ | 独立重放路径，工具轮次失真（V4） |
| P3-2 上下文（`context-engine`） | ➖ | 独立 crate，未被 ProviderLoop 调用（Phase 8/13 接线） |

对照 Phase 1/2（各 crate 都有真实消费者），Phase 3 的四个 crate **全部是叶子 crate**：`rg` 全仓库无 app-service/cli-host/core-runtime 依赖 agent-engine / context-engine / tool-runtime（agent-events 仅被 agent-engine、context-engine 引用）。这意味着「测试绿」≠「系统可用」——Phase 13 CLI Host 装配前，主干循环从未被任何宿主真实驱动。

#### 7.2 plan 文档漂移清单

| 文件 | 状态字段 | 未勾验收框 |
| --- | --- | --- |
| plan/P3-1-run-state-machine.md ~ plan/P3-10-interrupted-run-recovery.md（共 10 篇） | 全部 `🟡未开始`（应为 🟢） | 合计 18 个 `- [ ]`，如 [plan/P3-1-run-state-machine.md:24-25](../../plan/P3-1-run-state-machine.md)、[plan/P3-10-interrupted-run-recovery.md:22-23](../../plan/P3-10-interrupted-run-recovery.md) |

与 REVIEW-P2 §7.2 同一问题：ROADMAP 状态列已更新为 🟢，但 plan/ 未跟进。Phase 3 的提交未触碰任何 `plan/P3-*.md`。

### 8. 建议的后续动作（本次未执行，供研究）

1. 对 V1/V3/V4 立项（主干可观察性 + 重试落地 + 重建正确性，均属 Phase 3 自身验收范围）。
2. Provider Loop 接线任务（V2/V5/V6/V7/V9）：把已建成但未通电的模块接入主干，建议作为 Phase 13 CLI Host 装配的前置或并行任务。
3. ToolScheduler 上下文注入（V8）：Phase 4 工具实现前完成，避免返工。
4. 基线 + 文档同步小任务（§4 + §7.2）：与 REVIEW-P2 的清理合并一次提交。
5. 端到端集成测试：建立一条「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试 + 恢复」的最小真实组合测试，弥补当前「全模块自测、零组合」的覆盖盲区。

---

*评审方法：以 `67d6c4d` 为基线，逐项核对 ROADMAP/plan 状态、源码与依赖清单，并复跑 Phase 3 相关 4 个 crate 的测试与静态门禁（test/clippy/fmt/schema-typegen）；对终止事件缺失、LoopSink 广播、重试接线、replay 状态重建等关键断言直接核对了 `provider_loop.rs`/`recovery.rs`/`state.rs` 的控制流；文中所有结论均给出文件与行号级证据。本文档仅为评审记录，不代表已批准的变更。*

---

## 修复记录（review-remediation）

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P3-1 ~ P3-10（与 P4-13 在 `scheduler.rs` 上下文注入上同根，序列协调：本任务先做 V8 上下文注入，P4-13 再做 V1 策略接线）

**最终目的**：把 [REVIEW.md](../../REVIEW.md) §3（Phase 3）评审指出的「组件齐全、主干未接线」状态收口——让 Provider Loop 真正组合状态机、预算、重试、消息队列、取消、广播与工具调度，使终态事件可观察、流式增量对订阅者可见、断流可重试、崩溃重放对工具轮次正确，并把 plan 文档漂移一并处理。

**涉及范围**：`agent-engine`（provider_loop/state/budget/retry/cancel/queue/broadcast/recovery）、`tool-runtime`（scheduler）、`agent-events`、`context-engine`、`plan/P3-*.md`

### 细分步骤（分组）

#### A. 终态事件与流式广播（V1 / V2）

1. **V1 终态事件补发**：`provider_loop.rs` 四条终止路径（预检取消、预算耗尽、流中取消、流中错误）统一经 `emit_terminal_payload` 补发 `RunCancelled`/`RunFailed`。目的：每次转换都有事件、状态可由事件序列重建。
2. **V2 流式增量广播**：让 `LoopSink` 在缓冲的同时把 delta 事件（`AssistantTextDelta`/`AssistantThinkingDelta`/`ToolCallArgumentsDelta`/`ToolOutputDelta`/`ToolCallStarted`）fan-out 到 `EventBroadcaster`（双写 sink）。目的：GUI/CLI 可逐 token 流式显示。

#### B. 重试与队列/取消接入主干（V3 / V7）

3. **V3 重试接入**：在 `run_turn` 的 `provider.stream()` 外层包 `RetryController`，断流时保持 `messages` 不变重发，每次 `RetryAttempt` 翻译为 `Diagnostic`。目的：P3-7「断流可重试」真正落地。
4. **V7 队列/取消接入**：`ProviderLoop::run` 改为消费 `MessageQueue`（每轮 `drain_one` 决定续跑）并接收 `CancelHandle` 取代裸 `CancellationToken`，使 `cancel()` 联动 `ProcessTreeCleaner`。目的：P3-5「运行中可发消息」、P3-8「取消不留进程」在 loop 层成立。

#### C. 崩溃恢复正确性（V4）

5. **V4 工具轮次重放**：为 `StreamFinished{has_tool_calls:true}` 增加可持久化事件标记，或从助手 `MessageCommitted` 消息内容（是否含 `ToolCall` part）推断 `CollectingToolCalls`，补「工具轮次重放无 `IllegalTransition`」回归测试。目的：ADR-016 重建承诺对工具轮次成立。

#### D. 预算名副其实（V5 / V6）

6. **V5 软阈值事件**：每轮 `tick_iteration` 后若 `!report.soft_warnings.is_empty()` 则 emit `Diagnostic`，首次触发某维度记录避免刷屏。目的：兑现「达预算产生事件、不静默停」。
7. **V6 补齐四维记录**：loop 入口记起始 `Instant` 每轮 `set_elapsed`；token 记录处用 model-registry 定价 `record_cost`；artifact 工具结果 `record_artifact`；并发 `set_concurrency`。目的：Cost/Duration/Concurrency/ArtifactBytes 四维上限可触发（Phase 14 额度依赖 cost 维度）。

#### E. 调度器上下文与桥接（V8 / V9）

8. **V8 真实上下文注入**：`ToolScheduler` 构造 `ToolExecutionContext` 改用真实 workspace/run 来源（`execute_named` 增加 context 参数），消除 `"default"` 假值。目的：Phase 4 工具接入前置（与 P4-13 V2 同根，本任务负责上下文来源侧）。
9. **V9 LoopContext↔ToolScheduler 桥接**：在 agent-engine 或 app-service 提供 `SchedulerLoopContext` 适配，并加端到端测试（loop + scheduler + capability 冲突）。目的：打通 Phase 3 内部双轨。

#### F. 代码质量（V10 / V11）

10. **V10**：废弃 `ToolScheduler::execute()` 的 input.name 取名路径，统一走 `execute_named`。目的：消除语义错误与「工具自身 input 含 name 字段」冲突。
11. **V11**：`cancelled_run_emits_cancelled...` 测试补 `RunCancelled` 事件断言；评估 `LoopSink` 边组装边丢弃已消费 delta。目的：消除 V1 漏发的虚假信心。

#### G. 文档与基线漂移

12. **plan 漂移**：10 篇 `plan/P3-*.md` 状态回填 🟢、当前 14 个验收框全部勾选；修订 `provider_loop.rs` 模块头注释使其与实际组合范围一致。目的：纠正 AGENTS.md §4 流程偏差与注释失真。
13. **基线**：`futures` 回填属 P2-12（agent-engine 为其第三个消费者，本任务仅引用，不改基线表，避免与 P2-12 冲突）。目的：跨任务基线编辑不撞车。

### 主要产出物

- provider_loop 终态事件补发 + LoopSink 双写广播；RetryController 接入 + MessageQueue/CancelHandle 接入
- recovery 工具轮次重建修复；budget 软阈值事件 + 四维记录；scheduler 真实上下文 + LoopContext 桥接
- 10 篇 plan 回填 + 模块头注释订正

### 验收标准（保留 REVIEW 追踪编号）

- [x] **V1**：取消/预算耗尽路径广播 `RunCancelled`/`RunFailed`（订阅者断言）
- [x] **V2**：订阅者收到 `AssistantTextDelta` 等 delta（流式广播测试）
- [x] **V3**：`StreamInterrupted`/`Network`/`Timeout` 触发重试而非直接 Failed（断流重试测试）
- [x] **V4**：含工具轮次的 Run 重放无 `IllegalTransition`，`recovered_state` 正确（回归测试）
- [x] **V5**：达软阈值 emit `Diagnostic`（事件断言）
- [x] **V6**：Cost/Duration/Concurrency/ArtifactBytes 四维上限可触发（用例）
- [x] **V7**：运行中入队消息被消费；取消触发进程树清理（测试）
- [x] **V8**：工具收到真实 workspace_id/run_id，非 `"default"`（断言）
- [x] **V9**：存在 loop + scheduler + capability 冲突的端到端测试
- [x] **V10**：`execute()` input.name 路径已废弃/移除
- [x] **V11**：取消测试断言 `RunCancelled` 事件被广播
- [x] **文档**：10 篇 `plan/P3-*.md` 状态 🟢、当前 14 个验收框全部勾选；provider_loop 模块头注释与实现一致
- [x] **快速验证**：只运行 Agent Loop、预算、队列、事件重放的定向测试与最小 Mock smoke；workspace 全量与 schema 总门禁延后到 Core 主干 L2

### 验证记录（2026-08-09）

- `cargo test -p agent-engine -p tool-runtime -p policy-engine`
- `cargo clippy -p agent-engine -p tool-runtime -p policy-engine --all-targets -- -D warnings`
- ProviderLoop + Scheduler + 显式 Policy approval 组合测试覆盖批准、拒绝与灾难命令硬拒绝，真实 workspace/run context 与 capability 串行均有断言。

**相关文档**：[REVIEW.md](../../REVIEW.md) §3 · [ADR-016 核心事件可持久化重放](../../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../../ROADMAP.md)

> 跨任务协调（2026-08 review）：本任务与 P4-13 共同触碰 `tool-runtime/scheduler.rs`——P3-11 负责 V8 上下文注入（先）、P4-13 负责 V1 策略接线（后），序列执行避免冲突；建议补一条「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试 + 恢复」最小真实组合的端到端测试。
