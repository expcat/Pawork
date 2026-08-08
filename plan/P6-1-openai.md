# P6-1：OpenAI 适配

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P2-11
> **边界与扩展**：本任务交付 OpenAI **Chat Completions** 原生适配（canonical 转换 + 流式 + tool call/thinking/image 差异 + 错误归一）。OpenAI **Responses API** 及其原生能力（reasoning items、Web/File Search、Image Generation、Code Interpreter、Hosted Shell、Apply Patch、Skills、Computer Use、server-side MCP、Tool Search、Programmatic Tool Calling、API Multi-Agent、citations）**不在本任务范围**，由 [P15-2](P15-2-openai-responses.md) 独立承接，二者同 crate 并存、由 [P15-8](P15-8-capability-discovery.md) 协商选择传输路径，不支持 Responses 时降级回本任务的 Chat Completions。状态 🟢 仅表示 Chat Completions 基线已落地，**不代表 OpenAI 现代能力已完整适配**；实现扩展勿在本任务内重复，也勿据本状态误判 P15-2 完成。

**最终目的**：实现 OpenAI 原生适配并通过统一 Contract Tests，覆盖关键能力，核心 Agent 不含 OpenAI 特例。

**涉及范围**：`provider-openai`

## 细分步骤

1. **canonical 转换 + 流式组装** —— 目的：OpenAI 原生协议接入。
2. **覆盖差异（tool call/thinking/image）** —— 目的：行为对齐。
3. **通过 Contract Tests** —— 目的：达标。
4. **错误归一** —— 目的：统一 ProviderError。

## 主要产出物

- `provider-openai` crate + contract 结果

## 验收标准

- [ ] 通过统一 Contract Tests

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不引入整套 Provider SDK（与 ADR-002 canonical domain 冲突）；以 async-openai 的类型定义作为请求 / 响应字段清单参照（其由 OpenAPI 生成、维护活跃），行为以官方文档与 Pi 做差分测试。
