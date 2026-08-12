# P15-3：Anthropic Modern Messages 适配

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：domain + adapter verified、host composition deferred） · 依赖：P15-1、P15-5、P15-7、P15-8、P6-2

**最终目的**：把 `provider-anthropic` 升级到现代 Messages：`output_config.format` Structured Outputs、request-level effort、adaptive/interleaved thinking、modern prompt cache，以及 Web Search、Web Fetch、Code Execution、Advisor、Tool Search、MCP Connector、Memory、Bash、Text Editor、Computer Use 等 server/client tool 形态。工具结果与 citations 经 P15-5 归一，reasoning/signature 经 P15-7 持久化，Core 不感知 Anthropic 名称。

**涉及范围**：`provider-anthropic`（现代 server tools 与 thinking signature）、`provider-runtime`（复用 ServerToolEvent / ReasoningItem 通道）；与 P6-2 既有 Messages 路径同 crate 共存。

> 2026-08-12 事实纠正（[P15-10](P15-10-review-remediation.md)）：评审 §3.3「modern.rs 与 request.rs/stream.rs 边界偏散、更接近重写一遍」不成立——`modern.rs` 复用 `request.rs` 的 message / tool-choice / thinking-budget / cache-breakpoint helpers；现代与基线路径共同进入 `provider.rs::pump_messages`，共享 auth、`SseParser` 与 `stream.rs::event_to_events`（含 usage 归一）。modern 只承载现代字段与 server-tool 差异，不要求额外下沉。

## 细分步骤

1. **现代请求字段** —— 目的：把 canonical Structured Output、effort、adaptive thinking 与 prompt-cache 策略映射到 Anthropic 当前 Messages 字段；`output_config.format` 走原生 schema，不再退化为 system prompt，effort level 不再固定为 `budget_tokens`。
2. **hosted/client tools → Anthropic tools** —— 目的：按 capability 映射 Web Search、Web Fetch、Code Execution、Advisor、Tool Search、MCP Connector、Memory、Bash、Text Editor 与 Computer Use；客户端 function 工具仍走既有 `name/input_schema`，不同执行位点不得混用。
3. **server tool 结果归一** —— 目的：把各类 `server_tool_use` 与 Provider 产生的 result block 归一为 P15-5 的 ServerToolEvent/Citation/Source，大输出走 Artifact，并以 `ProviderTranscript` 续接原生 block / continuation reference。只有客户端 `tool_use` 经 Core 执行后才产生 `CoreSuppliedResult` 并映射 Anthropic `tool_result`；server tool 不由 Core 合成该 block。
4. **thinking signature 往返** —— 目的：捕获 thinking block 的 `signature` 与 thinking 内容，按 P15-7 / ADR-032 存为不透明 `ReasoningItem`（事件只存 `protected_blob_ref`），支持 adaptive/extended thinking 跨轮连续。
5. **interleaved thinking** —— 目的：支持 thinking 与 tool_use 交错输出，按事件顺序归一（ThinkingDelta / ServerToolEvent 交错不丢序）。
6. **能力协商与降级** —— 目的：经 P15-8 检测 Structured Output、effort、server tools、tool search 与 thinking 支持；不支持时显式退化到 P6-2 基线或返回 `Unsupported`，禁止静默丢字段。
7. **citation/source 规范化** —— 目的：Anthropic citations 映射到 P15-5 统一 Citation/Source，与 OpenAI/xAI 口径对齐。
8. **分段交付与 Mock smoke** —— 目的：先做 structured output + effort/adaptive thinking，再分组接入 search/execution/MCP/tool-search/memory/computer；每组只跑定向夹具，完整矩阵落 P15-9。

## 主要产出物

- `provider-anthropic` 现代 server tools + thinking signature 子路径
- Anthropic citation → 统一 Citation/Source 映射夹具

## 验收标准

