# P15-8：Capability Discovery（能力协商与传输选择）

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：Delivered · 依赖：P0-4、P2-7、P15-1、P15-5、P15-7

**最终目的**：在请求发出前，基于「模型 × 工具能力标签」做能力协商——检测当前模型/Provider 支持哪些现代能力（Responses 传输、server tools、thinking signature、code execution、computer use 等），据此选择传输路径与降级策略，使 P15-2/3/4 不必各自硬编码「能不能用」。协商结果可持久化、可观测，Core 仍不感知 Provider 名称。

**涉及范围**：`provider-api`（canonical 能力 vocabulary / 协商记录）、`provider-runtime`（CapabilityNegotiator）、`agent-domain`（能力标签，复用 P15-1 `ToolDescriptor.capabilities`）、`model-registry`（三源证据与 Provider 级探测缓存，复用 P2-7）；不新增 crate。

## 细分步骤

1. **ModelCapabilities v2** —— 目的：按四条独立轴定义稳定 vocabulary：(a) `ModelTransport` 表达 Responses / Messages / Chat 等 transport；(b) 复用 P15-1 的 `ToolCapabilityTag` 表达 Web Search、Web Fetch、File/Collection Search、X Search、Code Execution、Hosted Shell、Provider Apply Patch、Computer Use、Image Generation、server-side MCP、Tool Search、Memory、Programmatic Tool Calling 与 server-side Multi-Agent；(c) 模型/响应能力表达 Citations/Sources、Structured Output、Prompt Cache；(d) `ReasoningEffort { None, Low, Medium, High, XHigh, Max }` 与 Signature/Encrypted/Interleaved state。v2 新字段逐项 `serde(default)`，旧数据缺字段时 fail-closed。
2. **能力来源** —— 目的：能力来自三处：(a) model-registry 静态声明；(b) Provider 能力探测；(c) 夹具/配置 override。所有**已出现来源逐项取交集**，来源整体缺失不约束；override 只能收窄，不能创造支持。探测按 Provider 缓存，同一 Provider 的并发调用共享一次发现，锁不跨 `await`。
3. **CapabilityNegotiator（canonical effort 的协商入口）** —— 目的：以能力证据快照和请求要求为输入，提供纯函数式 `negotiate(...) -> ResolvedCapabilities { requested, supported, unsupported, chosen_transport, fallback }`；不触网、不读取 Provider 名或 wall-clock。`ReasoningConfig { effort: ReasoningEffort, state }` 是现代 canonical 权威字段；显式 effort 优先，旧 `ThinkingConfig.level` 只在缺省时派生，`XHigh/Max` 进入旧 adapter 时显式 clamp 为 `High` 并记录，不形成双轨。effort 不经 `provider_options`（P6-9）。
4. **传输选择决策** —— 目的：transport 支持由逐模型 `ModelCapabilities` 声明，禁止按 Provider 名推断。协商结果驱动 P15-2/3/4：模型支持 Responses / 现代 Messages 时走现代路径，否则降级到 Chat Completions / P6 基线；降级路径写入协商记录，供诊断与 P15-9 夹具。
5. **降级与回退策略** —— 目的：区分两层交集：证据层的来源缺失不约束；请求层中 capability 未声明即进入 `unsupported`。每项未支持能力必须显式选择 Client Tool 或 Reject + 可读原因，满足 `requested = supported ∪ unsupported`；禁止静默丢弃、静默 clamp 或伪造。
6. **协商记录持久化** —— 目的：协商结果随 `RunRequest → ProviderLoopConfig → CanonicalModelRequest` 保存，并在 Provider 请求前以稳定 `provider_capability_negotiated` Diagnostic 事件落入可观测/诊断通道（P1-9/P1-11）；重试复用同一记录，便于排查「为什么走了降级」。
7. **Mock smoke 协商 + 降级** —— 目的：构造模型 A（全能力）、模型 B（部分）、模型 C（仅基线），验证协商取交集、传输选择正确、降级路径可观察；真实 Provider 探测与全量门禁落 P15-9。

## 主要产出物

- `provider-runtime`：`CapabilityNegotiator` + `Capability`/`ResolvedCapabilities`
- `model-registry`：模型能力元数据字段（复用 P2-7）
- Mock smoke：协商取交集 / 传输选择 / 降级可观察用例

## 验收标准

- [x] 协商对「请求能力 × 模型支持」取交集，未支持项进入 `unsupported`（Mock smoke）
- [x] 传输选择由协商结果驱动（`CapabilityNegotiator::choose_transport`，P15-2/3/4 不各自硬编码）
- [x] 降级路径明确（ClientTool / LegacyTransport / ClampedEffort / Reject+可读原因），不静默丢弃或伪造
- [x] 协商记录可观测（稳定 `provider_capability_negotiated` Diagnostic，可解释「为何降级」）
- [x] 协商不引入 Provider 名称分支逻辑（`no_provider_branch` 断言：provider=Some/None 结果一致）
- [x] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

**相关文档**：[providers](../docs/features/providers.md) · [models](../docs/features/models.md) · [observability](../docs/features/observability.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-2](P15-2-openai-responses.md) · [P15-3](P15-3-anthropic-modern-messages.md) · [P15-4](P15-4-xai-responses.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：纯领域 + 协商逻辑，不新增依赖；Provider 运行时探测结果缓存复用 P2-7 model-registry 机制，避免重复请求。
