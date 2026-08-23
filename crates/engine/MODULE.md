# pawork-engine

Agent Engine：组请求、跑 session turn、工具循环。生产 `pawork-*` 依赖 **仅** `pawork-domain`。ADR-039 不合并清单成员。

## 职责

把 canonical 消息与工具定义装配成 `CanonicalModelRequest`，驱动 `run_session`：调 `ModelProvider::stream`、映射 `ProviderStreamEvent` → `AgentEvent`、在 `LoopContext` 注入点执行工具 / 请求审批 / 压缩。不重试、不落库、不按 Provider 名称分支。`run_turn` 为 `pub(crate)`（R0 D12 收口公开面）。

## 模块树

```
src/
  lib.rs
  tool_loop.rs  session_turn.rs  appender.rs  cancel.rs  event.rs
  context/{mod,budget,compaction,token,tool_result_trim}.rs
tests/
  domain_only.rs
  no_provider_branch.rs
```

## 对外入口/API 面

- 装配：`assemble_request` / `assemble_request_with_tools`。
- 循环：`run_session`、`run_manual_compaction`、`LoopContext`（`execute_tools` / `request_approval` / `compact_history` / `snapshot_write_tools`…）、`CompactionOutcome`（source count + `compacted_through`）、`DEFAULT_MAX_TOOL_ROUNDS = 20`、`ApprovalGate`。
- 会话：`SessionTurn`、`run_session_turn`。
- 上下文（`pub mod context`）：`ContextBudget` / `ContextLimits`、`compute_compaction`、`TokenEstimator` / `HeuristicEstimator`、工具结果 trim。
- 取消：`CancelHandle`、`CancelReason::{User,Budget,System,Shutdown}`、`ProcessTreeCleaner`（杀树由宿主注入，本包不依赖 exec）。
- 事件：`AgentEventSink`、`LoopEventEmitter`、`EngineError::{Provider,Sink,MaxToolRounds}`。

## 依赖与被依赖

- **生产依赖**：`pawork-domain` only（`tests/domain_only.rs`）。
- **dev-dep**：`pawork-testkit`、`pawork-providers`（守护测试从 `CHANNEL_REGISTRY` 派生名单）。
- **被依赖**：`pawork-app`、`pawork-cli`（chat 取消/渲染）。desktop deny-list 含本包。

## 红线与注意事项

- 禁止 `if provider_id == "…"` 一类分支；能力走 domain / registry。`tests/no_provider_branch.rs` 扫描 `src/`，对照通道 id 与基线别名（`openai`/`anthropic`/`grok`/`glm` 等）。
- `request_approval` 必须在等待前 emit `ToolApprovalRequested`（K-02 / R4 波 B）；engine 只补 `Responded`。
- 不持久化、不选通道、不读 Secret；宿主注入 Provider / 工具 / cleaner。
- 压缩 = 重写前缀 = 缓存失效；不要在 engine 里按厂商做 cache 特例。
- `compact_history` 的 storage 错误必须终止当前 run；无持久化 outcome 时水位只能为 0，不得拿摘要事件自身 sequence 代替。

## 相关文档

- [docs/design.md](../../docs/design.md) §3.2 引擎语义
- [plan/R4-host-decomposition.md](../../plan/R4-host-decomposition.md)（审批 emit 时序）
- [AGENTS.md](../../AGENTS.md) §2
- [代码地图总索引](../../docs/code-map/README.md)
