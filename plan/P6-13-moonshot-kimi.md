# P6-13：Moonshot Kimi 适配

> Phase 6 · 主要 Provider · 状态：🟡未开始 · 依赖：P2-11、P6-5

**最终目的**：接入 Moonshot AI Kimi 模型族（Kimi K2、moonshot-v1 等），以 API Key 直连，复用 Moonshot OpenAI-compatible 接口，核心 Agent 不含 Kimi 特例。

**涉及范围**：新增 `provider-moonshot` crate

## 细分步骤

1. **canonical 转换 + 流式组装** —— 目的：接入 `api.moonshot.cn/v1` 的 OpenAI-compatible 接口，复用 OpenAI-compatible 解析路径但保留独立 crate 以隔离差异。
2. **鉴权** —— 目的：Moonshot API Key（`Authorization: Bearer sk-...`）直连。
3. **Kimi K2 reasoning 归一** —— 目的：把 Kimi 的 reasoning 流映射到 canonical `ThinkingDelta`（P6-5）。
4. **通过 Contract Tests + 差分测试** —— 目的：达标，行为对照官方文档与 OpenAI-compatible 基线。
5. **错误归一** —— 目的：把限流、余额不足、内容审核归一为统一 ProviderError。

## 主要产出物

- `provider-moonshot` crate + contract 结果

## 验收标准

- [ ] 通过统一 Contract Tests
- [ ] Kimi reasoning 流归一到 `ThinkingDelta`
- [ ] 不在 Agent Core 走 Kimi 名称分支

**相关文档**：[providers](../docs/features/providers.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不引入 Provider SDK；Moonshot Platform 接口与 OpenAI Chat Completions 同构，复用 OpenAI-compatible 字段清单与解析逻辑。Kimi 的 `/v1/users/me/balance` 余额接口是少数原生支持额度查询的国内供应商接口，落地见 P14-2。
