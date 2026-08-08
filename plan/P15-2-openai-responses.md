# P15-2：OpenAI Responses 适配

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P15-1、P15-5、P15-7、P15-8、P6-1

**最终目的**：为 `provider-openai` 增加 OpenAI Responses API（`/v1/responses`）传输路径，作为 Chat Completions 之外的现代入口，原生承载 reasoning items，以及 Web Search、File Search、Image Generation、Code Interpreter、Hosted Shell、Provider Apply Patch、Skills、Computer Use、server-side MCP、Tool Search 与 Function Calling；同时为 Programmatic Tool Calling 和 API Multi-Agent 保留 canonical 路径。所有 citations、output items 与 encrypted reasoning content 统一经 canonical 域，不在 Core 走 Provider 特例。

**涉及范围**：`provider-openai`（新增 Responses 子适配器与传输选择）、`provider-runtime`（复用 canonical 转换与 ServerToolEvent 注入）；与 P6-1 Chat Completions 路径并存，由 P15-8 能力协商选择。

## 细分步骤

1. **Responses 请求转换** —— 目的：canonical 请求 → Responses `input` items（message / function / reasoning items）；把 P15-1 的 hosted capability 翻译为 Web Search、File Search、Image Generation、Code Interpreter、Hosted Shell、Apply Patch、Skills、Computer Use、MCP 与 Tool Search 工具声明；保留 `previous_response_id` 与加密 reasoning 内容回灌（P15-7）。
2. **output items → ProviderStreamEvent** —— 目的：把 Responses 的 `reasoning`、`message.output_text`、`function_call`、`web_search_call`、`file_search_call` 逐 item 归一为 canonical 事件，citations/annotations 经 P15-5 的 ServerToolEvent 与 Citation 类型回填。
3. **reasoning 持久化往返** —— 目的：捕获 `reasoning.summary` 与 `reasoning.encrypted_content`，按 P15-7 / ADR-032 存为 `ReasoningItem`（事件只含 `protected_blob_ref`；encrypted_content 作为作用域加密的不透明 blob，不解码、不入 Event/日志/Keychain/GUI），多轮回灌保证 reasoning 连续。
4. **传输选择与降级** —— 目的：经 P15-8 能力协商，模型支持 Responses 时走 Responses，否则降级到 P6-1 Chat Completions（web_search 等不可用时退化为客户端工具或拒绝）。
5. **server tool 事件接线** —— 目的：把搜索、代码/ shell、patch、图像、computer、MCP 与 skills/tool-search 的调用、进度、结果和 citations 经 P15-5 规范化为 ServerToolEvent；大型输出走 Artifact，后续轮次以 `ProviderTranscript` 续接原生 output item / response reference。只有客户端 Function Calling 的 `CoreSuppliedResult` 才映射 `function_call_output`，server tool 不经过该路径。
6. **Programmatic Tool Calling / API Multi-Agent** —— 目的：将程序化工具编排与 Provider-side Multi-Agent 表达为明确 capability 和事件，不把 Provider 的内部 worker 伪装成 Pawork 本地 P12 worker；不支持时显式 `Unsupported`。
7. **错误归一** —— 目的：Responses 特有错误（vector store 未就绪、code_interpreter/hosted shell 超时、computer_use 需确认、MCP/skill 不可用）归一为统一 ProviderError，重试建议与 P2-10 一致。
8. **分段交付与 Mock smoke** —— 目的：先完成 Responses + function/reasoning，再按 capability 逐组接入 hosted tools；每组只跑录制夹具的 item→event、citations、reasoning 与降级 smoke，完整矩阵统一落 P15-9。

## 主要产出物

- `provider-openai` Responses 子适配器 + 传输选择
- Responses ↔ canonical 事件映射夹具与定向测试

## 验收标准

- [ ] Responses output items 正确归一为 ProviderStreamEvent（reasoning/message/function/server tool）
- [ ] web_search / file_search 的 citations 经 P15-5 归一为 Citation，可重放
- [ ] Hosted Shell / Apply Patch / Code Interpreter / Image Generation / Computer Use 的进度与结果进入 canonical 事件，大输出走 Artifact
- [ ] server-side MCP / Skills / Tool Search 可经 capability negotiation 启用；Programmatic Tool Calling / API Multi-Agent 不支持时显式降级或拒绝
- [ ] `reasoning.encrypted_content` 只经 Protected Blob Store 引用往返，不入 Event/日志/Keychain/GUI（P15-7 / ADR-032）
- [ ] server tool 续接走 `ProviderTranscript`；只有客户端 Function Calling 结果可映射 `function_call_output`
- [ ] 不支持 Responses 时降级到 Chat Completions，行为可观察（Mock smoke）
- [ ] 不在 Core 走 OpenAI 名称分支（`no_provider_branch` 断言）
- [ ] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [P15-7](P15-7-reasoning-state.md) · [P15-8](P15-8-capability-discovery.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：Responses 与 Chat Completions 同 crate 共存；按「reasoning+function → search/MCP/tool-search → code/shell/patch → image/computer → programmatic/multi-agent」分段交付，每段保持数小时级写入集，完整能力矩阵只在 P15-9 集中验收。
