# P15-5：Server Tool Events（hosted tool 事件与 Citation/Source 统一）

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-3、P0-4、P1-4、P1-5、P15-1

> 2026-08-12 事实纠正（[P15-10](P15-10-review-remediation.md)）：评审「`ServerToolEvent` 部分变体仅 fixture 触发」不成立——三家 adapter 均有真实 wire producer（`ProviderStreamEvent::ServerTool` 发射点）：OpenAI `responses.rs` → `CitationAdded` / `SourceAdded` / `ProgramStarted` / `ProgramOutput` / `ComputerActionRequested` / `ComputerScreenshot` / `Started` / `Completed`；Anthropic `stream.rs` → `CitationAdded` / `SourceAdded` / `Started` / `Completed`；xAI `responses.rs` → `CitationAdded` / `SourceAdded` / `ProgramStarted` / `ProgramOutput` / `Started` / `Completed` / `Failed`。`ProgramStarted` / `Computer*` 属「真实 wire producer + 部分变体待真实服务端工具消费」。

**最终目的**：为 `ProviderHosted` / `ProviderExtension` 两种非本地执行位点定义统一的 canonical 事件与引用类型——`ServerToolEvent`、`Citation`、`Source`——让 OpenAI / Anthropic / xAI 三家 server tools（web_search / live_search / file_search / code_execution）的调用与结果用同一口径表达，并可持久化、可重放（ADR-016）。这是 P15-2/3/4 落地三家现代 API 的共享前置，确保 Core 侧无 Provider 特例。

**涉及范围**：`agent-domain`（Citation / Source / ServerToolEvent 领域类型）、`provider-runtime`（`ProviderStreamEvent` 扩展 server tool 变体）、`session-store` / Projection（事件持久化重放）；只定义与归一通道，不实现任何具体 server tool。

## 细分步骤

1. **Citation / Source 领域类型** —— 目的：定义 `Citation { index, url?, title?, snippet?, text?, document_index?, source_kind }` 与 `Source`（原始引用元数据），字段覆盖三家（OpenAI url/citations、Anthropic url/text/document_index、xAI sources 的 url/title/snippet），缺省字段为空而非猜值。
2. **ServerToolEvent 生命周期** —— 目的：扩展事件模型，至少覆盖 `ServerToolStarted` / `ServerToolProgress` / `ServerToolCompleted`、`CitationAdded` / `SourceAdded`、`ComputerActionRequested` / `ComputerScreenshot`、`ProgramStarted` / `ProgramOutput`；允许 arguments/output delta 与状态错误，全部归入可持久化事件流。与本地 `ToolCall*` 并列但语义分离，server tool 不走 scheduler 本地执行。
3. **ProviderStreamEvent 桥接** —— 目的：在 `provider-runtime` 的 `ProviderStreamEvent` 上新增 server tool 变体（或复用 `ProviderMetadata` + 专用枚举），供 P15-2/3/4 适配器逐 item 发射，避免每家各自定义。
4. **按 ContinuationMode 分离续传通道** —— 目的：冻结两条互斥路径：`CoreSuppliedResult` 仅供 `ClientFunction`，由适配器翻译为 OpenAI `function_call_output` / Anthropic `tool_result` 等客户端函数结果；`ProviderTranscript` 仅供 `ProviderHosted` / `ProviderExtension`，Core 持久化脱敏的 provider-neutral transcript envelope（原生 output item / cursor / continuation reference），适配器按原协议续接。Hosted/extension 结果不得经过前一条 function-result 路径，也不得由 Core 合成 `tool_result`。
5. **三家口径对齐表** —— 目的：以文档形式固化「OpenAI / Anthropic / xAI 字段 → 统一 Citation/Source」映射，作为 P15-2/3/4 与 P15-9 契约夹具的依据；对不上的字段返回 `Unsupported` 而非猜测。
6. **持久化与重放** —— 目的：ServerToolEvent 经 Event Store append、Projection 可重建（P1-4/P1-5），崩溃恢复与 Session 重放时 server tool 调用与 citations 完整再现；raw_metadata 脱敏（不含 Secret）。
7. **Mock smoke 往返 + 重放** —— 目的：用 Mock 发射一组 ServerToolEvent + Citation，验证落库、Projection 重建、重放结果与原始一致；三家字段映射各一条夹具。

