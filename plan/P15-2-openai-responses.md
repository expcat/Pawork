# P15-2：OpenAI Responses 适配

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：domain + adapter verified、host composition deferred） · 依赖：P15-1、P15-5、P15-7、P15-8、P6-1
>
> 2026-08-12 进展：`provider-openai` Responses 子适配器已落地（`responses.rs`），与 P6-1 Chat Completions 并存，transport 由 P15-8 `CapabilityNegotiator` 选择（内置目录中 `o3` / `gpt-4.1` 声明 `transport = Responses`，基线模型降级到 Chat Completions 并记录 `LegacyTransport`）。reasoning `encrypted_content` 只经统一 `provider-runtime::reasoning::ReasoningProtector` 边界往返（默认 `InMemoryReasoningProtector`，host 经 `with_reasoning_protector(Arc<dyn ReasoningProtector>)` 注入；持久化 `ProtectedBlobStoreProtector` 接线延 P18-3/4/14，见 [P15-10](P15-10-review-remediation.md)）。hosted tools（web_search / file_search / code_interpreter / image_generation / local_shell / apply_patch / computer_use / mcp）按协商结果放行，未通过项不发送（`Reject`）。错误归一覆盖 vector store 未就绪 / code_interpreter 与 hosted shell 超时 / computer_use 需确认 / MCP 与 skill 不可用。Mock smoke 覆盖 item→event、citations、reasoning 往返、降级与 no_provider_branch 断言。

**最终目的**：为 `provider-openai` 增加 OpenAI Responses API（`/v1/responses`）传输路径，作为 Chat Completions 之外的现代入口，原生承载 reasoning items，以及 Web Search、File Search、Image Generation、Code Interpreter、Hosted Shell、Provider Apply Patch、Skills、Computer Use、server-side MCP、Tool Search 与 Function Calling；同时为 Programmatic Tool Calling 和 API Multi-Agent 保留 canonical 路径。所有 citations、output items 与 encrypted reasoning content 统一经 canonical 域，不在 Core 走 Provider 特例。

**涉及范围**：`provider-openai`（新增 Responses 子适配器与传输选择）、`provider-runtime`（复用 canonical 转换与 ServerToolEvent 注入）；与 P6-1 Chat Completions 路径并存，由 P15-8 能力协商选择。

## 细分步骤

1. **Responses 请求转换** —— 目的：canonical 请求 → Responses `input` items（message / function / reasoning items）；把 P15-1 的 hosted capability 翻译为 Web Search、File Search、Image Generation、Code Interpreter、Hosted Shell、Apply Patch、Skills、Computer Use、MCP 与 Tool Search 工具声明；保留 `previous_response_id` 与加密 reasoning 内容回灌（P15-7）。✅ `responses::to_responses_body` + `resolve_reasoning_inputs`。
2. **output items → ProviderStreamEvent** —— 目的：把 Responses 的 `reasoning`、`message.output_text`、`function_call`、`web_search_call`、`file_search_call` 逐 item 归一为 canonical 事件，citations/annotations 经 P15-5 的 ServerToolEvent 与 Citation 类型回填。✅ `ResponsesStreamAssembler`（含 message / function_call / reasoning / web_search_call / file_search_call / code_interpreter_call / computer_call / image_generation_call / mcp_call / local_shell_call / custom_tool_call）。
3. **reasoning 持久化往返** —— 目的：捕获 `reasoning.summary` 与 `reasoning.encrypted_content`，按 P15-7 / ADR-032 存为 `ReasoningItem`（事件只含 `protected_blob_ref`；encrypted_content 作为作用域加密的不透明 blob，不解码、不入 Event/日志/Keychain/GUI），多轮回灌保证 reasoning 连续。✅ `ReasoningProtector` trait + 默认 `InMemoryReasoningProtector`；adapter `stream_responses` 在收到 reasoning candidate 时 protect 后只发射 blob 引用。
4. **传输选择与降级** —— 目的：经 P15-8 能力协商，模型支持 Responses 时走 Responses，否则降级到 P6-1 Chat Completions（web_search 等不可用时退化为客户端工具或拒绝）。✅ `OpenAiProvider::resolve_capabilities` → `CapabilityNegotiator::negotiate`，`stream` 据 `chosen_transport` 分支。
5. **server tool 事件接线** —— 目的：把搜索、代码/ shell、patch、图像、computer、MCP 与 skills/tool-search 的调用、进度、结果和 citations 经 P15-5 规范化为 ServerToolEvent；大型输出走 Artifact，后续轮次以 `ProviderTranscript` 续接原生 output item / response reference。只有客户端 Function Calling 的 `CoreSuppliedResult` 才映射 `function_call_output`，server tool 不经过该路径。✅ server tool 全部经 `ProviderStreamEvent::ServerTool`；`message_to_responses_input` 仅 `ContentPart::ToolResult` → `function_call_output`；`response_id` 作为下一轮 `previous_response_id` 续接。
6. **Programmatic Tool Calling / API Multi-Agent** —— 目的：将程序化工具编排与 Provider-side Multi-Agent 表达为明确 capability 和事件，不把 Provider 的内部 worker 伪装成 Pawork 本地 P12 worker；不支持时显式 `Unsupported`。✅ `ProgrammaticToolCalling` / `ServerSideMultiAgent` 经 `requirements_from_request` 进入协商；未声明即 `Reject`（fail-closed），custom_tool_call 以 ServerTool 表达、不伪装成客户端 function。
7. **错误归一** —— 目的：Responses 特有错误（vector store 未就绪、code_interpreter/hosted shell 超时、computer_use 需确认、MCP/skill 不可用）归一为统一 ProviderError，重试建议与 P2-10 一致。✅ `normalize_responses_error`。
8. **分段交付与 Mock smoke** —— 目的：先完成 Responses + function/reasoning，再按 capability 逐组接入 hosted tools；每组只跑录制夹具的 item→event、citations、reasoning 与降级 smoke，完整矩阵统一落 P15-9。✅ `tests/responses.rs` 9 个 Mock smoke；完整能力矩阵留 P15-9。