- [x] web_search / code_execution 的 server tool 结果经 P15-5 归一为 Citation/Source（Mock smoke）
- [x] `output_config.format`、effort、adaptive thinking 与 modern prompt cache 原生映射，不再用 system prompt 或固定 budget 静默替代
- [x] Web Fetch / Advisor / Tool Search / MCP Connector / Memory / Bash / Text Editor / Computer Use 均有 capability、执行位点与显式降级语义
- [x] thinking `signature` 只经 Protected Blob Store 引用往返，多轮 extended thinking 连续（P15-7 / ADR-032）
- [x] server tool 续接走 `ProviderTranscript`；只有客户端 `tool_use` 的 Core 结果可映射 `tool_result`
- [x] interleaved thinking 与 tool_use 顺序保持（顺序断言）
- [x] 不支持 server tools 的模型降级到 function calling，不报错（降级用例）
- [x] 不在 Core 走 Anthropic 名称分支（`no_provider_branch` 断言）
- [x] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

## 验证记录（2026-08-12）

- 现代请求：`output_config.format.json_schema` 原生结构化输出、request-level
  effort（`output_config.effort`）、`thinking: {"type":"adaptive"}`、modern
  prompt cache 断点；P6-2 基线路径与全部既有用例不回归（14 contract + 71 lib
  全过）。
- 工具与归一：WebSearch / WebFetch / CodeExecution / HostedShell(bash) /
  ProviderApplyPatch(text_editor) / ComputerUse / ToolSearch / ServerSideMcp
  (mcp_connector) / Memory 按 capability 映射，advisor 经 canonical 名称回退；
  客户端 function 仍走 `name/input_schema`；`server_tool_use` + 各
  `<name>_tool_result` / 错误对象 → ServerToolEvent 生命周期，citations →
  CitationAdded，轮末发 `ProviderTranscript` 信封；transcript → 原生块重建
  永不出客户端 `tool_result`（负向断言）。
- thinking signature：`content_block_stop` 捕获 → 统一 `provider-runtime::reasoning::ReasoningProtector`
  不透明保护（默认 `InMemoryReasoningProtector`，host 经 `with_reasoning_protector(Arc<dyn ReasoningProtector>)`
  注入；持久化 `ProtectedBlobStoreProtector` 接线延 P18-3/4/14，见 [P15-10](P15-10-review-remediation.md)）
  → 事件只携带 `protected_blob_ref` → 第二轮回灌重建原 thinking 块（fixture 精确断言）；
  默认与 OpenAI / xAI 共享同一 `InMemoryReasoningProtector`（进程内可回放），不再有独立 fail-closed 路径。
- 降级：基线模型（claude-3-5-sonnet）现代字段 → P6-2（effort clamp 为 budget、
  XHigh→High、system 指令、hosted 降级为 function calling）可观察且不报错；
  不可表达 kind 逐项 note 降级。
- Mock smoke：6 个 wiremock fixture 用例（structured output+effort+thinking、
  server tool 归一+transcript、signature 往返、interleaved 顺序、基线降级、
  kind 降级）全过；interleaved 顺序断言（ThinkingDelta / ReasoningItem /
  ServerTool 交错不丢序）。

```text
Validation Level: L1
Affected crates: provider-anthropic（provider-runtime 仅作依赖 check）
Validated: cargo test -p provider-anthropic（71 lib + 14 contract + 6 modern）/ cargo clippy -p provider-anthropic --all-targets -- -D warnings / cargo check -p provider-runtime / cargo fmt -p provider-anthropic
Targeted regressions: P6-2 contract 全量、现代请求体字段、server tool 生命周期与 transcript、signature 不透明往返、interleaved 顺序、两类降级用例
Full workspace gate: NOT RUN（P15-3 明确要求仅定向/Mock smoke 验收）
```

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [P15-7](P15-7-reasoning-state.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：按「structured output+effort+thinking → search/tool-search/MCP → execution/editor/memory → computer」分段交付；不新增整套 SDK，完整矩阵在 P15-9 集中验收。
