# P15-4：xAI Responses 适配

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified（有界：domain + adapter verified、host composition deferred） · 依赖：P15-1、P15-5、P15-7、P15-8、P6-10

**最终目的**：为 `provider-xai` 增加 xAI Responses 入口，原生覆盖 Web Search、X Search、Code Execution、Collection Search 与 server-side MCP，作为 P6-10 Chat Completions 之外的现代传输。Grok reasoning、tool lifecycle 与 `sources`/`citations` 经 P15-5/P15-7 归一，传输经 P15-8 协商，Core 不感知 xAI 名称。

**涉及范围**：`provider-xai`（Responses 子适配器 + Live Search）、`provider-runtime`（复用 ServerToolEvent / ReasoningItem）；与 P6-10 既有 Chat Completions 路径同 crate 共存。

## 细分步骤

1. **Responses 请求转换** —— 目的：canonical 请求 → xAI Responses `input`；把 hosted capabilities 翻译为 Web Search、X Search、Code Execution、Collection Search 与 MCP 配置，保留 `previous_response_id` 与 reasoning 回灌（P15-7）。
2. **output items → ProviderStreamEvent** —— 目的：把 xAI Responses 的 reasoning、message、function_call、live_search 调用逐 item 归一为 canonical 事件，`sources`/`citations` 经 P15-5 归一为 Citation/Source。
3. **搜索与集合结果归一** —— 目的：Web/X Search 和 Collection Search 的 `sources`、post/document 标识与片段映射到统一 Citation/Source，保留来源种类和可追溯原始元数据；后续轮次走 `ProviderTranscript` 续接 xAI 原生 output item / continuation reference，不经过客户端 function-result 路径。
4. **Code Execution / MCP 生命周期** —— 目的：把执行开始、输出、完成与 MCP tool 调用映射到 P15-5 事件，大输出走 Artifact；MCP Secret 仅经授权注入，不写事件或日志。
5. **reasoning 持久化往返** —— 目的：捕获 Grok reasoning 输出与 continuation 凭证，按 P15-7 / ADR-032 存为不透明 `ReasoningItem`（事件只存 `protected_blob_ref`），多轮保持连续。
6. **双鉴权与传输选择** —— 目的：复用 P6-10 的 API Key / OAuth 订阅鉴权；经 P15-8 协商现代能力，不支持时降级到 P6-10 Chat Completions 或明确拒绝对应 hosted tool。
7. **错误归一** —— 目的：搜索/集合/执行/MCP 的配额、权限与 billing 错误归一为 ProviderError，带可执行重试或降级建议。
8. **分段交付与 Mock smoke** —— 目的：先做 Responses + reasoning，再分组接入 Web/X、Collections、Code、MCP；每组只跑定向夹具，完整矩阵落 P15-9。

## 主要产出物

- `provider-xai` Responses 子适配器 + Live Search
- xAI sources/citations → 统一 Citation/Source 映射夹具

## 验收标准

## 验收标准

- [x] xAI Responses output items 正确归一为 ProviderStreamEvent（reasoning/message/function/live search）
- [x] Live Search 的 `sources` 经 P15-5 归一为 Citation/Source，可重放（Mock smoke）
- [x] Web Search、X Search、Collection Search、Code Execution 与 server-side MCP 均有 canonical capability、事件与显式降级语义
- [x] reasoning 经 Protected Blob Store 引用往返，加密凭证不入 Event/日志/Keychain/GUI（P15-7 / ADR-032）
- [x] hosted tool 续接走 `ProviderTranscript`，不映射客户端 function-result 字段
- [x] 不支持 Responses 时降级到 P6-10 Chat Completions，行为可观察（降级用例）
- [x] 双鉴权（API Key / OAuth 订阅）在 Responses 路径均可用（Mock smoke）
- [x] 不在 Core 走 xAI 名称分支（`no_provider_branch` 断言）
- [x] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

## 实现摘要（2026-08-12）

- `provider-xai` 新增 `responses` 模块：请求转换（`to_responses_body`，hosted tools 仅放行协商通过的类别）、SSE 组装器（`ResponsesStreamAssembler`：reasoning/message/function_call/web_search_call/x_search_call/file_search_call/code_interpreter_call/mcp_call → canonical 事件；[P15-10](P15-10-review-remediation.md) 判定保留 adapter-local，不强制下沉）、Live Search source 归一（`live_search_source_to_source`）、reasoning Protected Blob 往返（统一 `provider-runtime::reasoning::ReasoningProtector` + `InMemoryReasoningProtector`，复用 P15-7 `parse_responses_reasoning`/`to_reasoning_item`/`to_responses_input_reasoning`；持久化接线延 P18-3/4/14）、错误归一（`normalize_responses_error`）与协商要求折叠（`requirements_from_request`）。
- `provider.rs` 双传输接线：`resolve_capabilities` 经 `CapabilityNegotiator`（不读 Provider 名）选择 transport，Responses 走 `/responses` + SSE 组装，其余降级到 P6-10 `OpenAiCompatibleProvider`；`builtin_models()` 中 `grok-4`/`grok-4-fast` 声明 `transport = Responses`（Live Search / Collection / Code / MCP 标签 + citations + encrypted reasoning），`grok-3`/`grok-2` 保留 Chat Completions 基线；`with_reasoning_protector` host 注入点；双鉴权复用 P6-10。
- Mock smoke（`tests/responses.rs`，wiremock）：Responses+reasoning item→event、Live Search sources、Web/X/Collection/Code/MCP 事件、reasoning 往返、双鉴权（API Key / OAuth bearer）、降级到 Chat Completions、hosted tool 仅在协商通过时入 body、错误归一与 `no_provider_branch` 断言。
- 附带修复：`tests/contract.rs` 补 `reasoning: None` 字段（phase-15 漏改），并将 P6-10 Chat Completions 契约用例模型由 `grok-4` 改为 `grok-2`（`grok-4` 现声明 Responses transport，契约用例需用 Chat Completions 基线模型）。

Validation Level: L1 · Affected crates: provider-xai · Validated: `cargo check -p provider-xai` / `cargo test -p provider-xai`（17 unit + 5 contract + 12 responses smoke）/ `cargo clippy -p provider-xai --all-targets -- -D warnings` · Full workspace gate: NOT RUN（未命中升级条件）

**相关文档**：[providers](../docs/features/providers.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [P15-7](P15-7-reasoning-state.md) · [P15-8](P15-8-capability-discovery.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：按「Responses+reasoning → Web/X Search → Collections → Code/MCP」分段交付；端点字段变化以夹具锁定行为，订阅模式标注需跟进真实契约。
