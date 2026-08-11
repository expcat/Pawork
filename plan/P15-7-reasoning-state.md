# P15-7：Reasoning State（跨轮推理状态持久化）

> Phase 15 · Provider Native Capabilities · 状态：🟢已完成 · 交付成熟度：TargetVerified · 依赖：P0-3、P0-4、P1-4、P1-5、P1-6、P5-5、P5-7、P6-5

**最终目的**：让 reasoning / thinking 跨轮保持连续——把各家的加密或不透明回灌凭证（OpenAI `reasoning.encrypted_content`、Anthropic thinking `signature`、xAI reasoning 回灌标识）归一为统一 `ReasoningItem`，**敏感凭证原文存入专用 Protected Blob Store（加密落盘），Event Store 只保存安全引用**（ADR-032），多轮 Compaction 后仍能正确回灌，崩溃后可恢复 continuation。这是 P15-2/3/4 reasoning 往返的共享前置，且与 P5-5/P5-7 Compaction 协同保证压缩不丢推理连续性。

> 安全决策（2026-08 收敛）：reasoning blob **不存入 OS Keychain**。OS Keychain（ADR-014）仅用于小型凭证（API Key / OAuth Token）；reasoning 凭证体积大、频次高、需 retention/GC/compaction 兼容，改用专用 **Protected Blob Store**（ADR-032）。原 P15-7「可不入库」与「crash 后恢复 continuation」冲突，统一为「Event Store 只存安全引用，原文加密落盘于 Protected Blob Store」。

**涉及范围**：`agent-domain`（ReasoningItem 安全引用类型 / 推理状态）、`provider-runtime`（`ProviderStreamEvent.ThinkingDelta` 与 reasoning item 通道）、新增 `protected-blob-store`（加密落盘 + Provider/Session 作用域 + retention + 引用计数 GC）、`session-store` / Projection（只存安全引用的重放）、`compaction-engine`（压缩保留 reasoning）；不实现任何 Provider 特定解码。

## 细分步骤

1. **ReasoningItem 安全引用模型** —— 目的：定义 `ReasoningItem { id, summary?, protected_blob_ref, opaque_metadata, continuation_metadata }`。`protected_blob_ref` 指向 Protected Blob Store 中的加密条目（不透明）；Event Store 与 Projection **只持久化该安全引用 + summary**，绝不内联加密凭证原文。`opaque_metadata` / `continuation_metadata` 仅存回灌所需的非敏感提示（如 `provider_kind`，用于在 provider crate 选翻译器，而非在 Core 走分支）。
2. **Protected Blob Store（ADR-032）** —— 目的：新增加密落盘存储——`encrypted-at-rest`、按 Provider + Session 作用域、写入时不进普通 Event payload、不写日志、不展示给 GUI；提供 retention policy、reference counting / GC、完整性校验。它可复用 `artifact-store` 的存储/寻址底层，但叠加加密与作用域语义；与普通 Blob Store（ADR-004，非加密）和 OS Keychain（ADR-014，小型凭证）三者职责分离。
3. **事件持久化（只存引用）** —— 目的：推理状态作为可持久化事件（P0-3 / ADR-016），经 Event Store append、Projection 可重建；事件载荷只含 `protected_blob_ref`，凭证原文只进 Protected Blob Store；日志与诊断包对 blob 内容脱敏（不出现原文）。
4. **跨轮回灌** —— 目的：在构建后续 CanonicalModelRequest 时，按 `protected_blob_ref` 取回原文并把 ReasoningItem 按各家格式回灌（OpenAI input reasoning items、Anthropic thinking blocks with signature、xAI 回灌）；翻译器在 provider crate，Core 只传 canonical ReasoningItem + 取回的 blob。
5. **Compaction 保留** —— 目的：与 P5-5/P5-7 协同，Compaction 默认保留最近 N 个 ReasoningItem 及其 Protected Blob（保证 extended thinking 连续），压缩策略不可丢弃当前推理链所需的凭证，且 compaction 不破坏当前 reasoning chain 所依赖的 blob 引用计数。
6. **三家凭证对齐表** —— 目的：以文档固化「OpenAI encrypted_content / Anthropic signature / xAI 回灌标识 → ReasoningItem 字段 → Protected Blob」映射，作为 P15-2/3/4 与 P15-9 夹具依据；对不上的返回 `Unsupported` 不猜值。
7. **安全红线守护** —— 目的：凭证原文不解码、不解析、不入日志、不回显到 GUI 明文、不入普通 Event payload、不入 OS Keychain（ADR-032），仅作不透明回灌材料。
8. **Mock smoke 往返 + 压缩 + 恢复** —— 目的：Mock 发射带凭证的 reasoning，验证：原文只进 Protected Blob Store、Event 只存引用、跨轮回灌、Compaction 后保留、crash 后 continuation 可恢复、重放一致；三家凭证映射各一条夹具。

