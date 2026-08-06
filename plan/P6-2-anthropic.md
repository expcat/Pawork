# P6-2：Anthropic 适配

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P2-11

**最终目的**：实现 Anthropic 适配并通过 Contract Tests，覆盖其 thinking / tool / cache 等差异，核心无特例。

**涉及范围**：`provider-anthropic`

## 细分步骤

1. **canonical 转换 + 流式组装** —— 目的：Anthropic 协议接入。
2. **thinking / tool / cache 差异处理** —— 目的：行为对齐。
3. **通过 Contract Tests** —— 目的：达标。
4. **错误归一** —— 目的：统一 ProviderError。

## 主要产出物

- `provider-anthropic` crate + contract 结果

## 验收标准

- [ ] 通过统一 Contract Tests

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不引入整套 Provider SDK；Anthropic 的 Rust 生态碎片化、无稳定依赖，以官方 API 文档 + async-openai 式类型清单为字段参照，行为做差分测试。
