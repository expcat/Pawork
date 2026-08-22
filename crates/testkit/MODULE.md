# pawork-testkit

dev-only：可编程 Mock Provider / Mock Tool 与流形状断言。依赖 `pawork-domain`。

## 职责

给 engine / tools / app / client 的测试提供确定性 `ModelProvider` 与 `AgentTool` 实现，以及「文本流 / 单工具 / 并行工具」等契约断言。不进入 `pawork` 生产闭包；逻辑回归走 Mock，真实 API 只承担冒烟。

## 模块树

```
src/
  lib.rs         # MockScript / MockProvider / MockTool / Recording*Sink
  contract.rs    # 流形状断言
```

无 `tests/` 目录。

## 对外入口/API 面

- `MockScript`：按序追加 `response_started` / `text` / `thinking` / `tool_call` / `usage` / `complete` / `fail` / `wait_for_cancellation` 等。
- `MockProvider`（实现 `ModelProvider`）：`sequence`、`with_id`、`with_models`、`calls()` → `MockProviderCallRecord`。
- `MockTool`（实现 `AgentTool`）：`new` / `failing` / `with_descriptor`、`calls()` → `MockToolCallRecord`。
- `RecordingProviderSink` / `RecordingToolSink`。
- `contract`：`assert_text_stream`、`assert_single_tool_call`、`assert_parallel_tool_calls`、`count_variant`；crate 根 re-export。另有 `assert_provider_request_order`。

默认 `MockTool` descriptor：`ReadOnly`、`ClientFunction`、`Local`、`requires_approval: false`、`allowed_in_untrusted_workspace: true`。

P15 的 server_tool / citation / reasoning 断言 **不** 在本包。

## 依赖与被依赖

- **依赖**：`pawork-domain`；`async-trait`、`serde_json`。无 feature。
- **被依赖（全部为 dev-dependencies）**：`pawork-engine`、`pawork-tools`、`pawork-app`、`pawork-client`。
- **无生产依赖方**。不要把本包写进 `apps/pawork` 的依赖。

## 红线与注意事项

- 断言只关心 canonical 流形状，不按 Provider 名称分支。
- `MockProvider::stream`：脚本未以 `ResponseCompleted` 结束则报错；sequence 耗尽 → `ProviderErrorKind::StreamInterrupted`。
- 本包不是协议 golden 的家（那在 `pawork-domain` / `pawork-protocol` / `pawork-storage`）。

## 相关文档

- [docs/design.md](../../docs/design.md) §2
- [docs/task-guide.md](../../docs/task-guide.md) §3.3（engine 逻辑回归走 MockProvider）
- [代码地图总索引](../../docs/code-map/README.md)
