# P6-12：阿里 Qwen（DashScope）适配

> Phase 6 · 主要 Provider · 状态：🟡未开始 · 依赖：P2-11、P6-5

**最终目的**：接入阿里通义千问 Qwen 模型族（Qwen3、Qwen3-Max、Qwen-Plus 等），以 DashScope API Key 直连，复用 DashScope OpenAI-compatible 接口，核心 Agent 不含 Qwen 特例。

**涉及范围**：新增 `provider-qwen` crate

## 细分步骤

1. **canonical 转换 + 流式组装** —— 目的：接入 `dashscope.aliyuncs.com/compatible-mode/v1` 的 OpenAI-compatible 接口，复用 OpenAI-compatible 解析路径但保留独立 crate 以隔离差异。
2. **鉴权** —— 目的：DashScope API Key（`Authorization: Bearer sk-...`）直连。
3. **Qwen3 thinking 归一** —— 目的：把 Qwen3 的 `enable_thinking` 开关与 reasoning 流映射到 canonical `ThinkingDelta`（P6-5），非 thinking 模型不注入该字段。
4. **通过 Contract Tests + 差分测试** —— 目的：达标，行为对照官方文档与 OpenAI-compatible 基线。
5. **错误归一** —— 目的：把限流（按模型 RPM/TPM）、内容审核、无效 API Key 归一为统一 ProviderError。

## 主要产出物

- `provider-qwen` crate + contract 结果

## 验收标准

- [ ] 通过统一 Contract Tests
- [ ] Qwen3 thinking 流归一到 `ThinkingDelta`
- [ ] 不在 Agent Core 走 Qwen 名称分支

**相关文档**：[providers](../docs/features/providers.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不引入 Provider SDK；DashScope compatible-mode 与 OpenAI Chat Completions 同构，复用 OpenAI-compatible 字段清单与解析逻辑。注意 DashScope 有 `enable_search`、`enable_thinking` 等扩展参数，经 P6-9 provider options 透传，不在 canonical domain 特例化。
