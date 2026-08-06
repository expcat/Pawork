# P6-1：OpenAI 适配

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P2-11

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
