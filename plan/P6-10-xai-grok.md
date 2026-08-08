# P6-10：xAI Grok 适配

> Phase 6 · 主要 Provider · 状态：🟡未开始 · 依赖：P2-11、P6-4、P6-5
> **边界与扩展**：本任务交付 xAI Grok 的 **Chat Completions**（OpenAI-compatible）+ API Key / OAuth 订阅双鉴权 + reasoning 归一基线。xAI **Responses API** 及 Live Search 原生能力（Web/X Search、Collection Search、Code Execution、server-side MCP、`sources`/`citations` 归一）**不在本任务范围**，由 [P15-4](P15-4-xai-responses.md) 独立承接，二者同 crate 并存、由 [P15-8](P15-8-capability-discovery.md) 协商传输路径、复用本任务的双鉴权。状态 🟡 仅表示 Chat Completions 基线排期；勿在本任务实现 Responses 现代能力，也勿把 Chat Completions 完成误判为 xAI 能力完整。

**最终目的**：接入 xAI Grok，同时支持两条鉴权路径——API Key 直连（`api.x.ai`，OpenAI-compatible）与 OAuth 订阅授权（消费级 Grok / SuperGrok 订阅登录），核心 Agent 不含 Grok 特例。

**涉及范围**：新增 `provider-xai` crate

## 细分步骤

1. **canonical 转换 + 流式组装** —— 目的：以 `api.x.ai` 的 Chat Completions（OpenAI-compatible 形态）接入，复用 OpenAI-compatible 解析路径但保留独立 crate 以隔离差异。
2. **API Key 直连模式** —— 目的：把 `Authorization: Bearer <key>` 作为默认鉴权，覆盖 grok-2/3/4、grok-fast 等模型族。
3. **OAuth 订阅模式** —— 目的：复用 P6-4 的 PKCE / Device Flow 对消费级 Grok 账户登录（会话/订阅授权），保存短期 access token 与 refresh，按订阅的请求配额使用，而非按 token 计费。
4. **reasoning / thinking 流式归一** —— 目的：把 Grok 的 reasoning 输出映射到 canonical `ThinkingDelta`（P6-5），与 OpenAI / Anthropic 行为一致。
5. **通过 Contract Tests + 差分测试** —— 目的：达标，行为对照官方文档与 OpenAI-compatible 基线。
6. **错误归一** —— 目的：把 429 订阅速率限制、401 token 过期、403 订阅未覆盖模型等归一为统一 ProviderError，订阅速率限制带重试建议。

## 主要产出物

- `provider-xai` crate + contract 结果
- 双模式鉴权说明（API Key / OAuth 订阅）

## 验收标准

- [ ] API Key 模式通过统一 Contract Tests
- [ ] OAuth 订阅模式可完成登录、刷新、流式调用
- [ ] reasoning 流归一到 `ThinkingDelta`
- [ ] 不在 Agent Core 走 Grok 名称分支

**相关文档**：[providers](../docs/features/providers.md) · [usage-quota](../docs/features/usage-quota.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 review）**：不引入整套 Provider SDK；`api.x.ai` 端点与 OpenAI Chat Completions 同构，复用 OpenAI-compatible 的字段清单与解析逻辑。OAuth 订阅端点为消费级、非稳定公开契约——若 xAI 订阅 OAuth 不提供稳定契约，保留 API Key 模式作为默认支持路径，订阅模式标注「需跟进 xAI 端点变化」。
