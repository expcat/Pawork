# 测试体系

## 单元测试

覆盖：状态机；Provider parser；Tool arguments；Token budget；Compaction；Diff；Patch；路径；Policy；Session reducer；Plugin manifest；Event ordering。

## Provider Contract Tests

每个 Provider 使用相同测试套件：text；tool call；multiple tool calls；image；thinking；usage；stop reason；cancel；timeout；rate limit；malformed stream；partial JSON；reconnect；context overflow。

## Mock Provider

先实现完全可编程的 Mock Provider，绝大部分 Agent 测试不依赖真实 API。

```rust
MockScript::new()
    .text("Starting")
    .tool_call("read_file", json!({...}))
    .tool_call("edit_file", json!({...}))
    .text("Done")
    .complete();
```

Phase 0 的实现位于 `test-support`：脚本可输出 text、多个 tool call、跨 chunk partial JSON、完成或等待取消；`MockProvider` / `MockTool` 均记录调用并提供顺序与参数断言。最小链路测试不访问网络，覆盖 text → tool call → tool result → complete 以及 provider/tool 取消传播。

## Golden Tests

固定：System Prompt；Tool Schema；Context；Session Events；Compaction；Pi Import；Diff；API JSON Schema。

## Fuzz Tests

重点 Fuzz：SSE；JSON Lines；Tool Partial JSON；Unified Diff；Patch；Session Import；路径；Plugin Manifest；MCP Message；Artifact Metadata。

## Chaos Tests

模拟：Provider 中途断网；Core 崩溃；数据库锁；磁盘满；Tool 进程不退出；Side process 持有 stdout；文件被用户同时修改；Git Index 变化；Plugin 崩溃；MCP Server 崩溃。

## 差分测试

以 Pi 作参考行为（而非运行时依赖）。对同一 Mock Provider 脚本比较：Agent 消息顺序；Tool Call 顺序；Session 分支；Compaction 触发；Cancellation；错误恢复。不要求内部实现一致，只检查产品行为。

## 相关文档

- [性能目标](performance-targets.md) · [安全验收](security-acceptance.md)
- [ROADMAP 横切门禁](../../ROADMAP.md)
