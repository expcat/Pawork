# P15-4：xAI Responses 适配

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P15-1、P15-5、P15-7、P15-8、P6-10

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

- [ ] xAI Responses output items 正确归一为 ProviderStreamEvent（reasoning/message/function/live search）
- [ ] Live Search 的 `sources` 经 P15-5 归一为 Citation/Source，可重放（Mock smoke）
- [ ] Web Search、X Search、Collection Search、Code Execution 与 server-side MCP 均有 canonical capability、事件与显式降级语义
- [ ] reasoning 经 Protected Blob Store 引用往返，加密凭证不入 Event/日志/Keychain/GUI（P15-7 / ADR-032）
- [ ] hosted tool 续接走 `ProviderTranscript`，不映射客户端 function-result 字段
- [ ] 不支持 Responses 时降级到 P6-10 Chat Completions，行为可观察（降级用例）
- [ ] 双鉴权（API Key / OAuth 订阅）在 Responses 路径均可用（Mock smoke）
- [ ] 不在 Core 走 xAI 名称分支（`no_provider_branch` 断言）
- [ ] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

**相关文档**：[providers](../docs/features/providers.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [P15-7](P15-7-reasoning-state.md) · [P15-8](P15-8-capability-discovery.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：按「Responses+reasoning → Web/X Search → Collections → Code/MCP」分段交付；端点字段变化以夹具锁定行为，订阅模式标注需跟进真实契约。
