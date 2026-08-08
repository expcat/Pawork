# P15-3：Anthropic Modern Messages 适配

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P15-1、P15-5、P15-7、P15-8、P6-2

**最终目的**：把 `provider-anthropic` 升级到现代 Messages：`output_config.format` Structured Outputs、request-level effort、adaptive/interleaved thinking、modern prompt cache，以及 Web Search、Web Fetch、Code Execution、Advisor、Tool Search、MCP Connector、Memory、Bash、Text Editor、Computer Use 等 server/client tool 形态。工具结果与 citations 经 P15-5 归一，reasoning/signature 经 P15-7 持久化，Core 不感知 Anthropic 名称。

**涉及范围**：`provider-anthropic`（现代 server tools 与 thinking signature）、`provider-runtime`（复用 ServerToolEvent / ReasoningItem 通道）；与 P6-2 既有 Messages 路径同 crate 共存。

## 细分步骤

1. **现代请求字段** —— 目的：把 canonical Structured Output、effort、adaptive thinking 与 prompt-cache 策略映射到 Anthropic 当前 Messages 字段；`output_config.format` 走原生 schema，不再退化为 system prompt，effort level 不再固定为 `budget_tokens`。
2. **hosted/client tools → Anthropic tools** —— 目的：按 capability 映射 Web Search、Web Fetch、Code Execution、Advisor、Tool Search、MCP Connector、Memory、Bash、Text Editor 与 Computer Use；客户端 function 工具仍走既有 `name/input_schema`，不同执行位点不得混用。
3. **server tool 结果归一** —— 目的：把各类 `server_tool_use` 与对应 result block 归一为 P15-5 的 ServerToolEvent/Citation/Source，大输出走 Artifact，并按 Anthropic 要求回传 `tool_result`。
4. **thinking signature 往返** —— 目的：捕获 thinking block 的 `signature` 与 thinking 内容，按 P15-7 存为不透明 `ReasoningItem`，支持 adaptive/extended thinking 跨轮连续。
5. **interleaved thinking** —— 目的：支持 thinking 与 tool_use 交错输出，按事件顺序归一（ThinkingDelta / ServerToolEvent 交错不丢序）。
6. **能力协商与降级** —— 目的：经 P15-8 检测 Structured Output、effort、server tools、tool search 与 thinking 支持；不支持时显式退化到 P6-2 基线或返回 `Unsupported`，禁止静默丢字段。
7. **citation/source 规范化** —— 目的：Anthropic citations 映射到 P15-5 统一 Citation/Source，与 OpenAI/xAI 口径对齐。
8. **分段交付与 Mock smoke** —— 目的：先做 structured output + effort/adaptive thinking，再分组接入 search/execution/MCP/tool-search/memory/computer；每组只跑定向夹具，完整矩阵落 P15-9。

## 主要产出物

- `provider-anthropic` 现代 server tools + thinking signature 子路径
- Anthropic citation → 统一 Citation/Source 映射夹具

## 验收标准

- [ ] web_search / code_execution 的 server tool 结果经 P15-5 归一为 Citation/Source（Mock smoke）
- [ ] `output_config.format`、effort、adaptive thinking 与 modern prompt cache 原生映射，不再用 system prompt 或固定 budget 静默替代
- [ ] Web Fetch / Advisor / Tool Search / MCP Connector / Memory / Bash / Text Editor / Computer Use 均有 capability、执行位点与显式降级语义
- [ ] thinking `signature` 作不透明凭证往返，多轮 extended thinking 连续（P15-7）
- [ ] interleaved thinking 与 tool_use 顺序保持（顺序断言）
- [ ] 不支持 server tools 的模型降级到 function calling，不报错（降级用例）
- [ ] 不在 Core 走 Anthropic 名称分支（`no_provider_branch` 断言）
- [ ] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [P15-7](P15-7-reasoning-state.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：按「structured output+effort+thinking → search/tool-search/MCP → execution/editor/memory → computer」分段交付；不新增整套 SDK，完整矩阵在 P15-9 集中验收。
