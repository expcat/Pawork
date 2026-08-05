# P0-4：Provider 协议

> Phase 0 · 架构与协议冻结 · 状态：🟢已完成 · 依赖：P0-2

**最终目的**：冻结 Provider 的 canonical 契约（请求/流式事件/错误）。让 Agent Engine 只依赖 canonical domain、永不按 provider 名走特例（ADR-002）。

**涉及范围**：`provider-api`

## 细分步骤

1. **定义 CanonicalModelRequest** —— messages、model、tools、thinking、image、max_tokens、budget 等。目的：provider 无关的统一请求。
2. **定义 ProviderStreamEvent** —— text delta / tool call delta / thinking / usage / stop / error。目的：流式组装的统一事件。
3. **定义 ModelProvider Trait** —— `async fn stream(request, sink, cancel) -> ModelResponseSummary`，由 push sink 提供流式事件与背压。目的：所有 provider 的统一抽象。
4. **定义 ProviderError** —— 可重试判定、建议重试时间、错误类别。目的：为上层重试归一提供依据。
5. **守住解耦约束** —— 不依赖具体 HTTP/Tauri，仅依赖 `agent-domain` + `async-trait`。目的：守住解耦红线。

## 主要产出物

- `provider-api` crate：`ModelProvider` Trait + canonical 类型 + `ProviderError`

## 验收标准

- [x] Trait 不绑定 HTTP/Tauri
- [x] canonical 覆盖 text/tool/image/thinking/usage/stop
- [x] 错误可分类（可重试/不可重试/超时/限流/鉴权）

**相关文档**：[providers](../docs/features/providers.md) · [ADR-002 解耦](../docs/adr/ADR-002-agent-engine-provider-decoupled.md) · [ADR-015 Contract Tests](../docs/adr/ADR-015-provider-contract-tests.md) · [ROADMAP](../ROADMAP.md)