## 主要产出物

- `agent-domain`：`ReasoningItem`（安全引用模型）与推理状态类型
- `protected-blob-store`：加密落盘 + Provider/Session 作用域 + retention + 引用计数 GC
- `provider-runtime`：reasoning item 通道与回灌接口（经 blob ref 取回）
- 三家凭证对齐表（文档）+ Compaction 保留策略接线
- Mock smoke：引用持久化 / 跨轮回灌 / 压缩保留 / crash 恢复 / 重放用例

## 验收标准

- [x] `ReasoningItem` 覆盖三家加密/不透明凭证，缺省为空不猜值；Event Store 只存安全引用，不内联原文
- [x] 凭证原文不解码、不入日志、不入普通 Event payload、不入 OS Keychain、不回显 GUI 明文（红线断言，P15-7 §7 + ADR-032）
- [x] Protected Blob Store 加密落盘、Provider/Session 作用域、retention 与引用计数 GC 生效
- [x] 崩溃后经 blob ref 可重建推理链并恢复 continuation，跨轮回灌连续（重放测试）
- [x] Compaction 后保留最近推理链所需凭证，不中断 extended thinking、不破坏 blob 引用计数
- [x] 回灌翻译在 provider crate，Core 不走 Provider 名称分支（`no_provider_branch` 断言）
- [x] 仅定向/Mock smoke 验收，不要求 workspace 全量门禁

## 验证记录（2026-08-12）

- `ReasoningItem`、流式组装、Event/Projection 重放、跨轮 canonical request、Compaction 默认保留与 crash recovery 均有定向回归；OpenAI、Anthropic、xAI 的凭证提取/重建保持在各自 provider crate，未知 wire 形态返回 `Unsupported`。
- Protected Blob Store 覆盖 XChaCha20-Poly1305、Provider/Session scope、完整性校验、密钥轮换、磁盘预算、引用计数/retention GC，以及 `pending → ready` / `deleting` crash 恢复；Event Store 对 reasoning metadata 使用精确 allowlist，未知及嵌套载荷保持结构脱敏。
- 安全评审发现的 metadata allowlist、文件/元数据 crash 窗口和引用生命周期三项问题已修复；`ReasoningStateBridge` 明确首个事件所有权、append 失败回滚、额外所有者 retain/release 与 GC 契约。
- `cargo test` 与 `cargo clippy --all-targets -- -D warnings` 对 `agent-domain`、`agent-events`、`provider-api`、`protected-blob-store`、`provider-runtime`、`agent-engine`、`test-support`、`session-store`、`compaction-engine`、`context-engine`、`provider-openai`、`provider-anthropic`、`provider-xai`、`provider-openai-compatible`、`provider-google` 全部通过；`cargo fmt --all -- --check`、`cargo run -p schema-typegen -- --check`、`git diff --check` 通过。
- Validation Level：L1。真实 Provider 的 stream producer、请求回灌接线与生产 key resolver 组合由 P15-2 / P15-3 / P15-4 消费本基线；P15-7 以 provider 纯映射 + Mock smoke 完成独立验收。Full workspace gate：NOT RUN（未命中 P15-9 功能簇集中门禁条件）。

**相关文档**：[providers](../docs/features/providers.md) · [sessions](../docs/features/sessions.md) · [context](../docs/features/context.md) · [ADR-014 Secret OS Keychain（凭证范围）](../docs/adr/ADR-014-secret-os-keychain.md) · [ADR-016 事件持久化重放](../docs/adr/ADR-016-core-event-persist-replay.md) · [ADR-032 Protected Blob Store](../docs/adr/ADR-032-protected-blob-store.md) · [安全验收](../docs/quality/security-acceptance.md) · [P15-1](P15-1-canonical-tool-v2.md) · [P15-5](P15-5-server-tool-events.md) · [ROADMAP](../ROADMAP.md)

**依赖建议（2026-08 规划）**：加密落盘使用纯 Rust XChaCha20-Poly1305 AEAD，BLAKE3 只承担密文物理寻址，不自行构造加密算法；不引入 OS Keychain 存储 reasoning blob。依赖方向为 `protected-blob-store → agent-domain`，`session-store` / `provider-runtime → protected-blob-store`；与 `artifact-store`（非加密）平级、复用运行时原语但不混用命名空间或安全语义。
