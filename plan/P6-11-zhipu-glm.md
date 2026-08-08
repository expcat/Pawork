# P6-11：智谱 GLM 适配

> Phase 6 · 主要 Provider · 状态：🟡未开始 · 依赖：P2-11、P6-5

**最终目的**：接入智谱 AI GLM 模型族（GLM-4.6、GLM-4.5-Air、GLM-4-Air 等），以 API Key 直连，复用 BigModel OpenAI-compatible 接口，核心 Agent 不含 GLM 特例。

**涉及范围**：新增 `provider-zhipu` crate

## 细分步骤

1. **canonical 转换 + 流式组装** —— 目的：接入 `open.bigmodel.cn/api/paas/v4` 的 OpenAI-compatible 接口，复用 OpenAI-compatible 解析路径但保留独立 crate 以隔离差异。
2. **鉴权** —— 目的：支持 BigModel API Key（`id.secret`）直连；保留独立 crate 以便后续如需 JWT 时可注入而不污染通用适配器。
3. **GLM-4.6 reasoning_content 归一** —— 目的：把 GLM 的 `reasoning_content` 流映射到 canonical `ThinkingDelta`（P6-5）。
4. **通过 Contract Tests + 差分测试** —— 目的：达标，行为对照官方文档与 OpenAI-compatible 基线。
5. **错误归一** —— 目的：把限流、余额不足、内容审核等归一为统一 ProviderError，携带可重试判定。

## 主要产出物

- `provider-zhipu` crate + contract 结果

## 验收标准

- [ ] 通过统一 Contract Tests
- [ ] GLM-4.6 reasoning 流归一到 `ThinkingDelta`
- [ ] 不在 Agent Core 走 GLM 名称分支

**相关文档**：[providers](../docs/features/providers.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不引入 Provider SDK；BigModel v4 接口为 OpenAI-compatible 形态，复用 OpenAI-compatible 字段清单与解析逻辑。鉴权用 API Key（`Authorization: Bearer <api_key>`，BigModel 现已支持 Bearer 直传，无需客户端生成 JWT），避免在客户端实现 JWT 签名。
