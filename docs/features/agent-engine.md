# Agent Engine

## 职责

实现完整 Agent 循环：状态机、运行控制、预算、Tool 调度、重试、取消、事件流与中断恢复。

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
