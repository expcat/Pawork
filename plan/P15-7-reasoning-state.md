# P15-7：Reasoning State（跨轮推理状态持久化）

> Phase 15 · Provider Native Capabilities · 状态：🟡未开始 · 交付成熟度：Designed · 依赖：P0-3、P0-4、P1-4、P1-5、P5-5、P5-7、P6-5

**最终目的**：让 reasoning / thinking 跨轮保持连续——把各家的加密或不透明回灌凭证（OpenAI `reasoning.encrypted_content`、Anthropic thinking `signature`、xAI reasoning 回灌标识）抽象为统一 `ReasoningItem`，按事件持久化（ADR-016），多轮 Compaction 后仍能正确回灌，凭证作不透明 blob 处理（不解码、不入日志）。这是 P15-2/3/4 reasoning 往返的共享前置，且与 P5-5/P5-7 Compaction 协同保证压缩不丢推理连续性。

**涉及范围**：`agent-domain`（ReasoningItem / 推理状态类型）、`provider-runtime`（`ProviderStreamEvent.ThinkingDelta` 与 reasoning item 通道）、`session-store` / Projection（持久化重放）、`compaction-engine`（压缩保留 reasoning）；不实现任何 Provider 特定解码。

## 细分步骤

1. **ReasoningItem 领域类型** —— 目的：定义 `ReasoningItem { id, summary?, encrypted_blob?, opaque_token?, provider_kind }`，`encrypted_blob`/`opaque_token` 作不透明 byte/字符串存储（OpenAI encrypted_content、Anthropic signature、xAI 回灌标识统一归一），`provider_kind` 用于回灌时选翻译器而非在 Core 走分支。
2. **事件持久化** —— 目的：把推理状态作为可持久化事件（P0-3 / ADR-016），经 Event Store append、Projection 可重建；凭证字段在入库与日志中脱敏（仅存引用，原文进 SecretBackend 或不落库）。
3. **跨轮回灌** —— 目的：在构建后续 CanonicalModelRequest 时，把保留的 ReasoningItem 按各家格式回灌（OpenAI input reasoning items、Anthropic thinking blocks with signature、xAI 回灌）；翻译器在 provider crate，Core 只传 canonical ReasoningItem。
4. **Compaction 保留** —— 目的：与 P5-5/P5-7 协同，Compaction 默认保留最近 N 个 ReasoningItem（保证 extended thinking 连续），压缩策略不可丢弃当前推理链所需的加密凭证。
5. **三家凭证对齐表** —— 目的：以文档固化「OpenAI encrypted_content / Anthropic signature / xAI 回灌标识 → ReasoningItem 字段」映射，作为 P15-2/3/4 与 P15-9 夹具依据；对不上的返回 `Unsupported` 不猜值。
6. **安全红线守护** —— 目的：凭证不解码、不解析、不入日志、不回显到 GUI 明文（与 ADR-014 Secret 处理口径一致），仅作不透明回灌材料。
7. **Mock smoke 往返 + 压缩** —— 目的：Mock 发射带凭证的 reasoning，验证落库、跨轮回灌、Compaction 后保留、重放一致；三家凭证映射各一条夹具。

## 主要产出物

- `agent-domain`：`ReasoningItem` 与推理状态类型
- `provider-runtime`：reasoning item 通道与回灌接口
- 三家凭证对齐表（文档）+ Compaction 保留策略接线
- Mock smoke：持久化 / 跨轮回灌 / 压缩保留 / 重放用例

## 验收标准

- [ ] `ReasoningItem` 覆盖三家加密/不透明凭证，缺省为空不猜值
- [ ] 凭证字段不解码、不入日志、不回显明文（红线断言，P15-7 §6）
- [ ] 崩溃后 Projection 可重建推理链，跨轮回灌连续（重放测试）
- [ ] Compaction 后保留最近推理链所需凭证，不中断 extended thinking
- [ ] 回灌翻译在 provider crate，Core 不走 Provider 名称分支（`no_provider_branch` 断言）
- [ ] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

**相关文档**：[providers](../docs/features/providers.md) · [sessions](../docs/features/sessions.md) · [context](../docs/features/context.md) · [ADR-014 Secret OS Keychain](../docs/adr/ADR-014-secret-os-keychain.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：纯领域扩展，不新增依赖；凭证原文若需持久化走 SecretBackend（ADR-014），不落明文。
