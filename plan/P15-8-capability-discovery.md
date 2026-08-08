# P15-8：Capability Discovery（能力协商与传输选择）

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-4、P2-7、P15-1、P15-5、P15-7

**最终目的**：在请求发出前，基于「模型 × 工具能力标签」做能力协商——检测当前模型/Provider 支持哪些现代能力（Responses 传输、server tools、thinking signature、code execution、computer use 等），据此选择传输路径与降级策略，使 P15-2/3/4 不必各自硬编码「能不能用」。协商结果可持久化、可观测，Core 仍不感知 Provider 名称。

**涉及范围**：`provider-runtime`（CapabilityNegotiator + 能力枚举）、`agent-domain`（能力标签，复用 P15-1 `ToolDescriptor.capabilities`）、`model-registry`（模型能力元数据，复用 P2-7）；不新增 crate。

## 细分步骤

1. **ModelCapabilities v2** —— 目的：定义稳定 capability vocabulary，至少覆盖 Responses/Chat 传输、Web Search、Web Fetch、File/Collection Search、X Search、Code Execution、Hosted Shell、Provider Apply Patch、Computer Use、Image Generation、server-side MCP、Tool Search、Citations/Sources、Memory、Programmatic Tool Calling、server-side Multi-Agent、Structured Output、Prompt Cache，以及可枚举的 reasoning effort levels（`none/low/medium/high/xhigh/max`）、Thinking Signature/Encrypted/Interleaved state；与 P15-1 工具标签对齐。
2. **能力来源** —— 目的：能力来自三处并按优先级合并——(a) model-registry（P2-7）静态声明；(b) Provider 能力探测（运行时一次，可缓存）；(c) 夹具/配置覆盖；取交集而非并集，避免声明了但实际不可用。
3. **CapabilityNegotiator** —— 目的：提供 `negotiate(model_id, requested_capabilities) -> ResolvedCapabilities { supported, unsupported, chosen_transport, fallback }`，按请求侧 `hosted_tools`（P15-1）与 reasoning 需求（P15-7）匹配；输出可观测的协商记录。
4. **传输选择决策** —— 目的：协商结果驱动 P15-2/3/4 的传输选择——模型支持 Responses / 现代 Messages 时走现代路径，否则降级到 Chat Completions / P6 基线；降级路径写入协商记录，供诊断与 P15-9 夹具。
5. **降级与回退策略** —— 目的：被请求但未支持的能力（如 server web_search）明确降级——退化为客户端工具（P15-6 搜索）或拒绝并给可读原因；禁止静默丢弃或伪造。
6. **协商记录持久化** —— 目的：协商结果（supported/unsupported/chosen transport/fallback）落入可观测/诊断通道（P1-9/P1-11），便于排查「为什么走了降级」。
7. **Mock smoke 协商 + 降级** —— 目的：构造模型 A（全能力）、模型 B（部分）、模型 C（仅基线），验证协商取交集、传输选择正确、降级路径可观察；真实 Provider 探测与全量门禁落 P15-9。

## 主要产出物

- `provider-runtime`：`CapabilityNegotiator` + `Capability`/`ResolvedCapabilities`
- `model-registry`：模型能力元数据字段（复用 P2-7）
- Mock smoke：协商取交集 / 传输选择 / 降级可观察用例

## 验收标准

- [ ] 协商对「请求能力 × 模型支持」取交集，未支持项进入 `unsupported`（Mock smoke）
- [ ] 传输选择由协商结果驱动，P15-2/3/4 不各自硬编码（断言）
- [ ] 降级路径明确（退化客户端工具或拒绝+可读原因），不静默丢弃或伪造
- [ ] 协商记录可观测（P1-9/P1-11），可解释「为何降级」
- [ ] 协商不引入 Provider 名称分支逻辑（`no_provider_branch` 断言）
- [ ] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

**相关文档**：[providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [observability](../docs/features/observability.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-2](P15-2-openai-responses.md) · [P15-3](P15-3-anthropic-modern-messages.md) · [P15-4](P15-4-xai-responses.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：纯领域 + 协商逻辑，不新增依赖；Provider 运行时探测结果缓存复用 P2-7 model-registry 机制，避免重复请求。
