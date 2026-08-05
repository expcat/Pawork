# P2-5：OpenAI-compatible 适配

> Phase 2 · 首个真实 Provider · 状态：🟡未开始 · 依赖：P2-2、P2-4

**最终目的**：实现 OpenAI-compatible 适配器（canonical 转换 + 流式组装），用一个适配同时覆盖云端兼容接口与多数本地服务（Ollama/vLLM/LM Studio），是关键路径上第一个真实 provider。

**涉及范围**：`provider-openai-compatible`

## 细分步骤

1. **canonical 请求 → OpenAI 请求** —— 目的：provider 无关输入。
2. **OpenAI 响应 → canonical 流式事件** —— 目的：统一事件输出。
3. **连云端 + 本地服务验证** —— 目的：覆盖两类后端。
4. **错误归一** —— 目的：错误符合 canonical ProviderError。

## 主要产出物

- `provider-openai-compatible` crate

## 验收标准

- [ ] 文本对话与 tool call 流式可用
- [ ] 云端与本地服务均可连

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ROADMAP](../ROADMAP.md)
