# P6-2：Anthropic 适配

> Phase 6 · 主要 Provider · 状态：🟢已完成 · 依赖：P2-11
> **边界与扩展**：本任务交付 Anthropic **Messages** 基线适配（canonical 转换 + thinking/tool/cache 差异 + 错误归一）。**现代 Messages** 升级（Structured Outputs `output_config.format`、request-level effort、adaptive/interleaved thinking、modern prompt cache、thinking `signature` 往返，以及 Web Search/Web Fetch/Code Execution/Advisor/Tool Search/MCP Connector/Memory/Bash/Text Editor/Computer Use 等 server/client tools）**不在本任务范围**，由 [P15-3](P15-3-anthropic-modern-messages.md) 独立承接，二者同 crate 并存。状态 🟢 仅表示 Messages 基线已落地，**不代表现代能力已完整适配**；勿在本任务重复实现，也勿据本状态误判 P15-3 完成。

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
