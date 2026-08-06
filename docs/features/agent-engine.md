# Agent Engine

## 职责

实现完整 Agent 循环：状态机、运行控制、预算、Tool 调度、重试、取消、事件流与中断恢复。

## 实现模块（Phase 3）

落地于 `agent-engine` crate，按子模块组织（详见各模块文档注释）：

- `state`：Run 状态机（全部状态与转换、事件 hint、非法转换防御）。
- `appender`：Provider 流式增量组装为助手消息与 tool call。
- `provider_loop`：Provider Loop 主干（多轮工具循环、审批、执行、回填）。
- `budget`：多维预算控制（软/硬阈值、达预算产生决策而非静默停）。
- `queue`：用户消息队列（排队、replace queued、崩溃可恢复快照）。
- `broadcast`：事件广播（bounded channel + backpressure + Lagged 丢弃）。
- `retry`：Agent 层重试（断流/last call/run、退避、retry_after 尊重）。
- `cancel`：取消传播（根令牌 + 进程树清理抽象）。
- `recovery`：Interrupted Run 恢复（事件重放重建状态、<1s）。

工具执行与审批通过 trait 注入（`LoopContext` / `ApprovalResolver`），使 Provider Loop
与 SQLite / `tool-runtime::ToolScheduler` 解耦，便于单测与替换。

## Agent Loop

14 步循环见 [控制流](../architecture/control-flow.md) §2。状态机与事件见 [领域模型](../architecture/domain-model.md)。

## 运行控制

Cancel / Pause / Resume / Retry Last Provider Call / Retry Run / Fork From Message / Replace Queued Messages / 修改模型 / 修改 Thinking Level / 修改预算 / 手动 Compaction / 恢复 Interrupted Run。

## 预算控制

迭代次数、Tool Call 次数、运行时间、输入 Token、输出 Token、费用、Shell 输出、Artifact 大小、并发 Tool Call 上限。达预算产生明确事件，不静默停。

## Tool Call 调度

默认策略与 capability 分类见 [控制流](../architecture/control-flow.md) §5 与 [tools](tools.md)。

## 验收标准

- Mock Provider 可完成多轮工具循环
- Cancel 不留下运行进程
- 所有状态转换都有事件
- 重启后可识别 Interrupted Run
- Agent Engine 不含 Provider 特例

## 相关文档

- [控制流](../architecture/control-flow.md) · [领域模型](../architecture/domain-model.md)
- [context](context.md) · [tools](tools.md) · [sessions](sessions.md)
- [ADR-002 解耦](../adr/ADR-002-agent-engine-provider-decoupled.md)
- [ROADMAP Phase 3](../../ROADMAP.md)