## 主要产出物

- `agent-domain`：`Citation` / `Source` / `ServerToolEvent` 完整生命周期类型（含 search/execution/computer/program）
- `provider-runtime`：`ProviderStreamEvent` server tool 变体 + 两种 `ContinuationMode` 的互斥续传通道
- 三家 citation 口径对齐表（文档）
- 持久化重放 + 三家字段映射 Mock smoke 夹具

## 验收标准

- [x] `Citation`/`Source` 覆盖三家字段，缺省为空不猜值
- [x] `ServerToolEvent` 落入可持久化事件流，崩溃后 Projection 可重建（重放测试）
- [x] Search/citation、program output 与 computer action/screenshot 的增量顺序可重放；大型 screenshot/output 只存 Artifact 引用
- [x] hosted tool 结果属 `ContinuationMode::ProviderTranscript`（P15-1），Core 只归一为 ServerToolEvent / transcript envelope，不伪装成本地 `ToolResult`、不触发本地执行
- [x] 只有 `ClientFunction/CoreSuppliedResult` 可映射 `function_call_output` / `tool_result`；hosted/extension 续传不得经过该路径（负向断言）
- [x] transcript envelope 不携带 Provider 名称；具体协议翻译封装在 provider adapter（`no_provider_branch` 断言）
- [x] raw_metadata 经脱敏，不含 Secret
- [x] 三家字段映射夹具各通过；对不上口径的字段返回 `Unsupported`
- [x] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

## 验证记录（2026-08-11）

- canonical：`Citation` / `Source` / 11 类 `ServerToolEvent` / provider-neutral transcript envelope 已落地；`ProviderStreamEvent` 与 `AgentEvent` 顺序桥接，不参与本地消息组装。
- 续传边界：Hosted / Extension 只走 `ProviderTranscript`，Mock 覆盖 all-hosted、mixed 与 Extension approve/deny/cancel；不存在 hosted `ToolResult` 或本地执行。
- 持久化：全部事件变体按 sequence append、snapshot、删除投影、rebuild 后完全一致；未知 call 返回 `ProjectionInvariant`；screenshot 保留 `media_type`；嵌套 `raw_metadata.api_key` 在事实表、投影与重放均脱敏。
- 协议夹具：OpenAI `web_search_call.status/action/sources` 与扁平 `url_citation`；Anthropic `server_tool_use`、search result/error 与 citation；xAI server-side output item、顶层 citations URL 与 annotation。不可映射口径均返回 `Unsupported`，未提前接入 P15-2/3/4 transport。
- reviewer：PASS；两个 P2 观察中，全局 call id 主键沿用既有 projection 身份模型，Anthropic 官方 error object 形态已追加 fixture 锁定。

```text
Validation Level: L1
Affected crates: agent-domain、agent-events、provider-api、provider-runtime、agent-engine、session-store、tool-runtime、test-support、provider-openai、provider-anthropic、provider-xai
Validated: 11 个 changed crates cargo test（含 contract/doc tests）/ cargo clippy --all-targets -D warnings / cargo fmt --all --check / git diff --check
Targeted regressions: 11 类事件顺序与 Projection rebuild、深层 secret 脱敏、未知 call 不变量、ProviderTranscript 与 ToolResult 分离、三家 wire fixture 与 Unsupported 负向路径
Full workspace gate: NOT RUN（P15-5 明确要求定向/Mock smoke，未命中升级条件）
```

**相关文档**：[providers](../docs/features/providers.md) · [sessions](../docs/features/sessions.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ADR-018 大 payload Artifact](../docs/adr/ADR-018-large-payload-artifact-id.md) · [P15-1](P15-1-canonical-tool-v2.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：纯领域扩展，不新增依赖；大型 server tool 输出走 ADR-018 Artifact 引用，避免整段入事件流。
