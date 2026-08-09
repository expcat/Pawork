# P3-11：Phase 3 评审修复（REVIEW remediation）

> Phase 3 · Agent Loop · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P3-1 ~ P3-10（与 P4-13 在 `scheduler.rs` 上下文注入上同根，序列协调：本任务先做 V8 上下文注入，P4-13 再做 V1 策略接线）

**最终目的**：把 [REVIEW.md](../REVIEW.md) §3（Phase 3）评审指出的「组件齐全、主干未接线」状态收口——让 Provider Loop 真正组合状态机、预算、重试、消息队列、取消、广播与工具调度，使终态事件可观察、流式增量对订阅者可见、断流可重试、崩溃重放对工具轮次正确，并把 plan 文档漂移一并处理。

**涉及范围**：`agent-engine`（provider_loop/state/budget/retry/cancel/queue/broadcast/recovery）、`tool-runtime`（scheduler）、`agent-events`、`context-engine`、`plan/P3-*.md`

## 细分步骤（分组）

### A. 终态事件与流式广播（V1 / V2）

1. **V1 终态事件补发**：`provider_loop.rs` 四条终止路径（预检取消、预算耗尽、流中取消、流中错误）统一经 `emit_terminal_payload` 补发 `RunCancelled`/`RunFailed`。目的：每次转换都有事件、状态可由事件序列重建。
2. **V2 流式增量广播**：让 `LoopSink` 在缓冲的同时把 delta 事件（`AssistantTextDelta`/`AssistantThinkingDelta`/`ToolCallArgumentsDelta`/`ToolOutputDelta`/`ToolCallStarted`）fan-out 到 `EventBroadcaster`（双写 sink）。目的：GUI/CLI 可逐 token 流式显示。

### B. 重试与队列/取消接入主干（V3 / V7）

3. **V3 重试接入**：在 `run_turn` 的 `provider.stream()` 外层包 `RetryController`，断流时保持 `messages` 不变重发，每次 `RetryAttempt` 翻译为 `Diagnostic`。目的：P3-7「断流可重试」真正落地。
4. **V7 队列/取消接入**：`ProviderLoop::run` 改为消费 `MessageQueue`（每轮 `drain_one` 决定续跑）并接收 `CancelHandle` 取代裸 `CancellationToken`，使 `cancel()` 联动 `ProcessTreeCleaner`。目的：P3-5「运行中可发消息」、P3-8「取消不留进程」在 loop 层成立。

### C. 崩溃恢复正确性（V4）

5. **V4 工具轮次重放**：为 `StreamFinished{has_tool_calls:true}` 增加可持久化事件标记，或从助手 `MessageCommitted` 消息内容（是否含 `ToolCall` part）推断 `CollectingToolCalls`，补「工具轮次重放无 `IllegalTransition`」回归测试。目的：ADR-016 重建承诺对工具轮次成立。

### D. 预算名副其实（V5 / V6）

6. **V5 软阈值事件**：每轮 `tick_iteration` 后若 `!report.soft_warnings.is_empty()` 则 emit `Diagnostic`，首次触发某维度记录避免刷屏。目的：兑现「达预算产生事件、不静默停」。
7. **V6 补齐四维记录**：loop 入口记起始 `Instant` 每轮 `set_elapsed`；token 记录处用 model-registry 定价 `record_cost`；artifact 工具结果 `record_artifact`；并发 `set_concurrency`。目的：Cost/Duration/Concurrency/ArtifactBytes 四维上限可触发（Phase 14 额度依赖 cost 维度）。

### E. 调度器上下文与桥接（V8 / V9）

8. **V8 真实上下文注入**：`ToolScheduler` 构造 `ToolExecutionContext` 改用真实 workspace/run 来源（`execute_named` 增加 context 参数），消除 `"default"` 假值。目的：Phase 4 工具接入前置（与 P4-13 V2 同根，本任务负责上下文来源侧）。
9. **V9 LoopContext↔ToolScheduler 桥接**：在 agent-engine 或 app-service 提供 `SchedulerLoopContext` 适配，并加端到端测试（loop + scheduler + capability 冲突）。目的：打通 Phase 3 内部双轨。

### F. 代码质量（V10 / V11）

10. **V10**：废弃 `ToolScheduler::execute()` 的 input.name 取名路径，统一走 `execute_named`。目的：消除语义错误与「工具自身 input 含 name 字段」冲突。
11. **V11**：`cancelled_run_emits_cancelled...` 测试补 `RunCancelled` 事件断言；评估 `LoopSink` 边组装边丢弃已消费 delta。目的：消除 V1 漏发的虚假信心。

### G. 文档与基线漂移

12. **plan 漂移**：10 篇 `plan/P3-*.md` 状态回填 🟢、当前 14 个验收框全部勾选；修订 `provider_loop.rs` 模块头注释使其与实际组合范围一致。目的：纠正 AGENTS.md §4 流程偏差与注释失真。
13. **基线**：`futures` 回填属 P2-12（agent-engine 为其第三个消费者，本任务仅引用，不改基线表，避免与 P2-12 冲突）。目的：跨任务基线编辑不撞车。

## 主要产出物

- provider_loop 终态事件补发 + LoopSink 双写广播；RetryController 接入 + MessageQueue/CancelHandle 接入
- recovery 工具轮次重建修复；budget 软阈值事件 + 四维记录；scheduler 真实上下文 + LoopContext 桥接
- 10 篇 plan 回填 + 模块头注释订正

## 验收标准（保留 REVIEW 追踪编号）

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

## 验证记录（2026-08-09）

- `cargo test -p agent-engine -p tool-runtime -p policy-engine`
- `cargo clippy -p agent-engine -p tool-runtime -p policy-engine --all-targets -- -D warnings`
- ProviderLoop + Scheduler + 显式 Policy approval 组合测试覆盖批准、拒绝与灾难命令硬拒绝，真实 workspace/run context 与 capability 串行均有断言。

**相关文档**：[REVIEW.md](../REVIEW.md) §3 · [ADR-016 核心事件可持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ROADMAP](../ROADMAP.md)

> 跨任务协调（2026-08 review）：本任务与 P4-13 共同触碰 `tool-runtime/scheduler.rs`——P3-11 负责 V8 上下文注入（先）、P4-13 负责 V1 策略接线（后），序列执行避免冲突；建议补一条「ProviderLoop + ToolScheduler + MessageQueue + 预算 + 重试 + 恢复」最小真实组合的端到端测试。