## 主要产出物

- `provider-openai` Responses 子适配器（`responses.rs`）+ 传输选择（`provider.rs`）✅
- Responses ↔ canonical 事件映射夹具与定向测试 ✅

## 验收标准（P15-2 定向实现，2026-08-12）

- [x] Responses output items 正确归一为 ProviderStreamEvent（reasoning/message/function/server tool）—— `ResponsesStreamAssembler` 覆盖全部 output item 类型，Mock smoke `responses_text_reasoning_and_function_call_stream` 验证。
- [x] web_search / file_search 的 citations 经 P15-5 归一为 Citation，可重放 —— `responses_web_search_emits_server_tool_and_citations` 验证 ServerTool Completed + SourceAdded + CitationAdded（url_citation 归属产生它的 web_search_call）。
- [x] Hosted Shell / Apply Patch / Code Interpreter / Image Generation / Computer Use 的进度与结果进入 canonical 事件，大输出走 Artifact —— `handle_code_interpreter_done` / `handle_local_shell_done` / `handle_image_generation_done` / `handle_computer_call_done` 大输出统一以 `ArtifactId` 引用（ProgramOutput / ComputerScreenshot）。
- [x] server-side MCP / Skills / Tool Search 可经 capability negotiation 启用；Programmatic Tool Calling / API Multi-Agent 不支持时显式降级或拒绝 —— `AcceptedResponsesTools::from_supported` 仅放行协商通过的类别；未声明 hosted tool 进 `Reject`（`resolve_capabilities_chooses_responses_for_modern_model` / `responses_unsupported_hosted_tool_is_rejected_not_silently_dropped`）。
- [x] `reasoning.encrypted_content` 只经 Protected Blob Store 引用往返，不入 Event/日志/Keychain/GUI（P15-7 / ADR-032）—— `responses_reasoning_encrypted_content_only_reaches_blob_store` 断言事件序列 Debug / JSON 均不含明文；`responses_reasoning_round_trip_injects_decrypted_input` 验证回灌。
- [x] server tool 续接走 `ProviderTranscript`；只有客户端 Function Calling 结果可映射 `function_call_output` —— `message_to_responses_input` 只把 `ContentPart::ToolResult`（CoreSuppliedResult）映射为 `function_call_output`；server tool 仅经 `ProviderStreamEvent::ServerTool`，custom_tool_call 不映射 function_call_output。
- [x] 不支持 Responses 时降级到 Chat Completions，行为可观察（Mock smoke）—— `responses_degrades_to_chat_completions_for_baseline_model` 验证 gpt-4o 命中 `/chat/completions` 而非 `/responses`。
- [x] 不在 Core 走 OpenAI 名称分支（`no_provider_branch` 断言）—— transport 选择纯函数读 `ModelCapabilities.transport`，不读 Provider 名；`responses_path_has_no_provider_name_branch_in_events` 断言事件序列不含 `openai` 字面。
- [x] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁 —— 仅 `cargo check/test/clippy -p provider-openai`（+ provider-runtime / test-support / model-registry 依赖）。

> 完整能力矩阵（真实 Responses API 与 Programmatic Tool Calling / API Multi-Agent 端到端）在 P15-9 集中验收；持久化 reasoning protector（`ProtectedBlobStoreProtector` + 生产 `ProtectedKeyResolver`）由 host 经 `OpenAiProvider::with_reasoning_protector` 注入，接线延 P18-3/4/14（见 [P15-10](P15-10-review-remediation.md)）。正式宿主装配真实 Provider 亦延 P18-3/4，本任务为有界 TargetVerified（adapter verified，host composition deferred）。

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [P15-7](P15-7-reasoning-state.md) · [P15-8](P15-8-capability-discovery.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：Responses 与 Chat Completions 同 crate 共存；按「reasoning+function → search/MCP/tool-search → code/shell/patch → image/computer → programmatic/multi-agent」分段交付，每段保持数小时级写入集，完整能力矩阵只在 P15-9 集中验收。
